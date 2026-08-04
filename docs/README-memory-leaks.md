# Memory-leak hunt: what leaks, and what was ruled out

Written while chasing `frame_alloc FAILED: 2561 MiB used / 2561 MiB managed`,
which killed the XFCE session about two minutes into every `startx`.

Method throughout: churn one kernel path N times in QEMU and read
`/proc/memhogs` between rounds. That file splits physical use into
per-process private/shared, the MAP_SHARED VMO registry, and an
`unattributed` remainder (kernel heap, ramfs contents, leaked frames) —
a leak that belongs to no process is invisible in per-process accounting,
which is why it went unnoticed for so long.

## Found and fixed

**MAP_SHARED file mappings leaked their committed pages permanently.**
Every `mmap(MAP_SHARED)` of a file left its whole VMO behind: the process
exits, the file is unlinked, the pages stay. Measured at 8 MiB per cycle:

    6 maps:   89 MiB used,  6 registry entries,  48 MiB
   12 maps:  138 MiB used, 12 registry entries,  96 MiB
   30 maps:  288 MiB used, 30 registry entries, 240 MiB

Cause: a reference cycle. `prune_shared_vmos` dropped entries whose inode
`Weak` had died, but the entry itself keeps the inode alive —

    registry --Arc--> VmObject --Arc--> FileFrameFiller --Arc--> INode

— so `strong_count()` never reached 0 and nothing was ever pruned. The
cycle contributes exactly one inode reference, which is what makes a
correct rule expressible; see `prune_shared_vmos` in
`linux-object/src/fs/file.rs`. Eviction also needed a writeback path,
since with no `msync` the VMO *is* the file's storage while it lives.

## Ruled out by measurement

None of these grew across hundreds to thousands of iterations. Recording
them so the same ground is not covered twice:

| path | churn | result |
| --- | --- | --- |
| anonymous `mmap` + touch + `munmap` | 300 | flat |
| `pipe` create/close | 300 | flat |
| `socketpair` create/close | 300 | flat |
| Unix socket write+read round trip | 300 | flat |
| `eventfd` / `timerfd` / `signalfd` | 300 each | flat |
| `epoll_create` + `EPOLL_CTL_ADD` + close | 300 | flat |
| TCP socket create/close | 300 | flat |
| `memfd_create` + MAP_SHARED + touch | 1800 | flat |
| `pthread_create` + join | 900 | flat |
| open/close distinct files | 900 | flat |
| MAP_PRIVATE of a 1 MiB file (the ld.so shape) | 900 | flat |
| `SCM_RIGHTS` fd passing over a Unix socket | 900 | flat |
| MAP_SHARED under `/dev/shm` (the X11/Wayland shape) | 800 | flat |
| pty allocate/release | 800 | flat |
| process exit with a thread parked in `poll()` | 100 | flat |
| `fork` + `exec` from a large parent | see below | bounded |

**Process exit with a parked thread** deserves a note, because the code
invites the opposite conclusion. `Thread::stop` sets `Dying` and wakes the
suspend waker (unset for a thread inside a syscall) and the `killer`
channel (armed only by futex's `blocking_run`), so a future parked in
`poll` looks unreachable — and if the thread never dies, `Process::terminate`
never runs, so neither does `vmar.clear()`. It is not what happens: 100
cycles of a process committing 16 MiB and exiting with a thread parked in
`poll(NULL, 0, -1)` leave the process count flat and physical use plateauing.
The processes do terminate and their memory does come back.

**fork from a large parent** is bounded retention, not a leak. COW fork is
off (`FORKCOW=0`), so each fork eagerly copies the parent's committed
address space, and sampling mid-flight shows it: a 128 MiB parent pushed
use to 88–124 MiB across six rounds — oscillating, with no trend — and it
settled back to the 40 MiB baseline once the churn stopped. An 8 MiB parent
over 80 forks never moved at all, i.e. the transient scales with the copy,
not the fork count.

## Not reachable from the minimal rootfs

The DRM/framebuffer mapping path. `/dev/dri` only exists once a graphics
driver registers, and it does not in the plain QEMU image used here, so
Xorg's framebuffer mapping was never exercised. If a leak survives the fix
above, that is the first place to look — followed by simply reading the
`/proc/memhogs` dump that `.xinitrc` now writes to the console when the
session dies, which says in one line whether the memory is in processes or
unattributed. Those two answers need opposite fixes.
