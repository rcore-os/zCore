# Memory-leak hunt: what leaks, and what was ruled out

Written while chasing `frame_alloc FAILED: 2561 MiB used / 2561 MiB managed`,
which killed the XFCE session about two minutes into every `startx`.

Method throughout: churn one kernel path N times in QEMU and read
`/proc/memhogs` between rounds. That file splits physical use into
per-process private/shared, the MAP_SHARED VMO registry, and an
`unattributed` remainder (kernel heap, ramfs contents, leaked frames) —
a leak that belongs to no process is invisible in per-process accounting,
which is why it went unnoticed for so long.

## THE cause of the desktop OOM: no shared page cache — FIXED

Not a leak at all, which is why 24 churn-and-measure rounds never found it.
Every `MAP_PRIVATE` mapping of a file gets its **own** `VmObject`, demand-paged
from the inode independently (`File::get_vmo` → `VmObject::new_paged_with_source`
with a fresh `FileFrameFiller`). There is no page cache behind it, so **N
processes reading the same library cost N × its pages**.

Measured in QEMU — 8 processes mapping the same 32 MiB read-only file
`MAP_PRIVATE`, all holding their mappings:

    baseline                     39 MiB used
    8 readers holding           306 MiB used     (+267 ≈ 8 × 32)
    vmo by kind: PagedSource      8 /  256 MiB   (one VMO per reader)

Linux costs 32 MiB for the same thing: `MAP_PRIVATE` file pages come from
the shared page cache and are only copied on write.

That is the whole OOM. From a live session's `[eclipse-mem]` samples, taken
every 5 s once XFCE starts:

    t=0s   used=645/7160   pgsrc= 332 / 492MiB   contig=2/7MiB
    t=15s  used=2335       pgsrc=2042 /2122MiB   contig=2/7MiB
    t=30s  used=4540       pgsrc=4165 /4258MiB   contig=2/7MiB

`PagedSource` is the only bucket that moves; `Contiguous` (the DRM buffer
pool) sits at 2 objects the whole time, which rules out the graphics path.
The per-process census makes the mechanism plain — three GTK apps that
should be ~30 MiB each:

    xfce4-panel  PRIV 266 MiB
    xfdesktop    PRIV 266 MiB
    xfwm4        PRIV 265 MiB

They are not leaking; each is holding its own private physical copy of
GTK, GLib, cairo, pango and mesa. Twenty-five such processes is gigabytes.

### What the fix has to do, and the trap in it

Share the frames of file-backed `MAP_PRIVATE` mappings between processes and
copy only on write. The pieces are already here — `SHARED_FILE_VMOS` keyed by
inode, and `VmoFrameFiller`, which a `MAP_PRIVATE` mapping already uses when a
`MAP_SHARED` VMO exists for the file — but `VmoFrameFiller` *copies* each page
into the private VMO on first touch, so it gives correct semantics and no
sharing. Real sharing needs the source's frames mapped read-only into each
process and a copy taken on the write fault, i.e. `VmObject::create_child`
clone semantics rather than a filler.

The trap: the permission ceiling on file mappings is deliberately `RXW`
because ld.so `mprotect`s a library's text to RW for `DT_TEXTREL` relocations
and GNU_RELRO (see the comment in `sys_mmap`; capping it broke Firefox). So
"share when the mapping is read-only" is not sufficient — a later `mprotect`
can add WRITE to a shared mapping and let one process scribble on another's
pages. The write fault has to be handled, not the initial protection.

Note also that the kernel's existing COW-fork path is disabled (`FORKCOW=0`)
for corrupting user memory, so whatever COW machinery this uses needs its own
verification rather than inheriting that one's trust.

### The fix, and why it does not reuse the snapshot tree

A per-inode cache VMO, demand-paged from the file, held in `SHARED_FILE_VMOS`.
Both mapping flavours use it: `MAP_SHARED` maps it directly (as before), and
`MAP_PRIVATE` creates a *borrower* (`VmObject::new_paged_borrowing`). A
borrower keeps no frames for a clean page — a read fault resolves to the
cache's own frame, which the fault handler installs read-only — and the first
write fault copies just that page into the borrower and continues privately.
N processes mapping one library therefore share one set of frames.

The write-protect trap is handled at its root. `VmMapping::protect` no longer
special-cases the zero frame; it refuses to raise WRITE **in place** on any PTE
whose frame is not the one the VMO owns at that index (`committed_paddr`),
covering both the shared zero page and a borrowed cache frame. The PTE is
dropped instead, so the next write re-faults into `commit_page(WRITE)` and
copies up. That is exactly the ld.so `mprotect`-text-to-RW case.

The borrow deliberately does **not** go through the hidden-node snapshot tree
that COW-fork used (and that `FORKCOW=0` disables): `create_child` refuses a
borrower, the borrower holds the cache by a plain `Arc` and never reshapes it,
and `fork_copy` carries the borrow to the child. It shares no code with the
snapshot machinery, so it inherits none of its trust.

Verified in QEMU on this kernel:
- 8 processes mapping one 32 MiB file `MAP_PRIVATE` and holding: **306 MiB →
  78 MiB** (`PagedSource` 16 VMOs → 9: one cache + eight zero-resident
  borrowers). The remaining ~38 MiB over baseline is the readers' own stacks
  and the one shared copy.
- A correctness oracle (`cowpriv`, also passing unchanged on real Linux):
  cross-process isolation, `read(2)` coherence, the `mprotect`-then-write trap
  landing in a private copy and NOT in the file or another mapping, and a
  `MAP_SHARED` writer's store showing through a clean private page.
- Five unit tests in `vmo::tests` (clean-page sharing, copy-up-from-cache,
  base offset, demand-zero tail, `create_child` refusal) plus `fork_copy`.

One defect found and fixed along the way: the registry was keyed by the inode
`Arc`'s data pointer, but the VFS builds a fresh `Arc<dyn INode>` per `open()`,
so eight opens made eight caches and deduplicated nothing (306 MiB unchanged).
Re-keyed by `(filesystem, inode number)`; that is the run that gave 78 MiB.

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
