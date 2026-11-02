import ast
import operator
import random
from mdio_py.core.mdio_ir import PIPE_DATA
from mdio_py.core.text_utils import str_or_empty

# 安全白名单 (Expression Sandbox Limits)

_SAFE_BIN_OPS = {
    ast.Add: operator.add, ast.Sub: operator.sub,
    ast.Mult: operator.mul, ast.Div: operator.truediv,
    ast.FloorDiv: operator.floordiv, ast.Mod: operator.mod
}

_SAFE_UNARY_OPS = {
    ast.USub: operator.neg, ast.UAdd: operator.pos, ast.Not: operator.not_
}

_SAFE_CMPS = {
    ast.Eq: operator.eq, ast.NotEq: operator.ne,
    ast.Lt: operator.lt, ast.LtE: operator.le,
    ast.Gt: operator.gt, ast.GtE: operator.ge
}

_SAFE_FUNCS = {
    'min': min, 'max': max, 'abs': abs, 'round': round,
    'int': int, 'float': float,
    'rand': lambda min_v, max_v: random.randint(int(min_v), int(max_v)),
    'clamp': lambda val, min_v, max_v: max(min_v, min(val, max_v))
}

# 核心表达式求解器 (AST Evaluator)

def _safe_eval_math(expr_str: str, env: dict):
    try:
        node = ast.parse(expr_str, mode='eval')
    except SyntaxError:
        raise ValueError(f"表达式语法错误: {expr_str}")

    def _eval(n):
        if isinstance(n, ast.Expression):
            return _eval(n.body)
            
        elif isinstance(n, ast.Constant):
            # 放行数字与布尔值 (Python 中 bool 是 int 的子类，这里显式写出更严谨)
            if not isinstance(n.value, (int, float, bool)):
                raise ValueError(f"仅允许数字和布尔值，非法字面量: {n.value}")
            return n.value
            
        elif isinstance(n, ast.Name):
            if n.id not in env:
                raise ValueError(f"未定义的变量: '{n.id}'")
            return env[n.id]
            
        # --- 算术运算 ---
        elif isinstance(n, ast.BinOp):
            op_fn = _SAFE_BIN_OPS.get(type(n.op))
            if not op_fn: raise ValueError(f"禁止的二元操作符: {type(n.op).__name__}")
            return op_fn(_eval(n.left), _eval(n.right))
            
        elif isinstance(n, ast.UnaryOp):
            op_fn = _SAFE_UNARY_OPS.get(type(n.op))
            if not op_fn: raise ValueError(f"禁止的一元操作符: {type(n.op).__name__}")
            return op_fn(_eval(n.operand))
            
        # --- 比较运算 (a > b == c) ---
        elif isinstance(n, ast.Compare):
            left = _eval(n.left)
            for op, comparator in zip(n.ops, n.comparators):
                op_fn = _SAFE_CMPS.get(type(op))
                if not op_fn: raise ValueError(f"不支持的比较符: {type(op).__name__}")
                right = _eval(comparator)
                if not op_fn(left, right):
                    return False
                left = right
            return True
            
        # --- 逻辑运算 (and / or) ---
        elif isinstance(n, ast.BoolOp):
            if isinstance(n.op, ast.And):
                return all(_eval(v) for v in n.values)
            elif isinstance(n.op, ast.Or):
                return any(_eval(v) for v in n.values)
            raise ValueError(f"不支持的逻辑操作: {type(n.op).__name__}")
            
        # --- 三元表达式 (A if Cond else B) ---
        elif isinstance(n, ast.IfExp):
            return _eval(n.body) if _eval(n.test) else _eval(n.orelse)
            
        # --- 内部函数调用 ---
        elif isinstance(n, ast.Call):
            if not isinstance(n.func, ast.Name) or n.func.id not in _SAFE_FUNCS:
                raise ValueError(f"调用了非法函数: '{getattr(n.func, 'id', 'unknown')}'")
            func = _SAFE_FUNCS[n.func.id]
            args = [_eval(a) for a in n.args]
            # 捕获函数内部执行异常 (比如 rand(10, 5) 反向区间报错)
            try:
                return func(*args)
            except Exception as e:
                raise ValueError(f"函数 {n.func.id} 运算失败: {e}")
                
        else:
            raise ValueError(f"节点越界拦截: {type(n).__name__}")

    try:
        return _eval(node)
    except ArithmeticError as e:
        raise ValueError(f"算术异常 (如除零/溢出): {e}")

# Operator 接口

async def calc(workspace, pipe_data, args, kwargs, ctx):
    """
    MDIO 安全数学与逻辑计算引擎
    用法: "50" | calc "hp > 0 and mp > 10" -hp _ -mp $SYS:mp
    """
    template = kwargs.get("-e", args[0] if args else None)
    if not template: return "- mdio_error: [calc] 未提供表达式模板"

    # 全局静态常数解析
    static_env = {}
    for key, val in kwargs.items():
        if key.startswith('-') and key != "-e":
            var_name = key.lstrip('-')
            
            if var_name in _SAFE_FUNCS:
                return f"- mdio_error: [calc] '{var_name}' 是系统保留字"
            
            if val != "_" and val is not PIPE_DATA:
                try:
                    str_v = str_or_empty(val, "0")   # 与文本档位一致
                    if not str_v: str_v = "0"
                    # 这里放宽验证，容许传入 "true"/"false" 等字符串被转为 1/0
                    if str_v.lower() == "true": static_env[var_name] = True
                    elif str_v.lower() == "false": static_env[var_name] = False
                    else: static_env[var_name] = float(str_v) if '.' in str_v else int(str_v)
                except ValueError:
                    return f"- mdio_error: [calc] 参数 '{var_name}' 解析失效: '{val}'"

    def _do_calc(single_item):
        local_env = static_env.copy()
        
        pipe_num = 0
        if single_item is not None:
            s_val = str(single_item).strip()
            if s_val:
                try:
                    if s_val.lower() == "true": pipe_num = True
                    elif s_val.lower() == "false": pipe_num = False
                    else: pipe_num = float(s_val) if '.' in s_val else int(s_val)
                except ValueError:
                    # 这样如果是数组广播，10 个怪里有 1 个名字错了，剩下 9 个照样正常扣血并保存，错的那个位置会填入报错文本！
                    return f"- mdio_error: [calc] 管道流入了非法的杂质字符: '{s_val}'"
                    
        for var_name, original_val in kwargs.items():
            if str(original_val) == "_" or original_val is PIPE_DATA:
                local_env[var_name.lstrip('-')] = pipe_num
                
        if "_" not in local_env:
            local_env["_"] = pipe_num

        try:
            result = _safe_eval_math(str(template), local_env)
            
            if isinstance(result, bool):
                return "true" if result else "false"
            if isinstance(result, float) and result.is_integer():
                return str(int(result))
            return str(result)
            
        except ValueError as e:
            return f"- mdio_error: [calc] {str(e)}"

    try:
        if isinstance(pipe_data, list):
            return [_do_calc(item) for item in pipe_data]
        else:
            return _do_calc(pipe_data)
    except Exception as e:
        return f"- mdio_error: [calc] 引擎级断裂: {e}"