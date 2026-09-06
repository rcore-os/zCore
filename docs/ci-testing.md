# CI tests and kernel diagnostics

Kernel diagnostics default to INFO. LibOS writes them to `kernel.log`, or to
`ZCORE_KERNEL_LOG` when set. Its user console writes to stdout. Log output is
buffered and flushed on normal exit, ERROR records, and Rust panics.

QEMU builds use a dedicated debug channel: x86_64 uses port 0xe9 (debugcon),
and AArch64/RISC-V use semihosting SYS_WRITE0. The Makefile enables the matching
kernel feature and QEMU options. `KERNEL_LOG=/absolute/path` selects the host
file; the default is `target/kernel-ARCH.log`. Semihosting is only enabled for
QEMU builds; do not use that feature on hardware without a semihosting monitor.

`python3 scripts/zircon_core_test.py --arch x86_64 --libos --fast` discovers the
actual test names from the image and runs classified cases in bounded batches.
Each batch has `.guest.log`, `.kernel.log`, and `.host.log` files under
`target/test-logs/`; `results.json` records results and every skip reason.
Success requires a nonempty complete summary, every requested case passing,
and a successful guest exit. Missing test names cannot turn into empty passes.

Temporary implementation gaps are listed in
`scripts/test_expectations/zircon.json`, with evidence from CI run 34011543846
and local reproduction. Legacy FAILED/TIMEOUT/PARTIAL classifications and new
unclassified cases are reported as skips. Verified channel, Counter, IOVec and
port stress regressions are promoted explicitly. Use `-t 'Suite.Case'` and
`--include-skipped --timeout 300` to investigate skipped cases. The same skip
policy applies to PR and manually dispatched CI runs.

`scripts/linux_libc_test.py` applies the existing libc classifications and two
additional confirmed regressions without changing the tests submodule:
x86_64 `modfl.exe` and AArch64 `pthread_tsd-static.exe`. CI uploads the guest,
kernel, and host diagnostic files even if a test job fails.

The trapframe dependency is pinned to `codex/zcore-fncall-integration`, based
on upstream `codex/fncall`. zCore enables `fncall-preserve-x18` because Fuchsia
uses x18 for shadow-call stacks. Linux locates the active context using the
host thread ID; Darwin uses its pthread-specific slot. Native ARM CI runs the
trapframe ABI/layout tests before running zCore. SIMD registers have named
Q0–Q31 fields and multiline Debug output, with checked assembly offsets.
