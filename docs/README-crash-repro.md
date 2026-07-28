# Deterministic reproducer for the "intermittent" kernel corruption

The instability that has surfaced as random SIGSEGVs, the `startx`/evdev panic,
Firefox dying, and `^C`-teardown crashes is **one bug**, and it is now
deterministically reproducible in QEMU (no special hardware, no Xorg):

```sh
timeout -s TERM 1 sleep 5
```

Running that once from the Eclipse shell triple-faults the machine within a
second (QEMU exits under `-no-reboot`; on hardware the box resets).

## How it was captured

`qemu-system-x86_64 ... -d int,cpu_reset,guest_errors -D int.log` records the
CPU exception cascade. The tail before the reset shows control flow jumping to
**corrupted / non-code addresses**, then a double- then triple-fault:

```
v=0d cpl=0 IP=01ffff000003ebf0          # top byte ff -> 01 : corrupted code pointer
v=0e cpl=0 IP=00006578652f666c CR2=…    # IP is ASCII: bytes decode to "lf/exe"
                                        #   = the tail of the string "self/exe"
                                        #   (i.e. "/proc/self/exe")
v=0d cpl=0 IP=ffffff000003ad52          # lands in the scheduler deadlock-spin
check_exception old:0xd new 0xd -> v=08 (double fault) -> Triple fault
```

Consistent tells across runs: `R10=0x5555555555555555`, `R11=0x3333333333333333`,
`R9=0xffffffffffffffff`, and the SAME kernel-stack `RBP=ffffff00016ffbf8`.

## What it is (and isn't)

- **It is kernel memory corruption**: a path-like string (the bytes spell the
  tail of `/proc/self/exe`) overwrites a **return address / code pointer on the
  kernel stack**. A subsequent `ret`/`call` jumps into the string (or a
  byte-mangled address), faulting; the fault delivery itself then faults
  (the stack/pointer is already trashed), escalating double -> triple.
- **Not** an SMP work-stealing race — it reproduces identically at `-smp 1`.
- **Not** the itimer/timer-IRQ lock path — the wake path (`WakerRef::wake_by_ref`)
  is a lock-free atomic bit + reschedule IPI; `lock_linux` is IRQ-disabling.
  Those were ruled out.
- **Not** the earlier `rbp` physmap bug (that one is fixed and looks different).

The `timeout` lifecycle (fork + `setitimer`/alarm(SIGALRM) + `wait4`, then
`SIGTERM` to the child) is what reliably trips it, but the corruption is a
buffer overflow of a path string over kernel-stack state, most likely around
`/proc/self/exe` handling / `execute_path` storage / the exec or readlink path.

## Repro harness

`session.py` (in the scratchpad, not committed) drives a headless QEMU entirely
within one foreground call: launches qemu as a child, waits for the shell over a
`logfile=` serial chardev, injects commands, and screendumps via QMP. Set
`QEMU_DINT=1` to add `-d int` logging, `QEMU_SMP=1` to force a single CPU.

## Refined localization (from the `-d int` trace)

Resolving the pre-crash kernel IPs against the zcore ELF pins the context:

- The corrupt code pointer `0x01ffff000003ebf0` is a saved **`run_executor`**
  return address (`0xffffff000003ebf0`) with its **top byte overwritten**
  `0xff -> 0x01` — an off-by-one / overflow spilling into the adjacent qword.
- A second clobbered slot holds the ASCII of `.../self/exe`.
- Code running immediately before: `lookup_inode_at` (path lookup),
  `sys_clock_nanosleep`, `fork_from`, and the syscall-dispatch closure + its
  `drop_in_place`. This is the **exec/path-lookup path** of the timeout->sleep
  fork/exec, not an idle bug.
- **Not stack exhaustion**: at the fault RSP=`0x16ffac0` with the stack top at
  `~0x1700000` — only ~1.3 KB used. So a **fixed-size stack buffer in an outer
  frame** (RBP `0x16ffbf8`, ~1 KB from the top) overflows a path/string over the
  saved return addresses above it.

Prime suspects on that path:
- `INodeExt::read_as_vmo` (linux-object/src/fs/mod.rs) uses a **16 KB stack
  buffer** (`let mut buf = [0u8; 16384]`) while loading an ELF during exec — a
  very large frame to run on the async executor's per-CPU stack.
- The `/proc/self/exe` magic-link + symlink-follow recursion in
  `lookup_inode_at` / `lookup_follow`.

## Tooling constraint

**GDB against the QEMU gdbstub does NOT work in this environment** — qemu is
killed at exec whenever `-s` or `-gdb` (TCP *or* unix socket) is present, even
with the Bash sandbox disabled (a lower-level container restriction). So the
fix must be found by **kernel instrumentation + `-d int`**, not a live debugger.

## Next step

Add a stack canary / return-address validator around the exec+lookup path (or
just move the 16 KB `read_as_vmo` buffer off the stack and re-test the repro),
rebuild, and re-run `timeout -s TERM 1 sleep 5` under `-d int` to confirm.
