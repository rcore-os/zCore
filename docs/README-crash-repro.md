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

## Mechanism model (smp=1 trace)

Serialized at `-smp 1`, the last kernel functions before the corrupt-IP fault
are: `Syscall::syscall::{closure}` + its `drop_in_place`, `sys_clock_nanosleep`,
then `klog_emit::write_str` (kernel logging). `klog_emit` itself is **safe** —
its 512-byte stack `W` writer clamps `n = len.min(buf.len()-pos)` — it is only
the code that happened to be running when the *already-corrupted* return address
was used.

The corrupted slots sit HIGH on the executor stack (near the `0x1700000` top,
at `run_executor`'s saved return address). A buffer overflow that writes PAST
its end (low->high addresses, the direction `copy_from_slice`/`copy_nonoverlapping`
run) from a frame below `run_executor` reaches up and clobbers the parent
frames' return addresses. So the bug is a **bad-length copy writing upward**
somewhere in the fork/exec + `/proc/self/exe` path — NOT logging, NOT the
scheduler locks, NOT SMP, NOT stack exhaustion.

Ruled out so far: `klog_emit` (bounded), `sys_readlinkat` (heap vec, bounded),
`W` writers in logging.rs (clamped), SMP work-stealing (repros at smp=1), the
itimer/timer-IRQ lock path (wake is a lock-free atomic).

## Canary result: the corruption is in the vfork -> execve(sleep) window

A temporary canary (a per-syscall scan of the executor stack for a kernel code
pointer `0xffffff00_00xxxxxx` whose top byte was mangled, un-mangled low32 still
in `.text`) was added to `handle_user_trap` and run under the repro. It fired
on **`syscall=58` (vfork)** right before the crash, with the exact original
signature (`0x01ffff0000033140` -> a saved return address `0xffffff0000033140`).

Because `vfork_impl` **awaits** (`wait_signal` — the parent blocks until the
child execs/exits), "detected after syscall 58" means the corruption happens
somewhere in the **child's fork -> execve("sleep") window**, not in vfork's own
few instructions. The corruption is also **multi-form and non-deterministic**:
some runs mangle a return-address top byte (`ff -> 01/0a`), others overwrite a
whole slot with ASCII path bytes (`/proc/self/exe`) — so a single narrow
signature does not catch every instance.

Bounds-checked and therefore ruled OUT as the copy site: `sys_readlinkat`
(heap vec), `Pseudo::read_at` (`len = (content.len()-offset).min(buf.len())`),
`klog_emit`'s `W` writer, the logging.rs 1 KiB `W`, `VmAddressRegion::dump`
(gated on `log_enabled!(Info)`, a no-op at `LOG=error`). The bug is a subtler
**computed-length `copy_from_slice`/`copy_nonoverlapping`** somewhere in the
child's fork/exec/ELF-load path that writes past a stack buffer.

Harness (scratchpad, not committed): `session.py` runs one QEMU session per
foreground tool call; `QEMU_DINT=1` adds `-d int`, `QEMU_SMP=1` forces one CPU.
The repro is `timeout -s TERM 1 sleep 5`.

## CORRECTED DECISIVE: the crash needs the itimer to FIRE and deliver a signal

An earlier pass concluded "arming an itimer" was the trigger — that was wrong: it
conflated **hang** with **crash**. Re-running each case with a QMP screendump
after it (crash = QMP/VM gone, hang = screendump still works) shows:

| command | timer fires? | QMP after | verdict |
|---|---|---|---|
| `env true` | — | ok | completes |
| `timeout 5 true` | no (child exits in ms) | **ok** | **HANGS** (shell stuck; separate reaping bug) |
| `sh -c 'true & wait'` | — | **ok** | **HANGS** |
| `timeout -s TERM 1 sleep 5` | **yes** (1 s < 5 s) | **GONE** | **CRASHES** |

So the crash requires the itimer/alarm to **actually fire** and deliver a signal
(here SIGALRM to `timeout`, whose handler then SIGTERMs the blocked `sleep`).
The arm-only cases merely hang. The corruptor is the **timer-IRQ signal-delivery
path** `arm_itimer`'s callback -> `deliver_timer_signal` (runs in `timer_tick`,
i.e. hard IRQ context) -> `thread.lock_linux()` + `thread.signal_set(USER_SIGNAL_0)`
force-waking a task blocked in a syscall. This matches the fatal `-d int`: a `#GP`
on the IRQ delivered right after an `sti` in `lookup_inode_at`, on an
already-corrupted executor stack whose mangled slot is a `timer_tick` return
address — i.e. the timer IRQ's own signal-delivery work is trampling the stack.

(There is ALSO a separate, non-fatal bug: a forked non-top-level process that
`vfork`s a child and `wait`s for it — `timeout 5 true`, `sh -c 'true & wait'` —
hangs the shell. Distinct from the crash; noted here so it isn't confused for it.)

## (superseded) earlier note: the trigger is `setitimer`/`alarm` (arming an itimer)

A bisection in QEMU (repair-canary kernel, `-smp 1`) pinned it precisely. Each
command was run from the shell and the VM's survival checked:

| command | what it does | result |
|---|---|---|
| `/bin/busybox true` | applet run in-process (no child) | **survives** |
| `ls /` | applet in-process | **survives** |
| `env true` | fork + exec chain, **no alarm** | **survives** |
| `sh -c 'exec true'` | fork + exec-replace, **no alarm** | **survives** |
| `timeout 5 true` | `setitimer/alarm` + fork + exec + wait | **CRASHES** |
| `timeout 1 sleep 5` | same + the signal actually fires | **CRASHES** |

So it is NOT vfork/exec, NOT `/proc/self/exe` lookup, NOT fork+wait — every one
of those runs fine on its own. The one thing `timeout` does that the survivors
don't is **arm an interval timer** (`alarm(5)` / `setitimer(ITIMER_REAL)`).
Crucially `timeout 5 true` crashes *immediately* (the child exits in ms, the 5 s
timer never fires) — so it's the **arming**, not the delivery.

Fatal instruction (clean `-d int` of `timeout 5 true`): `#GP` at
`lookup_inode_at+0x919` which is `mov %r14,%rsi` **immediately after an `sti`**
(the interrupt-enable that ends a `lock::Mutex` guard's `pop_off`). A register
move can't fault — the IRQ that fires the instant `sti` re-enables interrupts
does, because the executor **kernel stack is already corrupted** by the earlier
`setitimer`. The repeatedly-corrupted slot resolves to a saved **`timer_tick`**
return address (`0xffffff000006defb`, top byte mangled `ff -> 01`): the timer
subsystem is trampling the executor stack.

Verified clean by inspection: `sys_setitimer` (bounds-checked `slots[which]`,
`itimers: [ItimerSlot; 3]`, `which <= ITIMER_PROF`), the `TimerHeap`
(`BinaryHeap` add/drain), `arm_itimer`'s `Box`'d closure. The corruption is in
the **arm_itimer -> timer_set -> NAIVE_TIMER** interaction with the executor
stack / `timer_tick`, not in the obvious syscall bounds.

## Next step

Instrument `arm_itimer` / `timer_set` / `timer_tick` with an executor-stack
return-address scan (before/after) under `timeout 5 true` to catch the exact
write. GDB is unavailable in this env; on real hardware or an env that allows
the QEMU gdbstub, a hardware watchpoint on the `timer_tick` return-address slot
would name the writer in one shot.
