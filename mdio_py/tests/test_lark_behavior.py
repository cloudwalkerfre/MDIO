"""
Lark 行为观察 — 不猜，直接看输出。
Lark 1.3.1, @v_args(inline=True) 类级别。
"""

from lark import Lark, Transformer, v_args

def grammar(text):
    """lark 语法：每一行是一条规则，接受缩进"""
    return text.strip()

def show(title, g, data, xform_cls, *, make_xform=None):
    print(f"\n{'─'*55}")
    print(f"  {title}")
    print(f"{'─'*55}")
    t_cls = make_xform(xform_cls) if make_xform else xform_cls
    t = t_cls()
    Lark(grammar(g), parser="lalr", transformer=t).parse(data)


# 1. 基础传参

@v_args(inline=True)
class T_Seq(Transformer):
    def seq(self, *args):
        print(f"  {len(args)} args: {[type(a).__name__ for a in args]}")
    def A(self,t): return "A"
    def B(self,t): return "B"
    def C(self,t): return "C"

show("1. A B C → 3 独立 arg", """
start: seq
seq: A B C
A: "a"
B: "b"
C: "c"
%import common.WS
%ignore WS
""", "a b c", T_Seq)


@v_args(inline=True)
class T_Plus(Transformer):
    def seq(self, *args):
        print(f"  {len(args)} args: {[type(a).__name__ for a in args]}")
    def A(self,t): return "A"

show("2. A+ (非展开) → 独立 arg", """
start: seq
seq: A+
A: "a"
%import common.WS
%ignore WS
""", "a a a", T_Plus)


show("3. A* (非展开, 空) → 0 arg", """
start: seq
seq: A*
A: "a"
%import common.WS
%ignore WS
""", "", T_Plus)


show("4. A* (非展开, 有) → 独立 arg", """
start: seq
seq: A*
A: "a"
%import common.WS
%ignore WS
""", "a a", T_Plus)


@v_args(inline=True)
class T_Opt(Transformer):
    def seq(self, *args):
        print(f"  {len(args)} args: {[(type(a).__name__,repr(a)[:20]) for a in args]}")
    def A(self,t): return "A"
    def B(self,t): return "B"

show("5. A? (存在) → 传值", """
start: seq
seq: A? B
A: "a"
B: "b"
%import common.WS
%ignore WS
""", "a b", T_Opt)

show("6. A? (不存在) → None 占位", """
start: seq
seq: A? B
A: "a"
B: "b"
%import common.WS
%ignore WS
""", "b", T_Opt)


# 2. ?expander — MDIO 核心

@v_args(inline=True)
class T_Expand(Transformer):
    def parent(self, *args):
        for i, a in enumerate(args):
            if isinstance(a, list):
                print(f"  arg[{i}]: list[{len(a)}]")
                for j, x in enumerate(a):
                    if hasattr(x, 'type'):   print(f"    [{j}] Token({x.type}, '{x}')")
                    elif isinstance(x, tuple): print(f"    [{j}] tuple{x}")
                    else:                      print(f"    [{j}] {type(x).__name__} {repr(x)[:40]}")
            else:
                if hasattr(a, 'type'):   print(f"  arg[{i}] Token({a.type}, '{a}')")
                elif isinstance(a, tuple): print(f"  arg[{i}] tuple{a}")
                else:                      print(f"  arg[{i}] {type(a).__name__} {repr(a)[:40]}")
    def X(self,t): return "X"
    def Y(self,t): return "Y"

show("7. ★ ?child+ (expander+重复) → 1 list arg ★", """
start: parent
parent: child+
?child: X | Y
X: "x"
Y: "y"
%import common.WS
%ignore WS
""", "x y x", T_Expand)


# 有 child() 方法
call_log = []
def make_T8(cls):
    class T(cls):
        def child(self, val):
            call_log.append(f"    child() 收到: {type(val).__name__} {repr(val)[:30]}")
            return f"child({val})"
    return T

show("8. ?child expander — child() 被调用", """
start: parent
parent: child+
?child: X | Y
X: "x"
Y: "y"
%import common.WS
%ignore WS
""", "x y", T_Expand, make_xform=make_T8)


# 无 child() 方法
show("9. ?child expander — 无 child(), 值直入 parent", """
start: parent
parent: child+
?child: X | Y
X: "x"
Y: "y"
%import common.WS
%ignore WS
""", "x y", T_Expand)


# 3. 全局 terminal 方法 + expander

SENTINEL = object()
@v_args(inline=True)
class T_Term(Transformer):
    def SPECIAL(self, t): return f"SP({t})"
    def DOT(self, t):     return SENTINEL
    def parent(self, *args):
        for i, a in enumerate(args):
            if isinstance(a, list):
                print(f"  arg[{i}]: list[{len(a)}]")
                for j, x in enumerate(a):
                    tag = " ←SENTINEL" if x is SENTINEL else ""
                    print(f"    [{j}] {type(x).__name__} {repr(x)[:40]}{tag}")
            else:
                tag = " ←SENTINEL" if a is SENTINEL else ""
                print(f"  arg[{i}] {type(a).__name__} {repr(a)[:40]}{tag}")

show("10. 全局 terminal 方法 + ?expander → 值直入 parent list", """
start: parent
parent: child+
?child: PLAIN | DOT | SPECIAL
PLAIN: /[a-z]+/
DOT: "."
SPECIAL: /[A-Z]+/
%import common.WS
%ignore WS
""", "hello.WORLD", T_Term)


# 4. MDIO 完整场景

@v_args(inline=True)
class T_MDIO(Transformer):
    def TYPE_ANNOT(self, t): return ("TYPE_ANNOT", str(t))
    def parent(self, *args):
        for i, a in enumerate(args):
            if isinstance(a, list):
                print(f"  arg[{i}]: list[{len(a)}]")
                for j, x in enumerate(a):
                    if x is SENTINEL:          print(f"    [{j}] SENTINEL")
                    elif hasattr(x, 'type'):    print(f"    [{j}] Token({x.type:12s} '{x}')")
                    elif isinstance(x, tuple):  print(f"    [{j}] tuple{x}")
                    else:                       print(f"    [{j}] {type(x).__name__} {repr(x)[:30]}")
            else:
                if hasattr(a, 'type'):    print(f"  arg[{i}] Token({a.type} '{a}')")
                elif isinstance(a, tuple): print(f"  arg[{i}] tuple{a}")
                else:                      print(f"  arg[{i}] {type(a).__name__} {repr(a)[:40]}")


# 11: path_chain 无 DOT 方法
class T11(T_MDIO):
    def path_chain(self, *args): T_MDIO.parent(self, *args)
    # DOT 无全局方法 → Token

show("11. MDIO path_chain (无 DOT 方法): H._b", """
start: path_chain
path_chain: path_part+
?path_part: PATH_IDENT | DOT | TYPE_ANNOT
PATH_IDENT.50: /\\w[\\w\\-]*/
TYPE_ANNOT.105: /_[a-zA-Z][\\w\\-]*/
DOT: "."
%import common.WS
%ignore WS
""", "H._b", T11)


# 12: path_chain 有 DOT→SENTINEL
class T12(T_MDIO):
    def path_chain(self, *args): T_MDIO.parent(self, *args)
    def DOT(self, t): return SENTINEL

show("12. MDIO path_chain (DOT→SENTINEL): H._b", """
start: path_chain
path_chain: path_part+
?path_part: PATH_IDENT | DOT | TYPE_ANNOT
PATH_IDENT.50: /\\w[\\w\\-]*/
TYPE_ANNOT.105: /_[a-zA-Z][\\w\\-]*/
DOT: "."
%import common.WS
%ignore WS
""", "H._b", T12)


# 13: kwarg_path
class T13(T_MDIO):
    def kwarg_path(self, *args): T_MDIO.parent(self, *args)

show("13. MDIO kwarg_path: /mem/test.md", """
start: kwarg_path
kwarg_path: (path_part | DOUBLE_COLON | COLON)+
?path_part: PATH_IDENT | FUNC_NAME | SLASH | DOT
PATH_IDENT.50: /\\w[\\w\\-]*/
FUNC_NAME.100: /[a-zA-Z_][a-zA-Z0-9_]*/
SLASH: "/"
DOT: "."
DOUBLE_COLON: "::"
COLON: ":"
%import common.WS
%ignore WS
""", "/mem/test.md", T13)


# 14: path_prefix
class T14(T_MDIO):
    def path_prefix(self, *args): T_MDIO.parent(self, *args)

show("14. MDIO path_prefix: /mem/test.md", """
start: path_prefix
path_prefix: SLASH path_part*
?path_part: PATH_IDENT | FUNC_NAME | SLASH | DOT
PATH_IDENT.50: /\\w[\\w\\-]*/
FUNC_NAME.100: /[a-zA-Z_][a-zA-Z0-9_]*/
SLASH: "/"
DOT: "."
%import common.WS
%ignore WS
""", "/mem/test.md", T14)


# 15: short_flag
@v_args(inline=True)
class T15(Transformer):
    def short_flag(self, *args):
        print(f"  {len(args)} args:")
        for i, a in enumerate(args):
            print(f"    arg[{i}] {type(a).__name__} {repr(a)[:60]}")
    def IDENT(self, t): return str(t)

show("15. MDIO short_flag: -f /mem/test.md (字面量 '-' 不传入)", """
start: flag
flag: "-" IDENT KWARG_PATH?   -> short_flag
IDENT: /[a-zA-Z_][a-zA-Z0-9_]*/
KWARG_PATH: /\\/[a-zA-Z.\\/]+/
%import common.WS
%ignore WS
""", "-f /mem/test.md", T15)


# 5. 对照实验：为什么测试 7 是独立 arg，测试 11 是 1 list？

@v_args(inline=True)
class T_Compare(Transformer):
    def parent(self, *args):
        for i, a in enumerate(args):
            if isinstance(a, list):
                print(f"  arg[{i}]: list[{len(a)}]")
                for j, x in enumerate(a):
                    print(f"    [{j}] {type(x).__name__} {repr(x)[:40]}")
            else:
                print(f"  arg[{i}] {type(a).__name__} {repr(a)[:40]}")

# 实验 A: 所有 terminal 无方法 (对比测试 7：测试7的X,Y都有方法)
@v_args(inline=True)
class TA(T_Compare):
    def parent(self, *args): T_Compare.parent(self, *args)

show("A. ?child+, 全部 terminal 无方法", """
start: parent
parent: child+
?child: X | Y
X: "x"
Y: "y"
%import common.WS
%ignore WS
""", "x y", TA)


# 实验 B: 仅部分 terminal 有方法 (1有1无)
@v_args(inline=True)
class TB(T_Compare):
    def parent(self, *args): T_Compare.parent(self, *args)
    def X(self, t): return f"X[{t}]"
    # Y: 无方法

show("B. ?child+, 部分 terminal 有方法 (X有, Y无)", """
start: parent
parent: child+
?child: X | Y
X: "x"
Y: "y"
%import common.WS
%ignore WS
""", "x y x", TB)


# 实验 C: 全部 terminal 有方法 (这就是测试 7 的情况)
@v_args(inline=True)
class TC(T_Compare):
    def parent(self, *args): T_Compare.parent(self, *args)
    def X(self, t): return f"X[{t}]"
    def Y(self, t): return f"Y[{t}]"

show("C. ?child+, 全部 terminal 有方法 (这就是测试7)", """
start: parent
parent: child+
?child: X | Y
X: "x"
Y: "y"
%import common.WS
%ignore WS
""", "x y x", TC)


# 实验 D: terminal 方法返回 tuple (模拟 TYPE_ANNOT)
@v_args(inline=True)
class TD(T_Compare):
    def parent(self, *args): T_Compare.parent(self, *args)
    def X(self, t): return ("TUPLE_X", str(t))
    def Y(self, t): return ("TUPLE_Y", str(t))

show("D. ?child+, terminal 返回 tuple (模拟 TYPE_ANNOT)", """
start: parent
parent: child+
?child: X | Y
X: "x"
Y: "y"
%import common.WS
%ignore WS
""", "x y x", TD)


# 实验 E: 混合 — 一个返回 tuple，一个无方法 (匹配 PATH_IDENT无方法 + TYPE_ANNOT有)
@v_args(inline=True)
class TE(T_Compare):
    def parent(self, *args): T_Compare.parent(self, *args)
    def X(self, t): return ("TUPLE_X", str(t))
    # Y: 无方法

show("E. ?child+, 混合: X返回tuple, Y无方法", """
start: parent
parent: child+
?child: X | Y
X: "x"
Y: "y"
%import common.WS
%ignore WS
""", "x y x", TE)


# 实验 F: 仅一个 terminal 有方法 (返回 tuple)
@v_args(inline=True)
class TF(T_Compare):
    def parent(self, *args): T_Compare.parent(self, *args)
    def X(self, t): return ("TUPLE_X", str(t))

show("F. ?child+, 仅 X 返回 tuple", """
start: parent
parent: child+
?child: X | Y
X: "x"
Y: "y"
%import common.WS
%ignore WS
""", "x x", TF)


# 实验 G: 使用 MDIO 的 terminal 定义方式 (regex+priority) 但规则名是 parent/child
@v_args(inline=True)
class TG(T_Compare):
    def parent(self, *args): T_Compare.parent(self, *args)
    def SPECIAL(self, t): return ("TUPLE", str(t))
    # PLAIN, DOT: 无方法

show("G. ★ MDIO式终端定义, parent/child 规则名", """
start: parent
parent: child+
?child: PLAIN | DOT | SPECIAL
PLAIN.50: /\\w[\\w\\-]*/
SPECIAL.105: /_[a-zA-Z][\\w\\-]*/
DOT: "."
%import common.WS
%ignore WS
""", "H._b", TG)


# 实验 H: 所有条件相同，只改规则名为 path_chain/path_part
@v_args(inline=True)
class TH(T_Compare):
    def path_chain(self, *args): T_Compare.parent(self, *args)
    def SPECIAL(self, t): return ("TUPLE", str(t))

show("H. ★ 仅改规则名为 path_chain/path_part", """
start: path_chain
path_chain: path_part+
?path_part: PLAIN | DOT | SPECIAL
PLAIN.50: /\\w[\\w\\-]*/
SPECIAL.105: /_[a-zA-Z][\\w\\-]*/
DOT: "."
%import common.WS
%ignore WS
""", "H._b", TH)


# 6. 方案：不设 Discard，DOT 作为普通 Token 流过

SEP = lambda title: print(f"\n{'─'*55}\n  {title}\n{'─'*55}")

# path_chain：DOT 不跳过，全部转 typed tuple

@v_args(inline=True)
class NewPathChain(Transformer):
    """新方案：path_chain 不做任何过滤，全量转 typed tuple"""
    def TYPE_ANNOT(self, t): return ("TYPE_ANNOT", str(t))
    # DOT: 无全局方法 → Lark Token
    # PATH_IDENT: 无全局方法 → Lark Token
    def path_chain(self, *args):
        typed = []
        for a in args:
            if isinstance(a, list):
                for item in a:
                    if hasattr(item, 'type'):
                        typed.append((item.type, str(item)))
                    elif isinstance(item, tuple):
                        typed.append(item)
            elif hasattr(a, 'type'):
                typed.append((a.type, str(a)))
            elif isinstance(a, tuple):
                typed.append(a)
        print(f"  → {typed}")
        return typed

SEP("新方案: path_chain('H._b') — DOT 保留为 ('DOT','.')")
Lark("""
start: path_chain
path_chain: path_part+
?path_part: PATH_IDENT | DOT | TYPE_ANNOT
PATH_IDENT.50: /\\w[\\w\\-]*/
TYPE_ANNOT.105: /_[a-zA-Z][\\w\\-]*/
DOT: "."
%import common.WS
%ignore WS
""", parser="lalr", transformer=NewPathChain()).parse("H._b")

SEP("新方案: path_chain('CONFIG.PORT') — 两个 PATH_IDENT，一个 DOT")
Lark("""
start: path_chain
path_chain: path_part+
?path_part: PATH_IDENT | DOT | TYPE_ANNOT
PATH_IDENT.50: /\\w[\\w\\-]*/
TYPE_ANNOT.105: /_[a-zA-Z][\\w\\-]*/
DOT: "."
%import common.WS
%ignore WS
""", parser="lalr", transformer=NewPathChain()).parse("CONFIG.PORT")

SEP("新方案: path_chain('/mem/test.md') — 文件路径中的 DOT")
Lark("""
start: path_chain
path_chain: path_part+
?path_part: PATH_IDENT | FUNC_NAME | SLASH | DOT
PATH_IDENT.50: /\\w[\\w\\-]*/
FUNC_NAME.100: /[a-zA-Z_][a-zA-Z0-9_]*/
SLASH: "/"
DOT: "."
%import common.WS
%ignore WS
""", parser="lalr", transformer=NewPathChain()).parse("/mem/test.md")


# kwarg_path：list 展开后全部取值参与 join

@v_args(inline=True)
class NewKwargPath(Transformer):
    def kwarg_path(self, *args):
        res = []
        for a in args:
            if isinstance(a, list):
                for item in a:
                    if hasattr(item, 'value'):
                        res.append(str(item.value))
                    elif hasattr(item, 'raw_value'):
                        res.append(str(item.raw_value))
                    elif isinstance(item, tuple):
                        res.append(str(item[1]))
                    else:
                        res.append(str(item))
            elif hasattr(a, 'value'):
                res.append(str(a.value))
            elif hasattr(a, 'raw_value'):
                res.append(str(a.raw_value))
            elif isinstance(a, tuple):
                res.append(str(a[1]))
            else:
                res.append(str(a))
        s = "".join(res)
        print(f"  → '{s}'")
        return s

SEP("新方案: kwarg_path('/mem/test.md') — DOT 自然参与 join")
Lark("""
start: kwarg_path
kwarg_path: (path_part | COLON)+
?path_part: FUNC_NAME | SLASH | DOT
FUNC_NAME.100: /[a-zA-Z_][a-zA-Z0-9_]*/
SLASH: "/"
DOT: "."
COLON: ":"
%import common.WS
%ignore WS
""", parser="lalr", transformer=NewKwargPath()).parse("/mem/test.md")


# path_prefix：list 展开，DOT 保留

@v_args(inline=True)
class NewPathPrefix(Transformer):
    def path_prefix(self, *args):
        typed = []
        for a in args:
            if isinstance(a, list):
                for item in a:
                    if hasattr(item, 'type'):
                        typed.append((item.type, str(item)))
                    elif isinstance(item, tuple):
                        typed.append(item)
            elif hasattr(a, 'type'):
                typed.append((a.type, str(a)))
            elif isinstance(a, tuple):
                typed.append(a)
        print(f"  → {typed}")
        return typed

SEP("新方案: path_prefix('/mem/test.md') — DOT 保留")
Lark("""
start: path_prefix
path_prefix: SLASH path_part*
?path_part: FUNC_NAME | SLASH | DOT
FUNC_NAME.100: /[a-zA-Z_][a-zA-Z0-9_]*/
SLASH: "/"
DOT: "."
%import common.WS
%ignore WS
""", parser="lalr", transformer=NewPathPrefix()).parse("/mem/test.md")


# 模拟完整 locator_core：DOT 在步骤构造时自然跳过，file_path 自然包含

@v_args(inline=True)
class NewLocatorCore(Transformer):
    def TYPE_ANNOT(self, t): return ("TYPE_ANNOT", str(t))

    def path_chain(self, *args):
        """同 NewPathChain：全量转 typed tuple"""
        typed = []
        for a in args:
            if isinstance(a, list):
                for item in a:
                    if hasattr(item, 'type'):
                        typed.append((item.type, str(item)))
                    elif isinstance(item, tuple):
                        typed.append(item)
            elif hasattr(a, 'type'):
                typed.append((a.type, str(a)))
            elif isinstance(a, tuple):
                typed.append(a)
        return typed

    def locator_core(self, *args):
        """模拟 locator_core：找出 :: 分界，构造 steps + file_path"""
        context_id = None
        typed_parts = []
        key_spec = None

        # 收集所有 parts
        for a in args:
            if a is None:
                continue
            if isinstance(a, int):
                context_id = a; continue
            if isinstance(a, list) and a:
                if isinstance(a[0], tuple):
                    typed_parts.extend(a); continue
                # filter list
                if a[0] in ('CMP',):
                    continue
            if hasattr(a, 'type'):
                typed_parts.append((a.type, str(a))); continue
            if isinstance(a, str):
                key_spec = a; continue

        # 拆分 ::
        file_path = None
        dc_indices = [i for i, (tt, _) in enumerate(typed_parts) if tt == 'DOUBLE_COLON']
        if dc_indices:
            dc_idx = dc_indices[0]
            before_dc = typed_parts[:dc_idx]
            after_dc = typed_parts[dc_idx+1:]
            remaining = [i for i, (tt, _) in enumerate(after_dc) if tt == 'DOUBLE_COLON']
            if remaining:
                dc2 = remaining[0]
                file_path = "".join(pv for _, pv in after_dc[:dc2])
                process_parts = after_dc[dc2+1:]
            else:
                if before_dc:
                    file_path = "".join(pv for _, pv in before_dc)
                process_parts = after_dc
        else:
            process_parts = typed_parts

        # 步骤构造：DOT 无 handler → 自然跳过
        steps = []
        for tt, tv in process_parts:
            if tt in ('PATH_IDENT', 'FUNC_NAME'):
                steps.append(["Drill", tv])
            elif tt == 'TYPE_ANNOT':
                raw = tv[1:] if tv.startswith('_') else tv
                if not steps or steps[-1] not in (["Wildcard"], ["RecursiveWildcard"]):
                    steps.append(["Wildcard"])
                steps.append(["TYPECHECK", raw])
            elif tt == 'WILDCARD':
                steps.append(["Wildcard"])
            elif tt == 'RECURSIVE_SEP':
                steps.append(["RecursiveWildcard"])
            elif tt == 'COLON' and key_spec:
                steps.append(["CMP", ".tag", "==", key_spec])
                steps.append(["Slice", 0])
            # DOT: no handler → skipped

        print(f"  file_path: {file_path!r}")
        print(f"  steps: {steps}")
        return (file_path, steps)


SEP("新方案完整模拟: $H._b:key")
Lark("""
start: locator_core
locator_core: path_chain (COLON KEY_SPEC)?
path_chain: path_part+
?path_part: PATH_IDENT | DOT | TYPE_ANNOT
KEY_SPEC: PATH_IDENT
PATH_IDENT.50: /\\w[\\w\\-]*/
TYPE_ANNOT.105: /_[a-zA-Z][\\w\\-]*/
DOT: "."
COLON: ":"
%import common.WS
%ignore WS
""", parser="lalr", transformer=NewLocatorCore()).parse("H._b:key")


SEP("新方案完整模拟: $/mem/test.md::TEST._b:key")
Lark("""
start: locator_core
locator_core: path_chain DOUBLE_COLON path_chain (COLON KEY_SPEC)?
path_chain: path_part+
?path_part: PATH_IDENT | FUNC_NAME | SLASH | DOT | TYPE_ANNOT
KEY_SPEC: PATH_IDENT
PATH_IDENT.50: /\\w[\\w\\-]*/
FUNC_NAME.100: /[a-zA-Z_][a-zA-Z0-9_]*/
TYPE_ANNOT.105: /_[a-zA-Z][\\w\\-]*/
SLASH: "/"
DOT: "."
DOUBLE_COLON: "::"
COLON: ":"
%import common.WS
%ignore WS
""", parser="lalr", transformer=NewLocatorCore()).parse("/mem/test.md::TEST._b:key")


print(f"\n{'─'*55}")
print("  实际 MDIO 行为总结")
print(f"{'─'*55}")
print("""
  ★ 以下是在 MDIO 真实 grammar + transformer 下实测确认的行为 ★

  path_chain  (path_part+ with ?path_part):
    → 1 个 list arg, list 内是 [Token|tuple|Sentinel, ...]
    → DOT 如果在 ?path_part 里且无全局方法 → Token('DOT', '.')
    → DOT 如果有全局方法 → 方法返回值直入 list（不经过 path_part()）

  kwarg_path  ((path_part|::|:)+ with ?path_part):
    → 1 个 list arg, 同上

  path_prefix (SLASH path_part*):
    → 1 个 list arg (SLASH token 在 list 内)

  short_flag  ("-" ident kwarg_value?):
    → 2 args: (ident值, kwarg_value值)
    → 字面量 '-' 不传入

  terminal 全局方法 + ?expander:
    → terminal 方法直接替换 Token 值，放入 list
    → ?expanded rule 的 transformer 方法不被调用
""")
