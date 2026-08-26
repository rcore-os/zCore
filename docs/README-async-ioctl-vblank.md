# Making `WAIT_VBLANK` actually block (async `ioctl`)

Handoff for the refactor that lets a DRM `WAIT_VBLANK` wait. Written after the
investigation that found it, so the next session starts from the constraints
rather than rediscovering them.

## The bug, in one measurement

`drmbench` (see `tools/drmbench/`) on Eclipse, 1920x1080@60:

```
pageflip_per_sec       = 59.43 flips/s      pageflip_latency_us = 16826   <- correct, 60 Hz
vblank_interval_ms     = 0.07 ms                                          <- should be 16.67
```

Same boot, same CRTC. The page-flip path paces itself correctly against the
synthetic vblank clock; the blocking `WAIT_VBLANK` returns ~200x too early. A
client that paces frames with `drmWaitVBlank` therefore spins instead of
sleeping, burning a core. Reproduced across two independent boots.

## Where it is

`linux-object/src/fs/devfs/drm_scheme.rs`, `DRM_IOCTL_WAIT_VBLANK` arm of
`io_control`. The event branch (`_DRM_VBLANK_EVENT`) is **correct** — it defers
delivery via `drm::schedule_vblank_event` to the next synthetic vblank. Only the
**blocking** branch is wrong: it fills the reply and returns immediately.

## Why it was left that way — read before "fixing" it

The comment in that arm records the history: a previous implementation *did*
block, as a **busy 16.7 ms spin**, and that caused

> severe CPU starvation on a cooperative async runtime — making the system
> appear frozen

**Do not reintroduce a spin.** Blocking without yielding starves every other
coroutine on that CPU. That regression has already been paid for once.

## Why it is not a one-liner

Sleeping cooperatively means `kernel_hal::thread::sleep_until`, which is
`async`. The whole ioctl chain is synchronous:

```
sys_ioctl  ->  FileLike::ioctl        (linux-object/src/fs/mod.rs:410,  fn)
           ->  INode::io_control      (drm_scheme.rs:962,               fn)
```

There is no synchronous yield in this kernel, so a sync frame simply cannot give
the CPU back. The blocking semantics require an async path.

## Options

**A. Make the ioctl path async** (recommended). `async fn ioctl` on `FileLike`,
propagated through `sys_ioctl`. Fixes the general problem — any ioctl that must
block gets the ability — not just vblank.

Cost: `FileLike` is implemented by every file type, so this touches a lot of
call sites even though most bodies are unchanged (`async fn` with no `.await`).
The risk is breadth, not depth.

**B. A cooperative sleep callable from sync context.** Narrower, but it has to
be designed so it cannot reintroduce the starvation above — which is exactly the
hard part, and why A is preferred.

## Suggested order for A

1. Add `async fn ioctl` alongside the existing sync one (default impl forwards
   to the sync version) so nothing breaks while it lands.
2. Move `sys_ioctl` to call the async variant.
3. Override it only in `DrmDevice`, where the blocking `WAIT_VBLANK` branch
   awaits `sleep_until(next_vblank_deadline())`.
4. Verify with `drmbench`: `vblank_interval_ms` should read ~16.7 with a small
   jitter stddev, and `pageflip_per_sec` must stay at ~60 (proving the flip path
   was not disturbed).
5. Watch for regressions in the paths that call ioctl from odd contexts (tty,
   sockets, epoll) — an `.await` newly introduced where a lock is held is the
   failure mode to look for.

## Verifying

```sh
cc -O2 -static -o drmbench tools/drmbench/drmbench.c   # runs on Eclipse and Linux
drmbench /dev/dri/card0 3
```

Before: `vblank_interval_ms=0.07`. After: ~16.7 at 60 Hz.

## Priority note

The event path already works, and modern compositors (wlroots/labwc) pace with
page-flip events, not the blocking ioctl. This is a legacy-path correctness bug,
worth fixing properly rather than quickly.

## Other findings from the same benchmark run

* `dumb_cycle_per_sec = 54` (~18 ms to create+map+destroy an 8 MiB buffer).
  Suspect synchronous zeroing on `CREATE_DUMB`, but part of that cost may be
  emulation of touching 8 MiB — **measure under KVM before calling it a bug**.
* `SET_CLIENT_CAP(ATOMIC)` returning `EOPNOTSUPP` is **not** a bug: the atomic
  uAPI is opt-in behind the `drm.atomic` cmdline flag, deliberately mirroring a
  Linux driver without `DRIVER_ATOMIC`. With `drm.atomic` the path measures
  6420 commits/s at 156 us/commit.
