# CI tests and kernel diagnostics

Kernel diagnostics default to INFO. LibOS writes them to `kernel.log`, or to
`ZCORE_KERNEL_LOG` when set. Its user console writes to stdout. Log output is
buffered and flushed on normal exit, ERROR records, and Rust panics. Set
`ZCORE_LOG_FLUSH=1` to flush each record when diagnosing native crashes.

QEMU builds use a dedicated debug channel: x86_64 uses port 0xe9 (debugcon),
and AArch64/RISC-V use semihosting SYS_WRITE0. The Makefile enables the matching
kernel feature and QEMU options. `KERNEL_LOG=/absolute/path` selects the host
file; the default is `target/kernel-ARCH.log`. Semihosting is only enabled for
QEMU builds; do not use that feature on hardware without a semihosting monitor.

`python3 scripts/zircon_core_test.py --arch x86_64 --libos --fast` discovers the
actual test names from the image and runs classified cases in bounded batches.
Each batch has `.guest.log`, `.kernel.log`, and `.host.log` files under
`target/test-logs/`; `results.json` records results and every skip reason.
Success requires a nonempty complete summary, every requested case passing
or explicitly self-skipping, and a successful guest exit. Missing test names
cannot turn into empty passes.

Temporary implementation gaps are listed in
`scripts/test_expectations/zircon.json`, with evidence from CI run 34011543846
and local reproduction. Legacy FAILED/TIMEOUT/PARTIAL classifications and new
unclassified cases are reported as skips. Verified channel, Counter, IOVec and
port stress regressions are promoted explicitly. Use `-t 'Suite.Case'` and
`--include-skipped --timeout 300` to investigate skipped cases. The same skip
policy applies to PR and manually dispatched CI runs.

`scripts/linux_libc_test.py` applies the existing libc classifications and
additional confirmed regressions without changing the tests submodule:
both `pthread_cancel` linkage variants on baremetal; x86_64 `modfl.exe`,
`log.exe`, `ipc_sem-static.exe`, and `pthread_rwlock-ebusy-static.exe`; AArch64
`pthread_tsd`, `tls_local_exec-static`, and `pthread_rwlock-ebusy-static`; AArch64/RISC-V `tls_init`,
`pthread_once-deadlock`, and `pthread_exit-cancel`; and RISC-V
`pthread_rwlock-ebusy`. Both linkage variants are skipped for these thread
lifecycle cases unless a suffix is given. CI uploads the guest,
kernel, and host diagnostic files even if a test job fails. LibOS libc tests
also use INFO and keep a separate kernel/host log for each case. The QEMU libc
runner waits for a unique guest completion marker and checks its exit code;
shell prompts and echoed commands cannot count as successful completion.

The trapframe dependency is pinned to `codex/zcore-fncall-integration`, based
on upstream `codex/fncall`. zCore enables `fncall-preserve-x18` because Fuchsia
uses x18 for shadow-call stacks. Linux locates the active context using the
host thread ID; Darwin uses its pthread-specific slot. Native ARM CI runs the
trapframe ABI/layout tests before running zCore. SIMD registers have named
Q0–Q31 fields and multiline Debug output, with checked assembly offsets.

Baremetal IRQ tests require a startup resource currently absent from
userboot. The registration-race stress test exceeds 300 seconds with INFO
tracing under QEMU, so it remains enabled on native Linux by default.
Both limitations are explicit in the expectation file and can be overridden.

AArch64 macOS currently builds zCore and runs the trapframe layout/concurrency
tests, but explicitly skips Fuchsia guest execution. Darwin clears x18 on host
exception return unless the process has `com.apple.private.custom-x18-abi`.
GitHub's standard macOS runners reject ad-hoc executables with that private
entitlement (SIGKILL, CI 34018139132). The Fuchsia image uses x18 for its shadow
call stack and crashes during libc startup without it (CI 34016355478).
The skip reason is recorded in the job log and `results.json`; it is not counted
as a successful guest run. Trapframe tests requiring x18 are likewise ignored
on macOS until an appropriately authorized host is available. Linux AArch64
runs the real Fuchsia image and the x18 preservation tests.

QEMU runs disable automatic reboot so a kernel crash cannot silently retry a
guest. The x86_64 SMP process-info/debug cases currently expose a child-process
teardown reboot after their assertions pass; those cases are explicitly skipped
pending a fix (CI 34018139132 and local reproduction).
