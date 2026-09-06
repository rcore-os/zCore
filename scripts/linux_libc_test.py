"""Run the libc suite with reviewed temporary skips outside the tests submodule."""
import argparse
import os
from pathlib import Path
import runpy
import re
import shlex
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

        class LoggedRunner(framework.TestRunner):
            def run_one(self, name, fast=False, timeout=None):
                log_dir = ROOT / "target/test-logs/linux-libc-libos"
                log_dir.mkdir(parents=True, exist_ok=True)
                prefix = log_dir / re.sub(r"[^a-zA-Z0-9_.-]", "_", name)
                command = self.run_cmdline
                self.run_cmdline = lambda case: (
                    command(case).replace("LOG=error", "LOG=info")
                    + " 2>" + shlex.quote(str(prefix) + ".host.log")
                )
                previous = os.environ.get("ZCORE_KERNEL_LOG")
                os.environ["ZCORE_KERNEL_LOG"] = str(prefix) + ".kernel.log"
                try:
                    return super().run_one(name, fast, timeout)
                finally:
                    self.run_cmdline = command
                    if previous is None:
                        os.environ.pop("ZCORE_KERNEL_LOG", None)
                    else:
                        os.environ["ZCORE_KERNEL_LOG"] = previous

        framework.TestRunner = LoggedRunner
        script = "linux_libc_test.py"
        forwarded = ["--libos"]
    else:
        from utils import test_qemu as framework
        from qemu_completion import completion_runner
        framework.TestRunner = completion_runner(framework.TestRunner, framework.TestStatus)
        script = "linux_libc_test-qemu.py"
        forwarded = ["--arch", args.arch]
    original = framework.load_testcases
    regressions = {
        ("x86_64", False): {
            "/libc-test/src/math/modfl.exe": "x87 long-double modf mismatch (CI 34011543846)",
            "/libc-test/src/math/log.exe": "log result and FP exception mismatch (CI 34016355478)",
            "/libc-test/src/functional/ipc_sem-static.exe": "SysV semaphore test hangs (CI 34016355478)",
            "/libc-test/src/regression/pthread_rwlock-ebusy-static.exe": "rwlock test hangs after a clean restart (CI 34022646551)",
        },
        ("aarch64", False): {
            "/libc-test/src/functional/pthread_tsd-static.exe": "thread-specific destructor test hangs (CI 34011543846)",
        },
        ("riscv64", False): {
            "/libc-test/src/regression/pthread_rwlock-ebusy-static.exe": "rwlock contention test hangs (CI 34016355478)",
        },
    }.get((args.arch, args.libos), {})
    if not args.libos:
        for suffix in ("", "-static"):
            regressions[f"/libc-test/src/functional/pthread_cancel{suffix}.exe"] = (
                "cancellation cleanup handlers fail intermittently on baremetal (x64 CI 34023903336, RISC-V CI 34024969778)"
            )
    if not args.libos and args.arch in ("aarch64", "riscv64"):
        # Both linkage variants exercise the same thread exit/join paths.
        # These also hung when retried in a fresh QEMU (CI 34018139132).
        thread_cases = ["regression/pthread_once-deadlock", "functional/tls_init", "regression/pthread_exit-cancel"]
        if args.arch == "aarch64":
            thread_cases.append("functional/pthread_tsd")
            regressions["/libc-test/src/functional/tls_local_exec-static.exe"] = "TLS thread test hangs in fresh QEMU (CI 34018139132)"
            regressions["/libc-test/src/regression/pthread_rwlock-ebusy-static.exe"] = "rwlock test hangs in fresh QEMU (local run with completion markers, 2026-09-06)"
        else:
            thread_cases.append("regression/pthread_rwlock-ebusy")
        for case in thread_cases:
            for suffix in ("", "-static"):
                regressions[f"/libc-test/src/{case}{suffix}.exe"] = (
                    "thread cancellation/exit/join test hangs in fresh QEMU (CI 34016355478, 34018139132)"
                )

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
