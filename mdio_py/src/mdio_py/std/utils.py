# from typing import List

def format_tree_node(node: dict, prefix: str = "", is_last: bool = True, is_root: bool = False):
    lines = []
    connector = "└── " if is_last else "├── "
    child_prefix = prefix + ("    " if is_last else "│   ")
    if is_root: connector = ""; child_prefix = prefix + "    " 

    name = node.get('name', 'Unknown')
    ntype = node.get('node_type', '')

    stats = ""
    if ntype in ('file', 'head', 'root', 'selection'):
        if ntype == 'file': name = f"📄 {name}"
        stats_list = [f"chars: {node.get('chars', 0)}", f"tokens: {node.get('tokens', 0)}"]
        if node.get('lines', 0) > 1: stats_list.append(f"lines: {node.get('lines')}")
        stats = f" ({', '.join(stats_list)})"

    if ntype == "root" or ntype == "selection":
        children = node.get('children', [])
        extras = _get_extras(node)
        total = len(children) + (1 if extras else 0)
        for i, child in enumerate(children):
            at_end = (i == total - 1)
            lines.extend(format_tree_node(child, prefix, at_end, False))
        if extras: lines.append(f"{prefix}└── [+ {', '.join(extras)}]")
        return lines

    lines.append(f"{prefix}{connector}{name}{stats}")

    children = node.get('children', [])
    extras = _get_extras(node)
    total_children = len(children) + (1 if extras else 0)

    for i, child in enumerate(children):
        at_end = (i == total_children - 1)
        lines.extend(format_tree_node(child, child_prefix, at_end, False))

    if extras:
        extras_str = f"[+ {', '.join(extras)}]"
        lines.append(f"{child_prefix}└── {extras_str}")

    return lines

def _get_extras(node: dict) -> list:
    extras = []
    if node.get('anon_bullets', 0) > 0: extras.append(f"{node['anon_bullets']} bullets")
    if node.get('paragraphs', 0) > 0: extras.append(f"{node['paragraphs']} paragraphs")
    if node.get('code_blocks', 0) > 0: extras.append(f"{node['code_blocks']} code blocks")
    return extras