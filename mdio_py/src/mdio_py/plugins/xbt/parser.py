"""
Pure Python recursive-descent parser for XBT new grammar.

New grammar rules:
  DM:          >> | ?? | ==
  CMD:         {{...}}   (may be multi-line)
  CONDITION:   [...]     (may be multi-line, bracket-depth matched)
  TAG:         #word
  RAW_TEXT:    anything not matching the above (bare NLP)

  line_head:  DM? CONDITION? TAG*
  action_node:  line_head CMD _NL
              | line_head RAW_TEXT _NL
              | line_head _NL CMD _NL
              | line_head _NL RAW_TEXT _NL
              | line_head _NL
              | CMD _NL
  control_node: line_head _NL _INDENT block _DEDENT
  root_stmt:    - CNAME: _NL _INDENT block _DEDENT
  text_stmt:    RAW_TEXT _NL   (inside indented block)
"""

import hashlib
import re



# Phase 1: Preprocessing — merge multi-line constructs into logical lines


def _rstrip_cr(line: str) -> str:
    return line.rstrip('\r')


def preprocess(text: str) -> list:
    """Convert raw XBT text into a list of (indent, content) tuples."""
    if not text.endswith('\n'):
        text += '\n'

    raw_lines = text.split('\n')
    logical_lines = []
    i = 0

    while i < len(raw_lines):
        raw = _rstrip_cr(raw_lines[i])

        if not raw or not raw.strip():
            i += 1
            continue

        expanded = raw.expandtabs(4)
        indent = len(expanded) - len(expanded.lstrip(' '))
        content = expanded.lstrip(' ')

        # Multi-line condition: [ ... without ] on same line
        if '[' in content and ']' not in content:
            merged_parts = [content]
            i += 1
            while i < len(raw_lines):
                nxt = _rstrip_cr(raw_lines[i])
                if not nxt:
                    i += 1
                    continue
                nxt_expanded = nxt.expandtabs(4)
                merged_parts.append(nxt_expanded)
                if ']' in nxt_expanded:
                    i += 1
                    break
                i += 1
            content = '\n'.join(merged_parts)
            logical_lines.append((indent, content))
            continue

        # Multi-line CMD: {{ ... without }} on same line
        if '{{' in content and '}}' not in content:
            merged_parts = [content]
            i += 1
            while i < len(raw_lines):
                nxt = _rstrip_cr(raw_lines[i])
                if not nxt:
                    i += 1
                    continue
                nxt_expanded = nxt.expandtabs(4)
                merged_parts.append(nxt_expanded)
                if '}}' in nxt_expanded:
                    i += 1
                    break
                i += 1
            content = '\n'.join(merged_parts)
            logical_lines.append((indent, content))
            continue

        logical_lines.append((indent, content))
        i += 1

    return logical_lines



# Phase 2: Line tokenization


DM_TOKENS = ('>>', '??', '==')


class TokenizeError(Exception):
    pass


def tokenize_line(content: str) -> dict:
    """Break a single logical line into components."""
    result = {
        'dm': None,
        'condition': None,
        'tags': [],
        'body_type': None,
        'body': None,
        'is_root': False,
        'root_name': None,
    }

    # XBT 自身语法 root_stmt 的**逐行**识别器（纯函数，无引擎句柄）：返回 per-line 的
    # is_root/root_name，丢弃冒号后的尾串。不用整档 to_json_text + bullets_to_kv 的原因：
    # 后者是整档 dict，实测在 `- a-b:` 名、缩进根、嵌套 `- k: v`、重复根名四类上与逐行
    # 识别不同；其中后两类在可解析蓝图上就会改变 root 集合（嵌套 `- k: v` 会被当成 root、
    # 重复根名产出两个 root）⇒ 会改变行为树输入，故 root 识别留在 parse_xbt。
    root_m = re.match(r'- (\w+):(.*)', content)
    if root_m:
        result['is_root'] = True
        result['root_name'] = root_m.group(1)
        return result

    pos = 0

    for dm in DM_TOKENS:
        if content.startswith(dm, pos):
            result['dm'] = dm
            pos += len(dm)
            if pos < len(content) and content[pos] == ' ':
                pos += 1
            break

    if pos < len(content) and content[pos] == '[':
        depth = 1
        end = pos + 1
        while end < len(content) and depth > 0:
            ch = content[end]
            if ch == '[':
                depth += 1
            elif ch == ']':
                depth -= 1
            end += 1
        if depth != 0:
            raise TokenizeError("Unclosed bracket")
        cond_inner = content[pos + 1:end - 1]
        result['condition'] = cond_inner.strip()
        pos = end
        if pos < len(content) and content[pos] == ' ':
            pos += 1

    while pos < len(content) and content[pos] == '#':
        tag_end = pos + 1
        while tag_end < len(content) and (content[tag_end].isalnum() or content[tag_end] == '_' or content[tag_end] == '.'):
            tag_end += 1
        if tag_end > pos + 1:
            result['tags'].append(content[pos:tag_end])
            pos = tag_end
            if pos < len(content) and content[pos] == ' ':
                pos += 1
        else:
            break

    if pos < len(content):
        body_raw = content[pos:].strip()
        if body_raw:
            if body_raw.startswith('{{') and body_raw.endswith('}}'):
                result['body_type'] = 'cmd'
                result['body'] = body_raw[2:-2].strip()
            else:
                result['body_type'] = 'nlp'
                result['body'] = body_raw

    return result



# Phase 3: Recursive-descent AST builder


def _has_line_head(tok: dict) -> bool:
    return bool(tok['dm'] or tok['condition'] is not None or tok['tags'])


def _make_action(tok: dict) -> dict:
    is_cmd_cond = False
    condition = tok.get('condition')
    if condition and '{{' in str(condition) and '}}' in str(condition):
        stripped = condition.strip()
        if stripped.startswith('{{') and stripped.endswith('}}'):
            condition = stripped[2:-2].strip()
            is_cmd_cond = True

    return {
        'type': 'action',
        'dm': tok.get('dm'),
        'condition': condition,
        'is_cmd_cond': is_cmd_cond,
        'tags': tok.get('tags', []),
        'prompt': tok.get('body'),
        'is_cmd': tok.get('body_type') == 'cmd',
    }


def _make_control(tok: dict, children: list) -> dict:
    is_cmd_cond = False
    condition = tok.get('condition')
    if condition and '{{' in str(condition) and '}}' in str(condition):
        stripped = condition.strip()
        if stripped.startswith('{{') and stripped.endswith('}}'):
            condition = stripped[2:-2].strip()
            is_cmd_cond = True

    return {
        'type': 'control',
        'dm': tok.get('dm'),
        'condition': condition,
        'is_cmd_cond': is_cmd_cond,
        'tags': tok.get('tags', []),
        'children': children,
    }


def _make_root(tok: dict, children: list) -> dict:
    return {
        'type': 'root',
        'name': tok.get('root_name'),
        'children': children,
    }


def _parse_block(tok_lines: list, start_idx: int, parent_indent: int):
    """Parse a block of statements at a given indentation level."""
    nodes = []
    i = start_idx

    while i < len(tok_lines):
        tl = tok_lines[i]

        if tl['indent'] < parent_indent:
            break

        if tl['indent'] > parent_indent:
            raise Exception("UnexpectedToken")

        if tl['is_root']:
            if i + 1 < len(tok_lines) and tok_lines[i + 1]['indent'] > parent_indent:
                children, i = _parse_block(tok_lines, i + 1, tok_lines[i + 1]['indent'])
            else:
                children = []
                i += 1
            nodes.append(_make_root(tl, children))
            continue

        if _has_line_head(tl):
            if tl['body'] is not None:
                if i + 1 < len(tok_lines) and tok_lines[i + 1]['indent'] > tl['indent']:
                    raise Exception("UnexpectedToken")
                nodes.append(_make_action(tl))
                i += 1
            else:
                if i + 1 < len(tok_lines):
                    nxt = tok_lines[i + 1]
                    if nxt['indent'] > tl['indent']:
                        children, i = _parse_block(tok_lines, i + 1, nxt['indent'])
                        nodes.append(_make_control(tl, children))
                    elif nxt['indent'] == tl['indent'] and not _has_line_head(nxt) and nxt['body'] is not None:
                        tl['body'] = nxt['body']
                        tl['body_type'] = nxt['body_type']
                        nodes.append(_make_action(tl))
                        i += 2
                    else:
                        nodes.append(_make_action(tl))
                        i += 1
                else:
                    nodes.append(_make_action(tl))
                    i += 1
            continue

        if tl['body_type'] in ('cmd', 'nlp'):
            if i + 1 < len(tok_lines) and tok_lines[i + 1]['indent'] > tl['indent']:
                raise Exception("UnexpectedToken")
            nodes.append(_make_action(tl))
            i += 1
            continue

        i += 1

    return nodes, i



# Semantic check


def _semantic_check(ast: list) -> list:
    errors = []
    if not ast:
        errors.append("UnexpectedEOF")
        return errors

    def _check_and_enrich(nodes):
        if not nodes:
            return

        dms = [n.get('dm') for n in nodes if n.get('dm')]
        if len(set(dms)) > 1:
            errors.append("同层 DM 不一致")

        for node in nodes:
            cat = node.get('type')

            if cat in ('control', 'root'):
                children = node.get('children', [])
                child_dms = [c.get('dm') for c in children if c.get('dm')]
                unified = child_dms[0] if child_dms else ">>"
                node['node_type'] = unified

                if cat == 'control':
                    if node.get('prompt') or not children:
                        errors.append("UnexpectedToken")
                    if node.get('dm') == '??':
                        for ch in children:
                            if ch.get('condition') is None:
                                errors.append("Selector 子节点必须携带 [条件]")
                                break

                if children:
                    _check_and_enrich(children)

            elif cat == 'action':
                if node.get('children') or node.get('prompt') is None:
                    errors.append("UnexpectedToken")

    _check_and_enrich(ast)
    return errors


def wrap_with_virtual_root(ast: list) -> list:
    if not ast:
        return []
    if len(ast) == 1 and ast[0].get('type') == 'root':
        return ast

    explicit_root_dm = '>>'
    for n in ast:
        if n.get('dm'):
            explicit_root_dm = n['dm']
            break

    return [{
        'type': 'root',
        'dm': explicit_root_dm,
        'node_type': explicit_root_dm,
        'condition': '',
        'children': ast,
    }]



# Cache


_AST_CACHE = {}
_MAX_CACHE_SIZE = 128


def _get_text_hash(text: str) -> str:
    return hashlib.md5(text.encode('utf-8')).hexdigest()



# Public entry point


def parse(text: str) -> list:
    """Parse XBT new-grammar text into AST.  Raises Exception on error."""
    if not text.strip():
        raise Exception("UnexpectedEOF")

    logical_lines = preprocess(text)
    if not logical_lines:
        raise Exception("UnexpectedEOF")

    tok_lines = []
    for indent, content in logical_lines:
        try:
            tok = tokenize_line(content)
        except TokenizeError:
            raise Exception("UnexpectedToken")
        tok['indent'] = indent
        tok_lines.append(tok)

    try:
        ast, _ = _parse_block(tok_lines, 0, 0)
    except Exception:
        raise Exception("UnexpectedToken")

    errors = _semantic_check(ast)
    if errors:
        raise Exception(errors[0])

    ast = wrap_with_virtual_root(ast)
    return ast


def parse_xbt(text: str) -> list:
    """Cached entry point — compatible with old interface."""
    if not text.strip():
        raise Exception("UnexpectedEOF")
    if not text.endswith('\n'):
        text += '\n'

    text_hash = _get_text_hash(text)
    if text_hash in _AST_CACHE:
        return _AST_CACHE[text_hash]

    ast = parse(text)

    if len(_AST_CACHE) >= _MAX_CACHE_SIZE:
        _AST_CACHE.pop(next(iter(_AST_CACHE)))

    _AST_CACHE[text_hash] = ast
    return ast
