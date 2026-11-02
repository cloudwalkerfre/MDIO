"""Runtime detection helpers.

Detects the current Python's capabilities (free-threading, JIT) and allows
callers to build the best-possible subprocess command line without hardcoding
flags that don't exist on older interpreters.
"""

import sys


def build_python_cmd(*args: str) -> list[str]:
    """Build [sys.executable, perf_flags..., *args].

    Inserts -X flags automatically when the current runtime supports them:
    - -X jit on 3.14+
    """
    cmd = [sys.executable]

    if sys.version_info >= (3, 14):
        cmd.extend(["-X", "jit"])

    cmd.extend(args)
    return cmd


def get_python_perf_env() -> dict[str, str]:
    """Extra environment variables for optimal runtime performance."""
    env: dict[str, str] = {}
    if sys.version_info >= (3, 14):
        env["PYTHON_JIT"] = "1"
    return env
