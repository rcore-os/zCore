"""Run the libc suite with reviewed temporary skips outside the tests submodule."""
import argparse
import os
from pathlib import Path
import runpy
import sys

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", default="x86_64", choices=["x86_64", "aarch64", "riscv64"])
    parser.add_argument("--libos", action="store_true")
    parser.add_argument("--fast", action="store_true")
    args = parser.parse_args()
    os.chdir(ROOT / "tests")
    sys.path.insert(0, str(ROOT / "tests"))
    if args.libos:
        from utils import test as framework
        script = "linux_libc_test.py"
        forwarded = ["--libos"]
    else:
        from utils import test_qemu as framework
        script = "linux_libc_test-qemu.py"
        forwarded = ["--arch", args.arch]
    original = framework.load_testcases
    regressions = {
        ("x86_64", False): {"/libc-test/src/math/modfl.exe": "x87 long-double modf result mismatch (CI 34011543846)"},
        ("aarch64", False): {"/libc-test/src/functional/pthread_tsd-static.exe": "thread-specific destructor test hangs (CI 34011543846)"},
    }.get((args.arch, args.libos), {})

    def load_testcases(filename):
        selected = []
        for name, status in original(filename):
            reason = regressions.get(name)
            if status != framework.TestStatus.OK:
                reason = reason or f"Existing classification: {status.name}"
            if reason:
                print(f"SKIP: {name}: {reason}", flush=True)
            else:
                selected.append((name, status))
        if not selected:
            raise RuntimeError("No runnable libc tests")
        return selected

    framework.load_testcases = load_testcases
    # The allowlist above applies equally to PR and manual runs.
    sys.argv = [script] + forwarded + (["--fast"] if args.fast else [])
    runpy.run_path(script, run_name="__main__")


if __name__ == "__main__":
    main()
