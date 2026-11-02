"""text_utils —— 文本/路径/参数微原语（这些微原语的唯一出处）。

纯函数、零依赖、零 mdio 导入——任何层都可安全 import（无循环）。
"""

import ast
from typing import Callable, Optional


def str_or_empty(v, default: str = "") -> str:
    """str(v).strip()；None ⇒ default（None 判定的条件式唯一实现）。"""
    if v is None:
        return default
    return str(v).strip()


def str_or(v, default: str = "") -> str:
    """str(v).strip()；falsy ⇒ default（falsy 判定的条件式唯一实现）。"""
    return str(v).strip() if v else default


def first_stripped(raw) -> str:
    """列表首元素的 strip；空/非列表 ⇒ \"\"（raw[0].strip() 一族的唯一实现）。"""
    if not raw:
        return ""
    return str(raw[0]).strip()


def unquote_plain(val):
    """档位①（xbt/cron/interpreter 使用）：strip + 成对剥 双/单引号，**不解转义**。"""
    if val is None:
        return ""
    s = str(val).strip()
    if (s.startswith('"') and s.endswith('"')) or (s.startswith("'") and s.endswith("'")):
        return s[1:-1]
    return s


def unquote_triple(val):
    """档位②（bash 使用）：strip + 成对剥 三引号/双/单引号，**不解转义**。"""
    if val is None:
        return ""
    s = str(val).strip()
    for q in ('"""', "'''", '"', "'"):
        if s.startswith(q) and s.endswith(q):
            return s[len(q):-len(q)]
    return s


def unquote_no_strip(val):
    """档位③（interpreter _eval_dynamic_path 的 str 分支）：**不 strip**，
    成对剥 双/单引号。路径首尾空格原样保留。"""
    if val is None:
        return ""
    s = str(val)
    if (s.startswith('"') and s.endswith('"')) or (s.startswith("'") and s.endswith("'")):
        return s[1:-1]
    return s


def unquote_eval(val):
    """档位④（llm 的 _safe_unquote）：strip + 成对剥 双/单引号，
    literal_eval 优先（转义正确解码），失败回退裸剥；非 str ⇒ str。"""
    if not isinstance(val, str):
        return str(val)
    s = val.strip()
    if (s.startswith('"') and s.endswith('"')) or (s.startswith("'") and s.endswith("'")):
        try:
            return str(ast.literal_eval(s))
        except Exception:
            return s[1:-1]
    return s


def unquote(val):
    """统一出口（语义同档位④）；新代码用，按需选档的调用点沿用各自档位。"""
    return unquote_eval(val)


def ensure_md_ext(path: str) -> str:
    """末段无点 ⇒ 补 .md（interpreter 四处内联的唯一实现）。"""
    p = str(path)
    if "." not in p.split("/")[-1]:
        return p + ".md"
    return p


def looks_like_file(s: str) -> bool:
    """「是文件路径还是选择器」指纹（search/logic 两处内联的唯一实现）。"""
    t = str(s)
    return "/" in t or t.endswith(".md") or t == "."


def parse_kv_lines(text, *, unquote_value: Optional[Callable[[str], str]] = None) -> dict:
    """md 文本 → 「- key: value」字典（唯一实现；行解析派 10 处的内核）。

    只认 `- ` 开头且含冒号的行；其余行（标题、空行、无冒号旗标行）忽略。
    unquote_value：可选的值后处理（bash 族需要 unquote；其余 None = 原样 strip）。
    """
    out: dict = {}
    for line in str(text).split("\n"):
        line = line.strip()
        if not line.startswith("- ") or ":" not in line[2:]:
            continue
        k, v = line[2:].split(":", 1)
        out[k.strip()] = (unquote_value(v.strip()) if unquote_value else v.strip())
    return out
