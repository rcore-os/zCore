//! Generic DRM sync objects (`drm_syncobj`) -- a timeline counter each GEM
//! client can create, signal, and wait on. Part of core DRM (ioctl numbers
//! `0xBF`-`0xCF`, above `DRM_COMMAND_END`), not driver-specific: any
//! `DrmScheme` implementor can use these, and the ioctls themselves are
//! dispatched generically by `linux-object`'s `drm_scheme.rs`.
//!
//! Lives in `drivers` (not `linux-object`, where the ioctls are actually
//! parsed) so that a driver's own submission path -- e.g. `NvidiaGpu`'s
//! nouveau-uAPI `EXEC` handler in `nvidia.rs` -- can signal a syncobj
//! directly after a real hardware completion, without needing to call
//! back up into a higher crate layer (this crate has no dependency on
//! `linux-object`, and layering only allows calls downward).
//!
//! # Model
//!
//! Every syncobj is a timeline: a `u64` counter starting at 0 (or 1, if
//! created with `DRM_SYNCOBJ_CREATE_SIGNALED`). "Binary" (legacy,
//! non-timeline) signal/wait is just timeline point 1. There is no real
//! `dma_fence`/interrupt-driven signaling here -- everything is signaled
//! by an explicit call (`signal`/`timeline_signal`, from an ioctl or from
//! a driver's own completion polling, e.g. `NvidiaGpu`'s `EXEC` after
//! `eclipse_rm_exec_submit_signaled` confirms a real GPU semaphore
//! landed). [`wait`] is a bounded, CPU-spinning poll of that counter, not
//! a real wait-queue: `linux-object`'s `io_control` (where these ioctls
//! are dispatched) is a synchronous, non-async function, so there is no
//! lower-cost way to block here without deeper scheduler surgery. This
//! matches the spin-poll idiom already used throughout this codebase for
//! bounded hardware waits (e.g. `nvidia.rs` `gmmu_flush`, `eclipse_rm_init.c`
//! step18's semaphore poll) -- consistent, but real: a long wait pegs the
//! CPU core handling the ioctl for its whole duration.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use lock::Mutex;

struct Syncobj {
    handle: u32,
    point: u64,
}

struct SyncobjTable {
    objects: Vec<Syncobj>,
}

static NEXT_HANDLE: AtomicU32 = AtomicU32::new(1);

lazy_static::lazy_static! {
    static ref TABLE: Mutex<SyncobjTable> = Mutex::new(SyncobjTable { objects: Vec::new() });
}

/// Creates a syncobj, initially at point 0 (or 1 if `signaled`). Returns the
/// new handle.
pub fn create(signaled: bool) -> u32 {
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    TABLE.lock().objects.push(Syncobj {
        handle,
        point: if signaled { 1 } else { 0 },
    });
    handle
}

/// Destroys a syncobj. Returns `false` if `handle` is unknown.
pub fn destroy(handle: u32) -> bool {
    let mut table = TABLE.lock();
    let len_before = table.objects.len();
    table.objects.retain(|o| o.handle != handle);
    table.objects.len() != len_before
}

/// Binary signal (point = 1). Returns `false` if `handle` is unknown.
pub fn signal(handle: u32) -> bool {
    timeline_signal(handle, 1)
}

/// Sets a syncobj's timeline point directly (`SYNCOBJ_TIMELINE_SIGNAL`, and
/// what a driver calls after confirming real GPU completion for `EXEC`'s
/// `sig` list). Monotonic: never moves the point backwards, matching real
/// `drm_syncobj` semantics (a stale/reordered signal can't un-signal a
/// later one). Returns `false` if `handle` is unknown.
pub fn timeline_signal(handle: u32, point: u64) -> bool {
    let mut table = TABLE.lock();
    let Some(obj) = table.objects.iter_mut().find(|o| o.handle == handle) else {
        return false;
    };
    if point > obj.point {
        obj.point = point;
    }
    true
}

/// Resets a syncobj to point 0 (`SYNCOBJ_RESET`). Returns `false` if
/// `handle` is unknown.
pub fn reset(handle: u32) -> bool {
    let mut table = TABLE.lock();
    let Some(obj) = table.objects.iter_mut().find(|o| o.handle == handle) else {
        return false;
    };
    obj.point = 0;
    true
}

/// Current timeline point (`SYNCOBJ_QUERY`), or `None` if `handle` is
/// unknown.
pub fn query(handle: u32) -> Option<u64> {
    TABLE
        .lock()
        .objects
        .iter()
        .find(|o| o.handle == handle)
        .map(|o| o.point)
}

pub enum WaitOutcome {
    /// All (`wait_all`) or at least one required handles reached their
    /// target point. Carries the index into `handles` of the first one
    /// observed signaled, for `drm_syncobj_wait.first_signaled`.
    Signaled { first_signaled_index: u32 },
    /// `deadline` (absolute, `kernel_hal::timer::timer_now()`-comparable --
    /// see the microsecond conversion at the call site) passed first.
    Timeout,
    /// One of `handles` does not exist.
    Invalid,
}

/// Polls `handles` until every one (`wait_all = true`) or any one
/// (`wait_all = false`) reaches its target point, or `deadline_us`
/// (absolute microseconds, same clock as [`crate::bus::drivers_timer_now_as_micros`])
/// passes. `points`, if given, is per-handle target points
/// (`SYNCOBJ_TIMELINE_WAIT`); `None` means "target = 1" for every handle
/// (binary `SYNCOBJ_WAIT`). Spin-polls -- see the module doc for why.
pub fn wait(handles: &[u32], points: Option<&[u64]>, wait_all: bool, deadline_us: u64) -> WaitOutcome {
    loop {
        let mut signaled_count = 0usize;
        let mut first_signaled: Option<u32> = None;
        {
            let table = TABLE.lock();
            for (i, &h) in handles.iter().enumerate() {
                let Some(obj) = table.objects.iter().find(|o| o.handle == h) else {
                    return WaitOutcome::Invalid;
                };
                let target = points.map(|p| p[i]).unwrap_or(1);
                if obj.point >= target {
                    signaled_count += 1;
                    if first_signaled.is_none() {
                        first_signaled = Some(i as u32);
                    }
                }
            }
        }
        let done = if wait_all {
            signaled_count == handles.len()
        } else {
            signaled_count > 0 && !handles.is_empty()
        };
        if done {
            return WaitOutcome::Signaled {
                first_signaled_index: first_signaled.unwrap_or(0),
            };
        }
        if unsafe { crate::bus::drivers_timer_now_as_micros() } >= deadline_us {
            return WaitOutcome::Timeout;
        }
        core::hint::spin_loop();
    }
}
