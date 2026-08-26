//! Per-process "ever-writable region" tracking for write-xor-execute (P3).
//!
//! A single per-call conjunction check (deny only when a mapping requests
//! `WRITE` *and* `EXEC` simultaneously) is trivially defeated by the two-step
//! `mmap(PROT_WRITE)` then `mprotect(PROT_EXEC)` sequence (finding WX-2): each
//! call on its own looks benign. This module remembers, per process, every
//! address range that was *ever* writable, so a later request to make any part
//! of such a range executable is recognised as a W^X violation.
//!
//! State is bounded: each process tracks at most [`MAX_REGIONS`] intervals; on
//! overflow the process is marked *saturated* and conservatively treated as if
//! its whole address space were ever-writable (fail-closed for security, never
//! fail-open). Intervals are dropped on `munmap` of the range and on task exit.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use lock::Mutex;

/// Maximum tracked writable intervals per process before saturation.
const MAX_REGIONS: usize = 1024;
/// Maximum number of processes tracked before the least-recently-touched is
/// dropped, bounding total memory against a spawn-flood DoS.
const MAX_TRACKED_PIDS: usize = 4096;

#[derive(Default)]
struct ProcRegions {
    /// Sorted, non-overlapping `[start, end)` intervals ever mapped writable.
    intervals: Vec<(usize, usize)>,
    /// Once true, every executable request is treated as a W^X violation.
    saturated: bool,
    /// Monotonic touch stamp for LRU eviction under the pid cap.
    touch: u64,
}

lazy_static::lazy_static! {
    static ref REGIONS: Mutex<BTreeMap<u64, ProcRegions>> = Mutex::new(BTreeMap::new());
}

/// Monotonic counter used only to order LRU eviction (independent of wall clock).
static TOUCH: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
fn next_touch() -> u64 {
    TOUCH.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

fn ranges_overlap(a: (usize, usize), b: (usize, usize)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

fn end_of(addr: usize, len: usize) -> usize {
    addr.saturating_add(len)
}

/// Records that `[addr, addr+len)` was mapped (or re-protected) writable.
pub fn record_writable(pid: u64, addr: usize, len: usize) {
    if len == 0 {
        return;
    }
    let new = (addr, end_of(addr, len));
    let mut map = REGIONS.lock();
    evict_if_needed(&mut map, pid);
    let pr = map.entry(pid).or_default();
    pr.touch = next_touch();
    if pr.saturated {
        return;
    }
    // Coalesce against any overlapping/adjacent existing interval.
    let mut merged = new;
    pr.intervals.retain(|&iv| {
        if iv.1 >= merged.0 && iv.0 <= merged.1 {
            merged = (merged.0.min(iv.0), merged.1.max(iv.1));
            false
        } else {
            true
        }
    });
    pr.intervals.push(merged);
    if pr.intervals.len() > MAX_REGIONS {
        // Give up on precise tracking and fail closed.
        pr.saturated = true;
        pr.intervals = Vec::new();
    }
}

/// Returns `true` if any part of `[addr, addr+len)` was ever writable for `pid`.
pub fn is_ever_writable(pid: u64, addr: usize, len: usize) -> bool {
    let q = (addr, end_of(addr, len.max(1)));
    let map = REGIONS.lock();
    match map.get(&pid) {
        Some(pr) if pr.saturated => true,
        Some(pr) => pr.intervals.iter().any(|&iv| ranges_overlap(iv, q)),
        None => false,
    }
}

/// Drops tracked writable intervals overlapping `[addr, addr+len)` (on munmap).
pub fn clear_region(pid: u64, addr: usize, len: usize) {
    let q = (addr, end_of(addr, len.max(1)));
    let mut map = REGIONS.lock();
    if let Some(pr) = map.get_mut(&pid) {
        if pr.saturated {
            return; // cannot subtract from an imprecise saturated set
        }
        pr.touch = next_touch();
        let mut out = Vec::with_capacity(pr.intervals.len());
        for &iv in pr.intervals.iter() {
            if !ranges_overlap(iv, q) {
                out.push(iv);
                continue;
            }
            // Keep the non-overlapping head/tail slivers.
            if iv.0 < q.0 {
                out.push((iv.0, q.0));
            }
            if iv.1 > q.1 {
                out.push((q.1, iv.1));
            }
        }
        pr.intervals = out;
    }
}

/// Releases all tracked state for an exited process.
pub fn forget(pid: u64) {
    REGIONS.lock().remove(&pid);
}

/// Evicts the least-recently-touched process if inserting `pid` would exceed
/// the cap (and `pid` is not already tracked).
fn evict_if_needed(map: &mut BTreeMap<u64, ProcRegions>, pid: u64) {
    if map.len() < MAX_TRACKED_PIDS || map.contains_key(&pid) {
        return;
    }
    if let Some((&victim, _)) = map.iter().min_by_key(|(_, pr)| pr.touch) {
        map.remove(&victim);
    }
}
