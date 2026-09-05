# Core tests and x64 SMP — 2026-09-06

Scope: AArch64 bare metal, RISC-V bare metal, x64 bare metal, and x64 Linux
LibOS. AArch64 LibOS is excluded. This is a regression subset, not a claim
that the full Zircon core suite passes.

## Changes

- Replace `x86-smpboot` with a kernel-owned INIT/SIPI trampoline. Enumerate
  enabled processors through ACPI, use private AP stacks, reserve the startup
  page, and wait for each AP to finish initialization. Use MSR ICR writes in
  x2APIC mode and MMIO ICR writes in xAPIC mode.
- Normalize the BSP/AP GDT, allocate per-CPU APIC driver state, and initialize
  every local timer. Empty AP queues no longer shut down the test machine.
  x64 now defaults to four CPUs; the vDSO reports the started CPU count.
- Claim scheduler tasks under the collection lock and preserve notifications
  received while another CPU polls the task. Keep round-robin selection across
  waker pages and retain preempted executors even when the local queue is empty.
- Complete synchronous cross-CPU TLB invalidation. Page-table callers hold
  IRQ-disabled locks, so the handler uses NMI delivery, a dedicated per-CPU IST
  stack, and a lock-free sequence acknowledgment. Using the interrupted stack
  is unsafe inside the syscall trampoline, where RSP can point into a saved
  user context. The handler flushes global entries too and does not use GS.
- Serialize stream append offset selection, writes, and content-size publication
  across all streams sharing a VMO. Per-stream cursor locks alone allowed two
  appends to overwrite the same byte.
- Implement port observer cancellation by source/key or key, timestamp and edge
  options, and weak observer ownership. Re-arm signal waits after another waiter
  consumes a notification.
- Enable extended register state for secondary Zircon threads and enable RISC-V
  FS/VS before restoring it. Select the hosted vDSO syscall clock alternatives
  so LibOS deadlines and timestamps share the kernel's nanosecond clock.

## Validation

Environment: Rust nightly-2026-09-01; Fuchsia QEMU 11.0.2 with TCG (no KVM).
x64 uses Haswell with SMAP and FSGSBASE. xAPIC is selected with `-x2apic`.

| Target | CPUs | IPC/stream/timer/port group |
| --- | ---: | --- |
| x64 bare metal, x2APIC | 4 | 248 / 256 pass; completes |
| x64 bare metal, xAPIC | 2 | 248 / 256 pass; completes |
| AArch64 bare metal | 1 | 248 / 256 pass; completes |
| RISC-V bare metal | 1 | 248 / 256 pass; completes |
| x64 Linux LibOS | host runtime | 248 / 256 pass; completes |

The same eight cases fail on the four target platforms:

```text
StreamTestCase.ReadShrinkRace
StreamTestCase.WriteShrinkRace
StreamTestCase.ReadWriteShrinkRace
StreamTestCase.ContentSizeUpdatedOnPartialWrite
StreamTestCase.PartialVmoDirty
StreamTestCase.AppendSuppliesZeroes
StreamTestCase.NoStreamFromContiguousOrPhysicalVmo
StreamTestCase.FaultBeyondStreamSizeResizeDownRace
```

These cover remaining pager/VMO/stream capabilities. All 37 selected port
functional cases pass. The group deliberately does not represent all PortTest
cases: policy-exception and observer-limit cases remain unsupported.

The six `port-stress` cases pass on 4-core x2APIC, 2-core xAPIC, and x64 LibOS.
They include registration/cancellation races, shared-key cancellation, concurrent
port closure, and destructor re-entry. `StreamTestCase.AppendWithMultipleThreads`,
`Threads.ThreadLocalRegisterState`, `Vmar.ProtectTest`, and
`Vmar.ConcurrentUnmapReadMemory` also pass with four CPUs.

Additional VMAR checks found failures in `ProtectMultipleTest`,
`PartialUnmapAndRead`, and `PartialUnmapAndWrite`. All three reproduce with one
CPU; they concern protect-range and unmapped-memory error semantics. They are
not included in the 256-case table.

Object unit tests: **84 passed, 1 ignored**, using `libos,aspace-separate`.
Without `aspace-separate`, parallel unit tests create overlapping hosted address
spaces and the COW mapping test fails. Scheduler unit tests: **3 passed**,
covering wakes during polling, task completion, and selection across three waker
pages. x64 bare-metal Clippy passes; the toolchain emits a future-compatibility
notice for its own `core` crate. All four kernel targets build.

Logs and machine-readable results are under `target/core-tests-20260905/`:

- `verified-smp4-ipc-port-x86_64.{log,json}`
- `verified-smp2-ipc-port-x86_64.{log,json}`
- `verified-ipc-port-{aarch64,riscv64,libos}.{log,json}`
- `smp4-ist-stress-x86_64.{log,json}` and `smp2-xapic-ist-x86_64.{log,json}`
- `smp4-streamfix-x86_64.{log,json}` and `vm-baseline-smp1-x86_64.{log,json}`
- `unit-object-isolated.log`, `unit-executor-final.log`, `clippy-x86_64.log`
- `runner-smp4-stress-final.log` (6/6) and `runner-aarch64-smoke.log` (1/1),
  validating the checked-in runner with read-only FAT boot disks

## Reproduction

Run from the repository root after fetching the prebuilt Zircon images and
initializing the `tests` and `rboot` submodules. Install `tests/requirements.txt`
and QEMU for the selected architecture. RISC-V's Makefile also requires
`cargo-binutils` or an `OBJCOPY` override.

```sh
python3 scripts/zircon_core_test.py --arch x86_64 --smp 4 --group port-stress
python3 scripts/zircon_core_test.py --arch x86_64 --smp 2 \
  --x64-cpu 'Haswell,+smap,-check,+fsgsbase,-x2apic' --group port-stress
python3 scripts/zircon_core_test.py --arch x86_64 --smp 4 --group ipc-port
python3 scripts/zircon_core_test.py --arch aarch64 --group ipc-port
python3 scripts/zircon_core_test.py --arch riscv64 --group ipc-port
python3 scripts/zircon_core_test.py --arch x86_64 --libos --group ipc-port

cargo test -p zircon-object --lib --features libos,aspace-separate
cargo test --manifest-path vendor/preemptive-scheduler/Cargo.toml --lib
make -C zCore ARCH=x86_64 TEST=1 ZBI=core-tests clippy
```

`--qemu /path/to/qemu-system-ARCH` selects an alternative QEMU build;
`--timeout SECONDS` changes the 90-second timeout per boot. `--skip-build`
reuses an already built and packaged kernel. The runner uses a read-only boot
disk, which also works with QEMU builds lacking the writable-FAT qcow backend.
`-t` accepts a
comma-separated positive filter. A group runs every matching case regardless of
the older per-test classifications. `ipc-port` returns failure while the eight
cases above remain unresolved. Success requires a nonempty, complete test
summary, every selected test passing, and a successful guest exit; QEMU's exit
code alone is insufficient.

The kernel supports up to eight CPU/APIC slots (IDs 0–7), with 2 and 4 tested.
The trampoline uses rboot's low identity mapping and CR3 below 4 GiB. Hardware,
CPU hotplug, arbitrary sparse APIC IDs, and AArch64 LibOS were not validated.

For this machine's installed stable toolchain, the bootloader was prepared with
`RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu make -C rboot build` because the
unqualified `stable` refresh tried an unavailable mirror. Packaging can reuse
that bootloader with
`make -C zCore -o bootloader ARCH=x86_64 TEST=1 ZBI=core-tests build`, followed by
the runner's `--skip-build` option.
