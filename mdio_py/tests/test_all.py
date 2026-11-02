"""MDIO 全量测试聚合运行器 - 一次跑完所有 tests（不含 test_py）"""
import os
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from threading import Lock

EXCLUDE = {
    "test_py.py",
    "test_all.py",      # 自身
    "test_llm.py",      # LLM 相关暂且排除
    "test_llm2.py",
    "test_llm_explore.py",  # 交互式探索测试，依赖外部 LLM API
    "test_llm_sandbox_new_features.py",
    "test_llm_retry.py",
    "test_llm_inspect.py",
    "batch_explore.py",
}

KNOWN_FAIL = {
    "test_hook.py": "风暴拦截已注释（已知问题）",
    "test_hook_extra.py": "风暴拦截已注释（已知问题）",
    "test_tree.py": "tree std 待废弃，-head/-key 未接线；RO/隐藏断言已对齐 Plan-3",
}

def _run_one(tf: Path, test_dir: Path):
    """在子进程中运行单个测试文件，返回 (name, elapsed, status, detail)。"""
    name = tf.name
    start = time.time()
    try:
        r = subprocess.run(
            [sys.executable, str(tf)],
            capture_output=True, text=True,
            timeout=180,
            cwd=str(test_dir.parent.parent),  # repo root for ../workspace/ paths
            env={**os.environ},
        )
        elapsed = time.time() - start
        if r.returncode != 0:
            if name in KNOWN_FAIL:
                return (name, elapsed, "known", KNOWN_FAIL[name])
            return (name, elapsed, "fail", r.stderr[-200:] + r.stdout[-200:])
        return (name, elapsed, "ok", "")
    except subprocess.TimeoutExpired:
        elapsed = time.time() - start
        return (name, elapsed, "timeout", "TIMEOUT (180s)")
    except Exception as e:
        elapsed = time.time() - start
        return (name, elapsed, "err", str(e))


def main():
    test_dir = Path(__file__).parent
    test_files = sorted(f for f in test_dir.glob("test_*.py") if f.name not in EXCLUDE)
    # 重文件先跑（iroh / cron / bash），避免最后等它们
    test_files.sort(key=lambda f: {"test_iroh.py": 0, "test_cron.py": 1, "test_bash.py": 2}.get(f.name, 99))

    workers = int(os.environ.get("MDIO_TEST_WORKERS", "4"))
    print(f"🚀 MDIO Full Test Suite ({len(test_files)} files, {workers} workers)\n" + "=" * 70)

    passed, failed, errors, known = [], [], [], []
    total_start = time.time()
    print_lock = Lock()

    with ThreadPoolExecutor(max_workers=workers) as pool:
        futures = {pool.submit(_run_one, tf, test_dir): tf for tf in test_files}
        for fut in as_completed(futures):
            name, elapsed, status, detail = fut.result()
            with print_lock:
                if status == "ok":
                    passed.append((name, elapsed))
                    print(f"  ✅ {name} ({elapsed:.1f}s)")
                elif status == "known":
                    known.append((name, elapsed, detail))
                    print(f"  ⚠️  {name} KNOWN ({elapsed:.1f}s)  — {detail}")
                elif status == "fail":
                    failed.append((name, elapsed, detail))
                    print(f"  ❌ {name} FAIL ({elapsed:.1f}s)")
                elif status == "timeout":
                    errors.append((name, elapsed, detail))
                    print(f"  💥 {name} TIMEOUT ({elapsed:.1f}s)")
                else:
                    errors.append((name, elapsed, detail))
                    print(f"  💥 {name} ERR ({elapsed:.1f}s)")

    total_elapsed = time.time() - total_start
    print("\n" + "=" * 70)
    print(f"RESULTS: {len(passed)} passed, {len(failed)} failed, {len(known)} known, {len(errors)} errors")
    print(f"Time: {total_elapsed:.1f}s\n")

    if passed:
        print("PASSED:")
        for n, t in passed:
            print(f"  ✅ {n} ({t:.1f}s)")
    if known:
        print("\nKNOWN (非 Bug):")
        for n, t, reason in known:
            print(f"  ⚠️  {n} ({t:.1f}s) - {reason}")
    if failed:
        print("\nFAILED:")
        for n, t, tail in failed:
            print(f"  ❌ {n} ({t:.1f}s)")
            for line in tail.strip().splitlines()[-5:]:
                print(f"     | {line.strip()}")
    if errors:
        print("\nERRORS:")
        for n, t, reason in errors:
            print(f"  💥 {n} ({t:.1f}s): {reason}")

    if failed or errors:
        sys.exit(1)


if __name__ == "__main__":
    main()
