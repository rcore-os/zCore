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
    /// A pending `sync_file` import (see [`import_snapshot`]): this object
    /// also counts as signaled — binary point 1 — once `src` reaches
    /// `target`. `None` for the normal case, and cleared whenever the object
    /// is signaled or reset directly, mirroring how a real `drm_syncobj`
    /// REPLACES its fence on those operations rather than accumulating them.
    linked: Option<(u32, u64)>,
}

/// The point `handle` counts as having reached: its own counter, plus the
/// binary signal a pending `sync_file` import contributes once its source
/// reaches the point captured at export time. `depth` bounds the (exotic)
/// case of an import whose source is itself waiting on an import.
///
/// Callers must already hold the table lock.
fn effective_point(objects: &[Syncobj], handle: u32, depth: u8) -> Option<u64> {
    let obj = objects.iter().find(|o| o.handle == handle)?;
    let mut point = obj.point;
    if let (Some((src, target)), true) = (obj.linked, depth > 0) {
        if let Some(src_point) = effective_point(objects, src, depth - 1) {
            if src_point >= target {
                point = point.max(1);
            }
        }
    }
    Some(point)
}

/// Link-following depth for [`effective_point`].
const LINK_DEPTH: u8 = 4;

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
        linked: None,
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
    // A direct signal replaces whatever fence the object carried, imported
    // sync_file included — same as real drm_syncobj.
    obj.linked = None;
    true
}

/// Snapshot of `handle`'s current fence, for
/// `SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE`: the point it has reached
/// right now. `None` for an unknown handle.
///
/// A real `sync_file` carries the `dma_fence` that was attached to the
/// syncobj at export time, and becomes signaled when that fence does. Here
/// a fence IS "this timeline reached point N", so the snapshot is that N —
/// and because this driver's submission path is synchronous (`EXEC` blocks
/// until the GPU fence lands and only THEN signals its `sig` syncobjs), a
/// fence exported after a submit is one whose work has already completed.
/// The exported snapshot is therefore normally already satisfied, which is
/// exactly what the importer needs to observe.
pub fn export_snapshot(handle: u32) -> Option<u64> {
    let table = TABLE.lock();
    effective_point(&table.objects, handle, LINK_DEPTH)
}

/// `SYNCOBJ_FD_TO_HANDLE_FLAGS_IMPORT_SYNC_FILE`: make `dst` carry the fence
/// captured in a snapshot (`src` reaching `target`). Returns `false` if
/// either handle is unknown.
///
/// Resolved immediately when the snapshot is already satisfied (the common
/// case, see [`export_snapshot`]); otherwise the dependency is recorded and
/// resolves on its own as `src` advances, so a waiter never has to know an
/// import happened.
pub fn import_snapshot(dst: u32, src: u32, target: u64) -> bool {
    let mut table = TABLE.lock();
    let Some(src_point) = effective_point(&table.objects, src, LINK_DEPTH) else {
        return false;
    };
    let reached = src_point >= target;
    let Some(obj) = table.objects.iter_mut().find(|o| o.handle == dst) else {
        return false;
    };
    if reached {
        obj.point = obj.point.max(1);
        obj.linked = None;
    } else {
        obj.linked = Some((src, target));
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
    obj.linked = None;
    true
}

/// Current timeline point (`SYNCOBJ_QUERY`), or `None` if `handle` is
/// unknown.
pub fn query(handle: u32) -> Option<u64> {
    let table = TABLE.lock();
    effective_point(&table.objects, handle, LINK_DEPTH)
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
    let start_us = unsafe { crate::bus::drivers_timer_now_as_micros() };
    let mut stall_logged = false;
    loop {
        let mut signaled_count = 0usize;
        let mut first_signaled: Option<u32> = None;
        {
            let table = TABLE.lock();
            for (i, &h) in handles.iter().enumerate() {
                let Some(point) = effective_point(&table.objects, h, LINK_DEPTH) else {
                    return WaitOutcome::Invalid;
                };
                let target = points.map(|p| p[i]).unwrap_or(1);
                if point >= target {
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
        let now_us = unsafe { crate::bus::drivers_timer_now_as_micros() };
        if now_us >= deadline_us {
            return WaitOutcome::Timeout;
        }
        // Stall reporter: NVK's fence waits pass an effectively infinite
        // absolute deadline (INT64_MAX ns), so a syncobj that never gets
        // signaled parks its caller here FOREVER with nothing in dmesg --
        // the exact shape of the vkcube/eglgears "hangs after device
        // creation" reports. Crossing 2 s with the deadline still far away
        // is that situation, not a normal frame wait; say so once per call
        // (budgeted per boot) with enough to identify the station.
        if !stall_logged && now_us.saturating_sub(start_us) >= 2_000_000 {
            stall_logged = true;
            stall_report(handles, points, wait_all, deadline_us.saturating_sub(now_us));
        }
        core::hint::spin_loop();
    }
}

/// One console line for a wait parked past 2 s: every handle with its target
/// point and current point (-1 = handle vanished mid-wait). Budgeted per boot
/// so a session full of legitimately-slow waits cannot storm the UART (klog
/// writes synchronously to it -- an uncapped line on a re-entered path is how
/// the pointer froze once before).
fn stall_report(handles: &[u32], points: Option<&[u64]>, wait_all: bool, remaining_us: u64) {
    use core::sync::atomic::{AtomicU32, Ordering};
    static BUDGET: AtomicU32 = AtomicU32::new(0);
    const MAX_REPORTS: u32 = 8;
    let n = BUDGET.fetch_add(1, Ordering::Relaxed);
    if n >= MAX_REPORTS {
        return;
    }
    let list = describe(handles, points);
    crate::klog_warn!(
        "[syncobj] WAIT parked >2s and still unsignaled (wait_all={} deadline in {}s):{} (handle:target/current; report {}/{} this boot)",
        wait_all,
        remaining_us / 1_000_000,
        list,
        n + 1,
        MAX_REPORTS
    );
}

/// `" handle:target/current"` for every handle, `-1` for one that does not
/// exist. The one line that turns "a wait timed out" into "THIS fence never
/// arrived", so every caller that gives up on a wait should print it — the
/// stall reporter below, and the driver's own `EXEC` timeout, whose 1 s
/// deadline expires long before this reporter's 2 s threshold and used to
/// report nothing but a count.
pub fn describe(handles: &[u32], points: Option<&[u64]>) -> alloc::string::String {
    let mut list = alloc::string::String::new();
    let table = TABLE.lock();
    for (i, &h) in handles.iter().enumerate() {
        let target = points.map(|p| p[i]).unwrap_or(1);
        let cur = effective_point(&table.objects, h, LINK_DEPTH).map_or(-1i64, |p| p as i64);
        let _ = core::fmt::write(&mut list, format_args!(" {:#x}:{}/{}", h, target, cur));
    }
    list
}
