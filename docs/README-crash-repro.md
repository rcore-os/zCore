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

## `deliver_timer_signal` RULED OUT (bracketed with a stack scan)

A temporary scan (`dts_scan`, reverted) was placed at the entry of
`deliver_timer_signal` and after `find_process` / `thread_ids` / `signals.insert`
/ `signal_set`. Under the crashing repro it fired **zero** times, yet the VM
still crashed. So the itimer's own IRQ-context delivery is NOT what trashes the
stack — `find_process` + `thread_ids` (alloc) + `signal_set` are all clean.

That leaves the **downstream signal cascade** as the corruptor. Full sequence of
the crashing `timeout -s TERM 1 sleep 5`:
1. 1 s itimer fires -> SIGALRM to `timeout` (deliver_timer_signal — clean).
2. `timeout`'s SIGALRM **handler** is set up (`handle_signal` -> `setup_uspace`
   writes a signal frame) and runs.
3. the handler `kill(SIGTERM)`s the child `sleep`, which is **blocked in
   nanosleep** and still running.
4. `sleep` must be woken and **terminated** while blocked.

The corruption is somewhere in steps 2–4 — the signal-handler frame setup and/or
delivering a terminating signal to a blocked, still-running child. (Plain
`kill -TERM` to a backgrounded blocked `sleep` does NOT crash, so it is the
handler-driven cascade specifically, not a bare SIGTERM.)

## MITIGATION landed + root cause EXPOSED

The corruption triple-faulted *during exception delivery* (the #GP raised on the
corrupt stack could not push its frame, escalating #GP -> #DF -> triple fault,
silently), so nothing was ever visible. Fix: give **#GP (vector 13) its own IST
stack** (mirroring the existing #DF IST1), and in the #GP handler **repair a
mangled saved RIP** (kernel code pointer with a mangled top byte) and resume.
See `vendor/trapframe/src/arch/x86_64/{gdt.rs,idt.rs}` and
`kernel-hal/src/bare/arch/x86_64/trap.rs`.

With that, `timeout -s TERM 1 sleep 5` no longer silently triple-faults — it now
reaches a **diagnosable panic**:

```
[KERNEL PAGE FAULT] vaddr=0x97 flags=READ rip=0xffffff00001bfee7
panic at zCore/src/handler.rs:42
```

`rip=0xffffff00001bfee7` resolves to **`<rcore_fs_mountfs::MNode as INode>::metadata`**,
and the disassembly is a **vtable call through a corrupt pointer**:
```
mov (%rsi),%rax
mov 0x8(%rsi),%rcx      ; rcx = inner inode's vtable ptr = 0x87 (GARBAGE)
mov 0x10(%rcx),%rdx     ; <- #PF reading [0x87+0x10] = [0x97]
...
jmp *%rcx               ; would call through the corrupt vtable
```

So the root cause is a **corrupted / use-after-freed filesystem node**: an
`MNode`'s inner `Arc<dyn INode>` (or its vtable pointer) is overwritten with a
tiny garbage value (`0x87`), reached via a path lookup (`metadata()`) during the
signal cascade. This matches the very first symptom seen this whole hunt
(`Arc<MountFS>::drop_slow`). The small garbage values sprayed over live pointers
(`0x01` on stack return addresses, `0x87` on a heap vtable) point at one wild
write, likely tied to the child process's termination dropping fs state that a
concurrent lookup still holds.

## Mitigation is PARTIAL / non-deterministic (important)

Re-running the repro: run 1 reached the diagnosable `MNode::metadata` #PF panic
(above); run 2 still **silently triple-faulted**. So the #GP-IST mitigation only
helps when the corruption happens to land on a saved return address (-> #GP it
can catch); when the same wild write lands on the #DF/#GP IST descriptor, the
TSS/GDT, or produces a #PF that re-faults during panic, the machine still dies
silently. This is consistent with a **wild write spraying small garbage values**
(`0x01`, `0x0a`, `0x87`) across whatever memory it hits — stack return addresses
AND heap `Arc`/vtable pointers — rather than a single clean fault. A reliable
keep-alive is therefore NOT achievable by fault interception; the real fix is to
stop the wild write. The mitigation is kept because it is pure hardening (a
dedicated #GP IST is standard practice) and it is what surfaced the `MNode` root
cause.

## MNode canary experiment: the primary victim is the executor STACK, not fs nodes

An `MNode` corruption canary was added (`vendor/rcore-fs-mountfs/src/lib.rs`): a
`poison` magic as the struct's first field (`#[repr(C)]`), plus a `check_poison`
run on entry to the hot `INode` methods (`metadata`/`read_at`/`write_at`/`find`/
`get_entry`) that validates both `poison` AND the inner inode fat pointer (data +
vtable words) against the plausible kernel-pointer range before dereferencing.

Under the crashing repro across ~6 fresh boots the result was decisive:

- **`MNODE-CORRUPT` fired 0 times.** The canary never tripped, yet the VM still
  died — either silently, or via `[#GP-repair]` (the mangled saved-return-address
  path) which fired on ~1/3 of boots.
- So the earlier diagnosable `MNode::metadata` `#PF` was a **rare secondary
  manifestation**. The **primary, dominant victim is the executor kernel stack**
  (saved return addresses near `run_executor`'s frame, the consistently-observed
  `RBP=ffffff00016ffbf8`), **not** heap `MNode`/inode objects.

Consequences for the fix:

- Fault interception on fs nodes (or any single heap object) cannot stabilise the
  system: the machine mostly dies from the stack corruption, not the node deref.
- A hardware watchpoint (kernel-programmed `DR0`–`DR3`) is **not viable** here:
  the corrupted slot is a *legitimately written, live* stack slot (`run_executor`'s
  frame), so a write-watch cannot distinguish the wild write from normal
  call/`ret` traffic, and debug registers cannot match on value.

Cleared by inspection this pass (all correctly bounded — not the upward
stack-buffer overflow): `sys_readlinkat` (heap `vec![0; len.min(4096)]`),
`prctl(PR_GET_NAME)` (`n = bytes.len().min(TASK_COMM_LEN-1)`), `INodeExt::
read_as_vmo`/`read_as_vec` (`len = (size-offset).min(buf.len())`), the procfs
readers, the ELF-loader init-stack build (`ProcInitInfo::push_at` → heap `Vec` →
bounded `stack_vmo.write`). Also: `vfork` does **not** share the address space
(`fork_from`'s `_vfork` arg is ignored — vfork == fork + parent blocks on
`wait_signal`), and the signal-return path (`restore_after_handle_signal`) does
only fixed-size struct copies + user-VA reads, no kernel-stack overflow.

## Hardening landed (option A): recover instead of triple-faulting

`check_poison` is kept as **defense-in-depth**: on a corrupt node it now `error!`s
the exact clobber pattern and returns `EIO` (`FsError::DeviceError`) instead of
dereferencing the garbage vtable. Combined with the `#GP` IST + RIP repair, this
converts the known secondary crash site from a silent triple fault into a
survivable per-syscall error. It does **not** fix the underlying wild write.

## SIMPLER reproducer: `cat /proc/self/exe` (no signals, no timers)

A much smaller trigger than the `timeout` cascade was found: reading the
`/proc/self/exe` magic link on its own perturbs the same state. From the shell:

```sh
cat /proc/self/exe > /dev/null
```

Behaviour is non-deterministic per boot: it usually **hangs silently** (the
shell blocks in the read, no serial output, no detector banner), and ~1 boot in
~15 instead surfaces a **clean, diagnosable kernel page fault** (below). Because
it needs neither a signal handler nor an itimer, the fork/exec + signal cascade
of `timeout` is NOT required — the corruption lives in the plain
exec/path-lookup / `/proc/self/exe` read machinery. This removes the entire
signal-delivery half of the earlier hypothesis space.

## The wild write is a `memset` (compiler_builtins `set_bytes`) — and memset is a VICTIM

The clean fault captured under `cat /proc/self/exe`:

```
[KERNEL PAGE FAULT] vaddr=0xffffff9c0169fbc8 flags=WRITE rip=0xffffff000056fdb9
  (no current thread -- fault in kernel-private context)
```

- `rip=0xffffff000056fdb9` resolves (addr2line) to
  **`compiler_builtins::mem::impls::set_bytes`** — i.e. the write is a `memset`.
- The faulting **destination** `0xffffff9c0169fbc8` is a *valid executor-stack
  heap address with one byte flipped*: strip byte 4 (`0x9c -> 0x00`) and it is
  `0xffffff000169fbc8`, squarely in the coroutine-stack region (cf. the
  consistently-observed `RBP=0xffffff00016ffbf8`).
- The span from any plausible valid base up to `0xffffff9c…` is `~0x9c00000000`
  bytes — absurd for a memset length. So this is **not** a runaway length: it is
  a memset whose **destination pointer was corrupted** (a single byte, `0x00 ->
  0x9c`, at byte offset 4), exactly the same *tiny-value byte-spray* signature as
  the stack case (return-address top byte `0xff -> 0x01/0x0a`) and the `MNode`
  case (inode vtable `-> 0x87`).

Conclusion: **`memset` is itself a downstream victim** — it was handed a
byte-corrupted pointer — just like the mangled `ret` and the `0x87` vtable. All
three are the same primary writer scattering small byte values across live
pointers/return-addresses; each fault is wherever a corrupted pointer was next
*used*, not where it was *written*. This is why neither a watchpoint (live slot)
nor a single victim's backtrace pins the root: the backtrace of the memset names
the code that *used* the bad pointer, not the code that flipped the byte.

## Instrumentation left in place

- `zCore/src/handler.rs` kernel-private `#PF` path now walks the frame-pointer
  chain **and** raw-scans the stack for kernel code pointers, via the blocking
  `serial_write_fmt_spin` writer (survives the re-fault during panic). Captured
  `rbp`/`rsp` come from `kstats::note_fault_regs` (set in the arch trap entry).
  When the diagnosable-fault variant lands it prints `[kfault-bt]` frames naming
  the memset's *user*; kept because it is generically useful for any future
  kernel-private fault.
- `vendor/rcore-fs-mountfs/src/lib.rs` keeps the `MNode` poison + inode-pointer
  guard as defense-in-depth (returns `EIO`, logs the clobber pattern). Verified
  it does **not** false-positive on normal fs traffic (`ls`, `cat /etc/hostname`,
  path lookups).

## ROOT CAUSE FOUND AND FIXED: self-referential `/proc/self/exe` -> unbounded recursion

The final bisection (`readlink` works, `head -c 16` works, `cat` of the whole
file hangs 14/14 boots, direct `cat /bin/busybox` works) plus one log line
cracked it: the dying child processes were named **`exe`** —

```
[exit] pid=1226 (exe) killed by signal ...
```

busybox in standalone-shell mode re-executes its applets via
**`execve("/proc/self/exe")`** (bb_busybox_exec_path). `sys_execve` stored that
LITERAL string as the new image's `execute_path`. From then on the magic link is
**self-referential** for that process: `lookup_inode_at("/proc/self/exe")` reads
`execute_path()` — which IS `"/proc/self/exe"` — and calls itself again, without
bound (not a compilable tail call: the looked-up `String` needs a drop after the
call, so every iteration burns a real stack frame).

The coroutine (executor) stack is a **guard-page-less 128 KiB heap allocation**
(`PreemptiveScheduler`'s `Executor::new`, `Global.allocate`). The runaway
recursion therefore silently writes thousands of frames DOWNWARD past
`stack_base` into **neighbouring heap allocations** — spraying saved return
addresses (`0xffffff00_00xxxxxx`, whose stray bytes read back as the "mangled
top byte" `ff->01/0a`), ASCII `"/proc/self/exe"` path bytes, and small values
(`0x87`...) over whatever lives below: other tasks' stacks, `MNode`s, Arc
vtables. Every observed symptom — the `timeout -s TERM 1 sleep 5` triple fault
(busybox `timeout` spawns `sleep` via the same `/proc/self/exe` re-exec, so the
*child execve itself* recursed), the mangled `#GP` RIPs, the `0x87` MNode
vtable, the memset-victim `#PF`, the deterministic `cat /proc/self/exe` hang —
was this one bug. The 4-word canary at `stack_base` never helped because the
overflowing task never *returns* to have it checked.

Why the trigger patterns made it look like signals/timers: plain applets exec
ONCE off the shell (whose `execute_path` is the real `/bin/busybox` — that exec
resolves fine and then *stores the poison*). The corruption only fires when a
**re-exec'd child itself execs another applet or opens `/proc/self/exe`** —
which `timeout` (spawns `sleep`), `time`, `nice` etc. do, and which the
signal-cascade repros all happened to involve.

Fix (two layers, both landed):
1. **`sys_execve` canonicalizes** (`linux-syscall/src/task.rs`): when the exec
   path is `"/proc/self/exe"`, substitute the CURRENT `execute_path` (by
   construction the real on-disk binary) before loading/storing — the literal
   magic string is never stored again. Children now show up named `busybox`,
   not `exe`.
2. **Recursion guard** (`linux-object/src/fs/mod.rs`): if `execute_path` is
   empty or itself `"/proc/self/exe"`, `lookup_inode_at` returns `ELOOP`
   instead of recursing — same contract as a real symlink cycle.

Verified in QEMU after the fix, all in one boot, zero corruption banners:

| test | before | after |
|---|---|---|
| `cat /proc/self/exe` | hang (14/14 boots) | exit 0 |
| `timeout -s TERM 1 sleep 5` (x3) | triple fault | exit 143 each, kernel alive |
| `timeout 5 true` | shell hang (the "separate reaping bug") | exit 0 — same root cause after all |
| `env true`, `readlink /proc/self/exe`, `ls` | ok | ok |

Two residual quirks were then also root-caused and FIXED (both pre-existing,
unrelated to the corruption):
- `dd bs>=4096` EFAULT ("Bad address") on any input: the VMAR first-fit search
  could place a non-FIXED `mmap(NULL, ...)` at **address 0** (glibc put dd's
  I/O buffer there; `read`'s null-check then bounced it). Fixed by enforcing
  Linux's `mmap_min_addr` (64 KiB) floor — threaded as `map_ext_min` so it
  applies ONLY to the mmap path: a global floor in `determine_offset` shifted
  the ELF loader's app sub-VMAR off base 0 and SIGSEGV'd every non-PIE binary
  (lesson recorded in the code comments).
- Pipelines carrying binary data (`cat /bin/busybox | wc -c`) died with a
  spurious SIGINT: a syscall-level "ETX (0x03) -> Ctrl-C" conversion keyed only
  on `fd == 0`, firing on any read chunk that happened to START with byte 0x03
  — regardless of whether stdin was a terminal. Removed: the VT `Stdin`
  (termios ISIG) and both PTY slave implementations already do proper VINTR ->
  SIGINT themselves. (Side note: `^C` at the *serial* console prompt was only
  ever "handled" by that hack cosmetically; serial input does not run through
  the termios line discipline — routing it there is a separate follow-up.)

Hardening that stays (all earned its keep in this hunt): the `#GP` IST stack +
mangled-RIP repair, the kernel-private-`#PF` `[kfault-bt]` stack walk, and the
`MNode` poison/EIO guard. Additionally, a **per-tick stack-canary tripwire**
landed: the timer IRQ checks the currently-running executor's base canary
(`executor::check_current_executor_canary`, wired in the x86_64 trap handler) —
any future runaway recursion through the guard-page-less coroutine stack now
panics with a labelled `[stack-canary] COROUTINE STACK OVERFLOW` banner within
~4 ms instead of silently corrupting the heap. (A real guard page would need
page-table surgery under the heap allocator and remains possible future work;
the tripwire covers the diagnosability gap at ~zero cost.)

## Hardware watchpoints ARE viable — on a write-once victim (landed)

An earlier pass ruled out debug registers, and was right *about its target*: the
corrupted slot it chased was a saved return address in `run_executor`'s frame —
a slot the kernel legitimately writes on every `call`/`ret`, so a write-watch
there fires constantly and cannot single out the wild write (and DR0–DR3 cannot
match on a *value*).

That reasoning does not carry to a **write-once** victim, and a new report from
hardware supplies one. A kernel page fault printed its running process as:

```
[KERNEL PAGE FAULT] vaddr=0x0 flags=WRITE rip=0xffffff0001d9daa3
[diag] running thread: 1981 "" in process "l<invalid utf-8>"
[kfault-bt]   [rsp0]=0x10497 <- return address of the faulting function (non-kernel value = stack corruption)
```

Two things follow:

- **The process name was clobbered.** A `KObjectBase` name is a Rust `String`,
  UTF-8 by construction, built once when the object is named and thereafter only
  read. Printing as `"l\u{fffd}"` means its heap buffer (or its ptr/len header)
  was overwritten — the same spray of small garbage values over live pointers
  this document has been tracking, landing on a new victim class.
- **`rip` was not in `.text`.** Symbolizing it against the kernel ELF returns
  `zcore::memory::init::HEAP` — a *small `.bss` static* (a
  `Mutex<BuddyAllocator>`), which no instruction pointer can legitimately be
  inside; `addr2line` reports the nearest preceding symbol for out-of-range
  addresses. And since the fault is `flags=WRITE` on `vaddr=0x0`, the
  instruction at `rip` was fetched and executed successfully — only its store
  faulted. So the CPU was executing a mapped, executable page that is not a
  function: control flow had already been hijacked before the NULL write.

Because the name buffer is never legitimately rewritten, a write-watch on it is
**silent under normal operation** — so the first trap it takes belongs to the
corruptor, with its RIP in the trap frame. That is the datum this hunt has been
missing (the `MNODE-CORRUPT` canary fired zero times; a canary only reports that
corruption *happened*, never *who*).

### Using it

`kernel-hal/src/common/watchpoint.rs` programs DR0 as a data-write watchpoint.
Arm it by name from `zircon-object`:

```rust
zircon_object::object::watch_process_name("lunarbar"); // next match gets watched
zircon_object::object::unwatch_process_name();          // disarm
```

The next kernel object whose name starts with that prefix has DR0 pointed at the
first 8 bytes of its name buffer. On a hit the handler prints, over the
spin/blocking serial writer (so it survives an already-corrupted machine), the
writing RIP, the CPU, and a frame-pointer walk:

```
[watchpoint] HIT #1 on 8B at 0xffffff0001a2b3c0 — WRITTEN BY rip=0xffffff00001c4e21 (cpu0 rsp=… rbp=…)
[watchpoint] symbolize with: llvm-addr2line -e <zcore.elf> -fC 0xffffff00001c4e21
[watchpoint]   #00 ret=…
```

Design notes that matter for reading the output:

- Debug registers are **per-CPU** and are not part of the task context, so the
  request is published globally with a generation counter and each CPU programs
  its own registers from the timer tick — the watch is live on every core within
  one tick (~4 ms), with no IPI. A hit therefore names whichever core did it.
- A data watchpoint is a **trap, not a fault**: it is delivered after the store
  retires, so the handler reports and resumes. The write still lands — this
  names the writer, it does not prevent the corruption.
- Only 8-byte-aligned names of ≥8 bytes are armed: a shorter window would
  overlap neighbouring allocations and make hits ambiguous, and x86 silently
  ignores a misaligned watchpoint rather than reporting an error.
- Idle cost is a relaxed load and compare per tick; DR7 stays 0 when disarmed.

### Caveat on the report above

The trace that motivated this carries diagnostic strings (`null-range fault --
not retriable, halting`, `[diag] running thread:`) that exist in **no commit of
this repository**, so it came from an unpushed working tree. Before trusting any
symbolization, confirm the ELF is the one that produced the crash — a rebuild
between crash and `addr2line` invalidates every address.

## Residual hunt (branch claude/eclipse-drm-nvidia-qemu-q078tq): the zero-writer is a DEVICE/mapping UAF, not a CPU write

Reproduction of the ORIGINAL software root cause (`/proc/self/exe` recursion)
is fixed and confirmed dead: a 25-minute in-container QEMU storm (multithreaded
mmap/memset churn + fork storm + `timeout -s TERM 1 sleep 5` + `cat
/proc/self/exe`, four parallel loops) produced **zero** corruption banners.

The residual crash the user still hits at desktop start (`[null-exec] ...
region=usable exec=1`, a ~1 KB run of zeros in the MIDDLE of a live IMMORTAL
coroutine stack, no guard hit, then a hang on a TLB shootdown to the wedged
CPU) has a different shape and a different writer:

- **Not a stack overflow**: the zeros are a bounded run with valid saved
  return addresses ABOVE them (a growth would have hit the bottom hard guard
  and printed `[stack-guard]`).
- **Not a CPU physmap write**: `check_physmap_write` guards `pmem_zero`/
  `pmem_write`/`pmem_copy` against the live-stack frame bitset and would print
  `[physmap-smash]`; the user's log had none.
- **Not the `/proc/self/exe` recursion**: fixed; storm confirms.

That leaves the two writers no CPU-side guard can see, both the same shape:

1. a **device DMA** (NIC RX ring, GPU pushbuffer/GEM, NVMe completion) whose
   descriptor still points at a physical block after the driver freed it;
2. a **userspace `VmObject::new_physical` mapping** — the nouveau-uAPI GEM CPU
   mmap: `eclipse_rm_gem_map_cpu` publishes `memdescGetPhysAddr(AT_CPU)`, Mesa
   `mmap`s it, and under the **zero-VRAM model every GEM travels this path**.

### The structural hole (confirmed by inspection)

- RM sysmem is allocated by `osAllocPagesInternal` (vendor/eclipse_rm_mem.c) →
  `drivers_dma_alloc` → `frame_alloc_contiguous` → `memory::frame_alloc`, which
  DOES run `frame_alias_check`. So an allocation that overlaps a live stack is
  caught at alloc time. Good.
- On free, `osFreePagesInternal` → `drivers_dma_dealloc` → `frame_dealloc`
  returns the block to the general pool **immediately, with no quarantine**.
- `frame_alloc` then re-hands that block to a fresh coroutine stack;
  `frame_alias_check` passes (nothing lived there when it was freed).
- If a device descriptor or a userspace GEM mmap still references the old
  physical block, its next write lands on the recycled stack — the exact
  "all-zeros usable region, no guard" signature. Zeros because Mesa clears a
  buffer it still holds mapped, or a device writes a zero-filled descriptor.

This only fires with the nouveau uAPI ACTIVE (real NVIDIA hardware): in
QEMU without an NVIDIA GPU the uAPI is disabled, and — decisively — **the
desktop stack (labwc/seatd/Mesa) cannot be installed in the CI sandbox** (the
agent proxy denies Alpine/git: `dl-cdn.alpinelinux.org` and `git.busybox.net`
both 403), so labwc never runs and the GPU/DMA path is never driven. That is
why the user's image (built with network) crashes and the sandbox image
(no network at build) cannot reproduce it.

### Detector landed (diagnostic only, zero behaviour change)

`kernel-hal/src/bare/stack_guard.rs` gains a ring of the last 512 freed DMA
blocks (`dma_free_note`, recorded from both `drivers_dma_dealloc` and
`virtio_dma_dealloc`). The null-execute fault path
(`report_dma_uaf_if_recycled` in trap.rs) translates the faulting stack VA to
its physical frame and asks whether that frame was a recently-freed DMA block.
A hit prints:

```
[dma-uaf] the corrupted stack frame pa=... was a DMA buffer freed N DMA-free(s)
ago — ... THIS is the zero-writer (a freed GEM/ring recycled into a live
coroutine stack)
```

On the user's next hardware crash this CONFIRMS the UAF and names it (a canary
only ever said corruption happened, never that a freed DMA buffer was the
writer). If it does NOT fire on a real crash, the writer is a live (never-freed)
DMA mapping instead, which narrows it to a descriptor pointing at a stack frame
from the start — a different fix.

### The real fix (needs the confirmation above before shipping)

Refcount a GEM's physical frames against BOTH its handle AND any live
`VmObject::new_physical` / VM_BIND mapping, so `drivers_dma_dealloc` cannot
return frames to the pool while a device or a process can still write them —
Linux's `drm_gem_object` page refcount. A cheaper interim: quarantine freed
DMA blocks (hold them out of the pool for a bounded window, like the freed-stack
quarantine) to shrink the UAF window. Both are behaviour changes on the
desktop-critical path and should land only after the detector confirms the
mechanism on real hardware.
