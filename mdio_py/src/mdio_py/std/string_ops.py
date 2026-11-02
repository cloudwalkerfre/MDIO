import json

async def fmt(workspace, pipe_data, args, kwargs, ctx):
    # print(f"[FMT DEBUG] kwargs={kwargs}, args={args}")
    """
    用法: $DATA | fmt "Name: {}"
    占位: fmt -t "Target: {}" -v _
    """
    # 1. 提取控制参数: 模板 (必须提供，切忌吃掉管道数据)
    template = kwargs.get("-t", args[0] if args else None)
    if template is None:
        return "- mdio_error: [fmt] requires a template string (-t or args[0])"
    
    template = str(template).replace('\\n', '\n')
    
    # 2. 提取数据参数
    target_data = kwargs.get("-v", pipe_data)
    # print(f"[DEBUG fmt] template={template}, target_data={target_data}, type={type(target_data)}")
    if target_data is None:
        target_data = ""

    # 3. 智能插值
    if isinstance(target_data, dict):
        try:
            # 允许 {key} 命名映射
            return template.format(**target_data)
        except KeyError as e:
            # 兼容普通 {} 替换
            return template.replace("{}", json.dumps(target_data, ensure_ascii=False))
    else:
        # 普通变量替换
        return template.replace("{}", str(target_data))

async def split(workspace, pipe_data, args, kwargs, ctx):
    """
    用法: $STR | split ","
    """
    # 如果不传 sep，Python 默认按 whitespace 切分，这非常符合预期
    sep = kwargs.get("-sep", args[0] if args else None)
    target = kwargs.get("-v", pipe_data)
    
    if not isinstance(target, str):
        if target is None: return []
        target = str(target)
        
    return target.split(str(sep)) if sep else target.split()

async def join(workspace, pipe_data, args, kwargs, ctx):
    """
    用法: $LIST | join "\n"
    """
    sep = kwargs.get("-sep", args[0] if args else "\n")
    sep = str(sep).replace('\\n', '\n')
    
    target = kwargs.get("-v", pipe_data)
    
    if isinstance(target, list):
        return sep.join(str(x) for x in target)
    return str(target) if target is not None else ""

async def trim(workspace, pipe_data, args, kwargs, ctx):
    """用法: $STR | trim"""
    target = kwargs.get("-v", pipe_data)
    if target is None: return ""
    return str(target).strip()

async def upper(workspace, pipe_data, args, kwargs, ctx):
    target = kwargs.get("-v", pipe_data)
    return str(target).upper() if target else ""

async def lower(workspace, pipe_data, args, kwargs, ctx):
    target = kwargs.get("-v", pipe_data)
    return str(target).lower() if target else ""
