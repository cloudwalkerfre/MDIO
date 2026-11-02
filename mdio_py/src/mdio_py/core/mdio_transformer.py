import ast
from lark import Transformer, v_args, Token
from .mdio_ir import *
from .mdio_ir import _prepend_wildcard
from .error import MdioError, ErrCode

DEBUG = False

class MdioString(str):
    """偷运元数据的隐形特工"""
    is_triple_quoted: bool = False
    def __new__(cls, value, is_triple=False):
        obj = str.__new__(cls, value)
        obj.is_triple_quoted = is_triple
        return obj

@v_args(inline=True)
class MdioTransformer(Transformer):

    # ── 路径独占终端 → (type, value) 元组 ──
    def UNDERSCORE(self, t):     return PIPE_DATA
    def TYPE_ANNOT(self, t):     return ('TYPE_ANNOT', str(t))
    def RECURSIVE_SEP(self, t):  return ('RECURSIVE_SEP', str(t))
    def WILDCARD(self, t):       return ('WILDCARD', str(t))

    # Top Level Structure
    def start(self, *stmts):
        res = []
        for s in stmts:
            if isinstance(s, list): res.extend(s)
            elif s: res.append(s)
        return res

    def stmt_seq(self, *stmts):
        return [s for s in stmts if s]

    def stmt(self, pipe_chain):
        return pipe_chain

    def pipe_chain(self, *atoms):
        return Pipeline(atoms=[a for a in atoms if a is not None])

    def exec_block(self, pipe_chain=None):
        return ExecBlock(pipeline=pipe_chain or Pipeline([]))

    def scope_block(self, locator, stmts):
        if isinstance(locator, SelectorIR):
            fs_path = locator.file_path or ""
            ctx_id = locator.context_id
            loc_steps = locator.steps
        else:
            fs_path = str(locator)
            ctx_id = None
            loc_steps = []
        # 容错：去除尾部的 ::
        if fs_path.endswith('::'): fs_path = fs_path[:-2]

        if stmts is None: stmts = []
        elif not isinstance(stmts, list): stmts = [stmts]
        return ScopeBlock(file_path=fs_path, pipeline=stmts, context_id=ctx_id, steps=loc_steps)

    # The Universal Path Builder
    def locator_core(self, *args):
        if DEBUG: print(f"[DEBUG locator_core] raw args: {args}")

        context_id = None
        file_path = None
        # typed_parts = 统一步骤序列：path token（tuple）与 filter 表达式
        # （list）按书写顺序混合，filter 按书写位置生成，
        # 保证"书写顺序 = 执行顺序"（filter 后投影 / slice 位置语义统一）。
        typed_parts = []

        for arg in args:
            if arg is None:
                continue
            if isinstance(arg, int) or isinstance(arg, MacroToken):
                context_id = arg; continue
            # list 参数统一元素级处理（保持书写顺序）：
            #   path_chain 产物  → [(type, value), ...]（tuple）
            #   step_chain 产物  → tuple 与 filter 表达式（list）混合
            #   path_tail 产物   → Token 列表
            #   filter 表达式    → ['CMP'|'REGEX'|..., ...]
            if isinstance(arg, list):
                for item in arg:
                    if isinstance(item, tuple):
                        typed_parts.append(item)
                    elif isinstance(item, list):
                        typed_parts.append(item)
                    elif hasattr(item, 'type'):
                        typed_parts.append((item.type, str(item)))
                    elif isinstance(item, str):
                        typed_parts.append(('PATH_IDENT', item))
                continue
            # Token (COLON, DOUBLE_COLON, key_spec 的 PATH_IDENT 等)
            if hasattr(arg, 'type'):
                typed_parts.append((arg.type, str(arg))); continue
            # key_spec 可能返回纯字符串
            if isinstance(arg, str):
                typed_parts.append(('PATH_IDENT', arg)); continue

        if DEBUG: print(f"[DEBUG locator_core] typed_parts={typed_parts}")

        # ── 第二遍：将 typed_parts 转换为 steps ──
        # 先找出 :: 位置，拆分 file_path 和 head 部分
        double_colon_indices = [i for i, item in enumerate(typed_parts)
                                if isinstance(item, tuple) and item[0] == 'DOUBLE_COLON']
        if double_colon_indices:
            dc_idx = double_colon_indices[0]
            before_dc = typed_parts[:dc_idx]
            after_dc = typed_parts[dc_idx+1:]
            remaining_dc = [i for i, item in enumerate(after_dc)
                            if isinstance(item, tuple) and item[0] == 'DOUBLE_COLON']
            if remaining_dc:
                dc2 = remaining_dc[0]
                file_parts = after_dc[:dc2]
                head_parts = after_dc[dc2+1:]
                file_path = "".join(pv for _, pv in file_parts)
                process_parts = head_parts
                # Three-segment: before_dc is worktree name → store as context_id
                if before_dc and context_id is None:
                    ns_name = "".join(pv for _, pv in before_dc)
                    context_id = ns_name
            else:
                if before_dc:
                    file_path = "".join(pv for _, pv in before_dc)
                process_parts = after_dc
        else:
            process_parts = typed_parts
            # 无 :: 时，若路径以 / 开头且不含 COLON，则是纯文件路径（非 selector）
            if typed_parts and isinstance(typed_parts[0], tuple) and typed_parts[0][0] == 'SLASH':
                has_colon = any(isinstance(item, tuple) and item[0] == 'COLON' for item in typed_parts)
                if not has_colon:
                    file_path = "".join(pv for _, pv in typed_parts)
                    process_parts = []

        steps = []
        tag_suffix = []
        i = 0
        while i < len(process_parts):
            item = process_parts[i]

            # filter 表达式（含 Slice）——按书写位置生成（取代延迟追加）
            if isinstance(item, list):
                steps.append(item)
                i += 1; continue

            tt, tv = item

            if tt in ('PATH_IDENT', 'FUNC_NAME'):
                steps.append(["Drill", tv])
                i += 1; continue

            if tt == 'TYPE_ANNOT':
                annot = tv[1:] if tv.startswith('_') else tv
                type_name = TYPE_ANNOT.get(annot, annot)
                _prepend_wildcard(steps)
                steps.append(["TYPECHECK", type_name])
                i += 1; continue

            if tt == 'RECURSIVE_SEP':
                if i + 1 < len(process_parts) and isinstance(process_parts[i + 1], tuple) \
                        and process_parts[i + 1][0] in ('PATH_IDENT', 'FUNC_NAME'):
                    steps.append(["RecursiveDrill", process_parts[i + 1][1]])
                    i += 2
                else:
                    steps.append(["RecursiveWildcard"])
                    i += 1
                continue

            if tt == 'WILDCARD':
                if not steps or steps[-1] != ["RecursiveWildcard"]:
                    steps.append(["Wildcard"])
                i += 1; continue

            if tt == 'COLON':
                if i + 1 < len(process_parts) and isinstance(process_parts[i + 1], tuple) \
                        and process_parts[i + 1][0] in ('PATH_IDENT', 'FUNC_NAME'):
                    key_val = process_parts[i + 1][1]
                    # 仅在前面没有 descender/类型约束/定位切片时补 Wildcard
                    # （避免与 ._b 等 TYPE_ANNOT 重复；Slice 已定位到具体
                    # 节点——`_b[-1]:k` 对第 -1 个 bullet 自身匹配）
                    if not steps or steps[-1] not in (["Wildcard"], ["RecursiveWildcard"]):
                        if not (steps and steps[-1] and steps[-1][0] in ("TYPECHECK", "Slice")):
                            steps.append(["Wildcard"])
                    tag_suffix = [["CMP", ".tag", "==", key_val], ["Slice", 0]]
                    i += 2
                else:
                    i += 1
                continue

            if tt == 'DOUBLE_COLON':
                i += 1; continue

            i += 1

        # ── :tag suffix（key 定位的终结展开）──
        if tag_suffix:
            steps.extend(tag_suffix)

        if DEBUG: print(f"[DEBUG Transformer] SelectorIR: ctx={context_id} fp={file_path} steps={steps}")
        return SelectorIR(context_id=context_id, file_path=file_path, steps=steps)

    # 必须要让这三个辅助函数只返回普通字符串！
    def path_chain(self, *parts):
        # 实际行为：?path_part+ → 1 个 list arg, list 内是 [Token|tuple, ...]
        typed = []
        for p in parts:
            if isinstance(p, list):
                for item in p:
                    if hasattr(item, 'type'):
                        typed.append((item.type, str(item)))
                    elif isinstance(item, tuple):
                        typed.append(item)
                    elif isinstance(item, str):
                        typed.append(('PATH_IDENT', str(item)))
            elif hasattr(p, 'type'):
                typed.append((p.type, str(p)))
            elif isinstance(p, tuple):
                typed.append(p)
            elif isinstance(p, str):
                typed.append(('PATH_IDENT', str(p)))
        return typed

    def path_part(self, part):
        if hasattr(part, 'is_triple_quoted'):
            return ('PATH_IDENT', str(part))
        if isinstance(part, tuple):
            return part
        return ('PATH_IDENT', str(part))

    def path_prefix(self, *parts):
        # SLASH path_part* → 1 list arg (SLASH 在 list 内)
        typed = []
        for p in parts:
            if isinstance(p, list):
                for item in p:
                    if hasattr(item, 'type'):
                        typed.append((item.type, str(item)))
                    elif isinstance(item, tuple):
                        typed.append(item)
            elif hasattr(p, 'type'):
                typed.append((p.type, str(p)))
            elif isinstance(p, tuple):
                typed.append(p)
        return typed

    def key_spec(self, key):
        k = str(key)
        # 用脱壳逻辑兼容带引号的 Key
        if len(k) >= 2 and (k.startswith('"') and k.endswith('"')) or (k.startswith("'") and k.endswith("'")):
            return k[1:-1]
        return k

    # Operations
    def basic_op(self, op):
        return op
    
    def locator(self, loc):
        return loc
    
    def read_op(self, *args):
        if DEBUG: print(f"[DEBUG Transformer read_op] raw args: {args}")
        clean_args, kwargs = self._extract_args_and_kwargs(args)
        selector = clean_args[-1]
        if DEBUG: print(f"[DEBUG Transformer read_op] selector before wrap: {selector}")
        if isinstance(selector, ExecBlock):
            # 动态路径：保持 ExecBlock，由 interpreter 执行
            if DEBUG: print(f"[DEBUG Transformer read_op] keeping ExecBlock as dynamic path")
            return ReadOp(selector=selector, kwargs=kwargs)
        if not isinstance(selector, SelectorIR):
            selector = SelectorIR(file_path=str(selector), steps=[])
        if DEBUG: print(f"[DEBUG Transformer read_op] final selector: {selector}")
        return ReadOp(selector=selector, kwargs=kwargs)

    def mem_read_op(self, *args):
        clean_args, kwargs = self._extract_args_and_kwargs(args)
        if not clean_args:
            # $$ -n __name: no locator, the whole current pipe data is the mem.
            return MemReadOp(selector=SelectorIR(steps=[]), kwargs=kwargs)
        selector = clean_args[-1]
        if isinstance(selector, ExecBlock):
            return MemReadOp(selector=selector, kwargs=kwargs)
        if not isinstance(selector, SelectorIR):
            selector = SelectorIR(file_path=str(selector), steps=[])
        return MemReadOp(selector=selector, kwargs=kwargs)

    def write_op(self, *args):
        clean_args, kwargs = self._extract_args_and_kwargs(args)
        operator = str(clean_args[0])
        selector = clean_args[-1]
        if isinstance(selector, ExecBlock):
            return WriteOp(operator=operator, selector=selector, kwargs=kwargs)
        if isinstance(selector, Op) or not isinstance(selector, SelectorIR):
            selector = SelectorIR(file_path=str(selector), steps=[])
        return WriteOp(operator=operator, selector=selector, kwargs=kwargs)

    def hook_op(self, *args):
        # args 的第一个元素是 Token('HOOK_KW')，后面是 kwarg_flag 元组
        # 使用已有的 _extract_args_and_kwargs 辅助函数
        _, kwargs = self._extract_args_and_kwargs(args)
        return HookOp(kwargs=kwargs)
    
    def MACRO_IDENT(self, token):
        # 将 %xx 削减为纯名字 xx
        return MacroToken(name=str(token)[1:])
    
    def ctx_prefix(self, val, _dc):
        if getattr(val, 'type', None) == 'NUMBER':
            return int(str(val))
        return val

    # Filters & Logic
    def step_chain(self, *parts):
        """path/filter 交错序列 → 扁平混合列表（tuple path token 与
        filter 表达式 list 按书写顺序）。locator_core 按元素处理。"""
        out = []
        for p in parts:
            if p is None:
                continue
            if isinstance(p, list):
                out.extend(p)
            else:
                out.append(p)
        return out

    def path_tail(self, *parts):
        """filter 后的投影段（drill/递归/类型标注/通配）——tuple 化，
        与 path_chain 产物同构。"""
        typed = []
        for p in parts:
            if isinstance(p, list):
                for item in p:
                    if hasattr(item, 'type'):
                        typed.append((item.type, str(item)))
                    elif isinstance(item, tuple):
                        typed.append(item)
                    elif isinstance(item, str):
                        typed.append(('PATH_IDENT', str(item)))
            elif hasattr(p, 'type'):
                typed.append((p.type, str(p)))
            elif isinstance(p, tuple):
                typed.append(p)
            elif isinstance(p, str):
                typed.append(('PATH_IDENT', str(p)))
        return typed

    def filter_chain(self, *exprs): return list(exprs)
    def filter_expr(self, c): return c
    def filter_content(self, c): return c

    def or_expr(self, *args):
        clean = [a for a in args if str(a) != "||"]
        return ['OR', clean] if len(clean) > 1 else clean[0]

    def and_expr(self, *args):
        clean = [a for a in args if str(a) != "&&"]
        return ['AND', clean] if len(clean) > 1 else clean[0]

    def negated_condition(self, expr):
        return ['NOT', expr]

    def grouped_condition(self, expr):
        return expr

    def value(self, item):
        # 兼容 Cython Token 鸭子类型
        if getattr(item, 'type', None) == 'NUMBER':
            s = str(item)
            return float(s) if '.' in s else int(s)
        return item

    def comparison_op(self, item): return str(item)
    def comparison_op(self, item): return str(item)
    def comparison_expr(self, field, op, val): return ['CMP', str(field), op, val]
    def regex_match(self, field, pattern): return ['REGEX', str(field), str(pattern)]

    # Type Expr & Methods
    KNOWN_METHODS = {'has','eq','re','empty'}

    def type_expr(self, annot_token, method=None):
        annot = str(annot_token)
        type_name = TYPE_ANNOT.get(annot, annot)
        if method is None:
            return ['TYPECHECK', type_name]
        # 有 method 时组合为 AND: [TYPECHECK, type_name] AND method_result
        type_check = ['TYPECHECK', type_name]
        return ['AND', [type_check, method]]

    # ── method ──
    def method(self, *args):
        """args contains inline children from one of the 5 method alternatives"""
        # First arg is always the list of children
        items = args[0] if len(args) == 1 and isinstance(args[0], list) else list(args)
        # Flatten nested Trees
        flat = []
        for x in items:
            if hasattr(x, 'data') and hasattr(x, 'children'):
                # Tree('method_arg', [value]) → unwrap
                for c in x.children:
                    flat.append(c)
            else:
                flat.append(x)
        str_flat = [str(x) for x in flat]

        # 1. cmp_method: FILTER_FIELD comparison_op value
        if len(flat) >= 3 and any(s in ('==', '!=', '=', '>=', '<=') for s in str_flat):
            return ['CMP', str_flat[0], flat[1], flat[2]]

        # 2. bare_empty_method: EMPTY_KW only
        if len(flat) == 1 and 'empty' in str_flat[0]:
            return ['EMPTY', '.raw']

        # 3. field_empty_method: FILTER_FIELD, EMPTY_KW
        if len(flat) == 2 and 'empty' in str_flat[1]:
            return ['EMPTY', str_flat[0]]

        # 4. bare_method: PATH_IDENT, method_arg
        #    field_method: FILTER_FIELD, PATH_IDENT, method_arg
        if len(flat) >= 2:
            field = str_flat[0] if str_flat[0].startswith('.') else '.raw'
            # PRE-RAW：field_method（'.raw' 起头且 ≥3 元素——[.raw, method, arg]）
            # 与 bare_method（[method, arg]）按元素数区分：field_method = [.raw, method, arg]，
            # bare_method = [method, arg]。
            if field != '.raw' or len(flat) >= 3:
                # field_method: field=FILTER_FIELD, method=flat[1], arg=flat[2]
                method_name = str_flat[1]
                arg_val = str(flat[2]) if len(flat) > 2 else ''
            else:
                # bare_method: field=.raw, method=flat[0], arg=flat[1]
                method_name = str_flat[0]
                arg_val = str(flat[1]) if len(flat) > 1 else ''
            if method_name not in ('re', 'has', 'eq', 'empty'):
                raise MdioError("parser", ErrCode.PARSE_TOKEN,
                                f"未知过滤方法 '{method_name}'(仅支持 has/eq/re/empty)")
            tag = {'re':'REGEX','has':'HAS','eq':'EQ','empty':'EMPTY'}.get(method_name, method_name.upper())
            return [tag, field, arg_val]

        return ['EMPTY', '.content']

    # ── by[...] evidence filter ──
    def by_method(self, *args):
        """FILTER_FIELD . by [ filter_content ] by_tail? → BY IR"""
        field = str(args[0])
        inner_filter = args[2]  # args[1] is the "by" token
        tail = args[3] if len(args) > 3 else None
        return self._build_by_ir(field, inner_filter, tail)

    def by_bare_method(self, *args):
        """. by [ filter_content ] by_tail? → BY with default field .content"""
        inner_filter = args[1]  # args[0] is the "by" token
        tail = args[2] if len(args) > 2 else None
        return self._build_by_ir('.content', inner_filter, tail)

    def _build_by_ir(self, field, inner_filter, tail):
        if tail is None:
            return ['BY', str(field), inner_filter]
        if tail[0] == 'EMPTY':
            return ['BY_EMPTY', str(field), inner_filter]
        # tail = [quantifier_type, predicate_filter]
        return [f'BY_{tail[0]}', str(field), inner_filter, tail[1]]

    def by_empty(self, *_):
        return ['EMPTY']

    def by_quantifier(self, name, inner):
        return [str(name).upper(), inner]

    def slice_chain(self, *items):
        clean = [i for i in items if i is not None and str(i) != ","]
        return ['Slice', clean]

    def slice_item(self, item):
        if getattr(item, 'type', None) == 'NUMBER':
            return int(str(item))
        return item

    def range_slice(self, *args):
        parts = [None, None, None]
        slot = 0
        for item in args:
            if item is None: continue
            if getattr(item, 'type', None) == 'COLON' or str(item) == ':':
                slot += 1
                slot = min(slot, 2)
            else:
                # 强制 str() 脱壳，防止 Cython token 直接进 int() 引发崩溃
                v = int(str(item)) if getattr(item, 'type', None) == 'NUMBER' else item
                parts[slot] = v
        return ['RANGE', parts[0], parts[1], parts[2]]

    # Functions & Plugins
    def arg_atom(self, atom):
        if getattr(atom, 'type', None) == 'UNDERSCORE':
            return PIPE_DATA
        return atom

    def kwarg_value(self, val):
        if getattr(val, 'type', None) == 'UNDERSCORE':
            return PIPE_DATA
        if str(val) == "_":
            return PIPE_DATA
        return val
         
    def func_call(self, *args):
        clean_args, kwargs = self._extract_args_and_kwargs(args)
        raw_name = str(clean_args[0])
        name = raw_name[1:] if raw_name.startswith('_') else raw_name
        params = clean_args[1] if len(clean_args) > 1 else []
        return FuncCall(name=name, args=params, kwargs=kwargs)

    def plugin_call(self, *args):
        clean_args, kwargs = self._extract_args_and_kwargs(args)
        protocol = str(clean_args[0])
        chain = []
        i = 1
        while i < len(clean_args):
            item = clean_args[i]
            # 兼容 Cython Token
            if getattr(item, 'type', None) in ('PATH_IDENT', 'FUNC_NAME'):
                if i + 1 < len(clean_args) and isinstance(clean_args[i+1], list):
                    chain.append({"method": str(item), "args": clean_args[i+1]})
                    i += 2 
                else:
                    chain.append(str(item))
                    i += 1
            else:
                i += 1
        return PluginCall(protocol=protocol, chain=chain, kwargs=kwargs)

    def args(self, *a):
        """在这里就把传入函数的包装壳（Literal/FString/Token）全部褪去，只给引擎留纯粹的 Python 数据"""
        clean_args = []
        for x in a:
            # 兼容 Cython Token (判断鸭子模型或类名)
            if type(x).__name__ == 'Token' or (hasattr(x, 'type') and hasattr(x, 'value') and not hasattr(x, 'raw_value')): 
                continue
                
            # 脱掉 Literal 的壳
            if hasattr(x, 'value'):
                clean_args.append(x.value)
            elif hasattr(x, 'raw_value'):
                clean_args.append(x.raw_value)
            else:
                clean_args.append(x)
                
        return clean_args

    # Kwargs 脱壳器
    def kwarg_path(self, *parts):
        # 兼容 Lark_Cython Token 鸭子类型
        if len(parts) == 1 and (type(parts[0]).__name__ == 'Token' or hasattr(parts[0], 'type')):
            p = parts[0]
            is_trip = getattr(p, 'type', None) in ('TRIPLE_STRING_DOUBLE', 'TRIPLE_STRING_SINGLE', 'RAW_TRIPLE_STRING_DOUBLE', 'RAW_TRIPLE_STRING_SINGLE')
            # 顺便剥皮
            s = str(p)
            unquoted = s[3:-3] if is_trip else s[1:-1] if s.startswith(('"', "'")) else s
            return MdioString(unquoted, is_triple=is_trip)
            
        # 已是 MdioString 包装的单个 token 直接返回，不执行下面的 join()
        if len(parts) == 1 and hasattr(parts[0], 'is_triple_quoted'):
            return parts[0]
            
        # --- 原始的拼合逻辑，用于处理没有引号的长路径 ---
        res = []
        for p in parts:
            if isinstance(p, list):
                for item in p:
                    if hasattr(item, 'raw_value'): res.append(str(item.raw_value))
                    elif hasattr(item, 'value'): res.append(str(item.value))
                    elif isinstance(item, tuple): res.append(str(item[1]))
                    else: res.append(str(item))
            elif hasattr(p, 'raw_value'): res.append(str(p.raw_value))
            elif hasattr(p, 'value'): res.append(str(p.value))
            elif isinstance(p, tuple): res.append(str(p[1]))
            else: res.append(str(p))
            
        s = "".join(res)
        if len(s) >= 2 and s[0] in "'\"" and s[-1] == s[0]:
            is_triple = False
            if len(s) >= 6 and (s.startswith('"""') or s.startswith("'''")):
                unquoted = s[3:-3]
                is_triple = True
            else:
                unquoted = s[1:-1]
            return MdioString(unquoted, is_triple)
        return s

    def _unwrap_val(self, val):
        # 保留隐性布尔特征
        if val is None: return True
        if val is PIPE_DATA: return PIPE_DATA
        
        # 先确认是字符串再比对，避免大对象被 str() 分配内存
        if isinstance(val, str) and val == "_": return PIPE_DATA
        
        # 兜底兼容 Lark 的 Token 解析树
        if getattr(val, 'type', None) == 'UNDERSCORE': return PIPE_DATA
        if hasattr(val, 'raw_value'): return val.raw_value
        if hasattr(val, 'value'): return val.value
        return val

    # def _unwrap_val(self, val):
    #     """安检传输带：卸下 Literal 外壳，但保护 MdioString 特工"""
    #     if val is None: return True
    #     if val is PIPE_DATA: return PIPE_DATA
        
    #     # 兼容 Cython Token
    #     if getattr(val, 'type', None) == 'UNDERSCORE': return PIPE_DATA
    #     if str(val) == "_": return PIPE_DATA
    #     if hasattr(val, 'raw_value'): return val.raw_value
    #     if hasattr(val, 'value'): return val.value
    #     return val

    def short_flag(self, key, val=None):
        return ('KWARG', f"-{key}", self._unwrap_val(val))

    def long_flag(self, key, val=None):
        return ('KWARG', f"--{key}", self._unwrap_val(val))

    def _extract_args_and_kwargs(self, items):
        """只分拣普通参数与 Kwargs，全凭规矩办事"""
        clean_args = []
        kwargs = {}
        for item in items:
            if isinstance(item, tuple) and item[0] == 'KWARG':
                full_key, val = item[1], item[2]
                kwargs[full_key] = val
            else:
                clean_args.append(item)
        return clean_args, kwargs

    # Literals
    # def literal(self, val): return Literal(val)
    def literal(self, val): 
        # 兜底：如果 val 已经是 Literal 就不再包了
        if isinstance(val, Literal): return val
        return Literal(val)

    def string_expr(self, *parts): return "".join(parts)

    def string_lit(self, val):
        s = str(val)
        is_triple = s.startswith(('"""', "'''", 'r"""', "r'''"))
        
        if s.startswith(('r"""', "r'''")): 
            unquoted = s[4:-3]
        elif s.startswith(('"""', "'''")): 
            unquoted = s[3:-3]
        elif s.startswith(('r"', "r'")): 
            unquoted = s[2:-1]
        else:
            unquoted = s[1:-1]
            if '\\' in unquoted:
                replacements = [('\\"', '"'), ("\\'", "'"), ('\\\\', '\\'), ('\\n', '\n'), ('\\t', '\t'), ('\\r', '\r')]
                for old, new in replacements:
                    unquoted = unquoted.replace(old, new)
                
        return MdioString(unquoted, is_triple=is_triple)

    # def string_lit(self, val):
    #     s = str(val)
    #     is_triple = s.startswith(('"""', "'''", 'r"""', "r'''"))
        
    #     if s.startswith(('r"""', "r'''")): 
    #         unquoted = s[4:-3]
    #     elif s.startswith(('"""', "'''")): 
    #         unquoted = s[3:-3]
    #     elif s.startswith(('r"', "r'")): 
    #         unquoted = s[2:-1]
    #     else:
    #         unquoted = s[1:-1]
    #         replacements = [('\\"', '"'), ("\\'", "'"), ('\\\\', '\\'), ('\\n', '\n'), ('\\t', '\t'), ('\\r', '\r')]
    #         for old, new in replacements:
    #             unquoted = unquoted.replace(old, new)
                
    #     #  终点发车：所有情况都统一用特工包装！
    #     return MdioString(unquoted, is_triple=is_triple)


    def f_string(self, _, val): return FString(raw_value=val)
    def number_lit(self, val): s = str(val); return float(s) if '.' in s else int(s)
    def bool_lit(self, val): return str(val).lower() == 'true'

    def list_lit(self, *items):
        result = []
        for item in items:
            if item is None: continue
            if isinstance(item, Literal): result.append(item.value)
            elif isinstance(item, (str, int, float, bool, list, dict)): result.append(item)
            else: result.append(str(item))
        # 不包 Literal：直接返回 list，外层的 atom/literal 函数会统一打包
        return result

    def dict_lit(self, *items):
        d = {}
        for item in items:
            if isinstance(item, tuple):
                k, v = item
                k_str = str(k).strip('"\'').replace('\\"', '"').replace("\\'", "'")
                d[k_str] = v.value if isinstance(v, Literal) else v
        # 不包 Literal：直接返回 dict。
        return d

    def pair(self, k, v):
        return (k, v)
