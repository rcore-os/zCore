//! In-memory record of contained kernel faults, for `/proc/oops`.
//!
//! Once a fault is *contained* (see `zcore::oops`) the machine keeps running —
//! which is exactly when the report matters and exactly when it is easiest to
//! lose: the `[isolate]` lines go to the serial console, and a box with no
//! serial cable (or a user who scrolled past) has nothing left to read.
//!
//! # Why not write `/var/log/oops.log` directly
//!
//! Because the writer runs on the fault path: interrupts are off, the heap is
//! possibly the thing that just got smashed, and `oops` only proceeds when **no
//! kernel lock is held** — its central safety condition. Going through the VFS
//! would allocate, take filesystem and block-device locks, and block on I/O;
//! any one of those re-faults or deadlocks, turning a contained fault back into
//! a dead machine. That is the whole failure mode this subsystem exists to
//! prevent.
//!
//! So this module is the kernel half of the split Linux uses (`printk` ring →
//! `/proc/kmsg` → syslogd → `/var/log`): the fault path appends to a fixed
//! `static` byte buffer with **no allocation, no locks and no I/O**, and
//! userspace drains `/proc/oops` into `/var/log/oops.log` at its leisure. The
//! kernel survives the fault; the log survives the scrollback.
//!
//! Deliberately **fills and stops** rather than wrapping: the budget is bounded
//! anyway (`MAX_CONTAINED`), and a log that silently eats its own beginning is
//! worse than one that says it is full. Contents are lost on reboot — this
//! records faults the kernel *survived*, and userspace has seconds to minutes
//! to pick them up.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Room for well past `MAX_CONTAINED` events at a few hundred bytes each.
const CAP: usize = 16 * 1024;

/// The record itself. `AtomicU8` per byte so appends from a faulting CPU need
/// no lock — the one thing the fault path cannot take.
static BUF: [core::sync::atomic::AtomicU8; CAP] =
    [const { core::sync::atomic::AtomicU8::new(0) }; CAP];

/// Bytes claimed so far. Reservations are `fetch_add`, so two CPUs containing
/// at once get disjoint ranges instead of interleaving mid-line.
static LEN: AtomicUsize = AtomicUsize::new(0);

/// Set once the buffer fills, so readers know the tail is missing.
static OVERFLOWED: AtomicBool = AtomicBool::new(false);

/// A `fmt::Write` sink that appends into [`BUF`].
struct Sink;

impl Write for Sink {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        if bytes.is_empty() {
            return Ok(());
        }
        // Reserve first, then fill: the reservation is what makes concurrent
        // appends safe without a lock.
        let start = LEN.fetch_add(bytes.len(), Ordering::AcqRel);
        if start >= CAP {
            // Undo the runaway so `LEN` stays a usable length, and latch the
            // truncation for the reader.
            LEN.store(CAP, Ordering::Release);
            OVERFLOWED.store(true, Ordering::Release);
            return Ok(());
        }
        let end = (start + bytes.len()).min(CAP);
        if end < start + bytes.len() {
            OVERFLOWED.store(true, Ordering::Release);
            LEN.store(CAP, Ordering::Release);
        }
        for (i, b) in bytes.iter().take(end - start).enumerate() {
            BUF[start + i].store(*b, Ordering::Relaxed);
        }
        Ok(())
    }
}

/// Append a formatted line to the in-memory oops record.
///
/// Callable from the fault path: allocation-free, lock-free, and it never
/// touches a device. Failure to format is ignored — losing a log line must
/// never be able to escalate into a second fault.
pub fn record(args: fmt::Arguments) {
    let _ = Sink.write_fmt(args);
}

/// Whether anything has been recorded since boot.
pub fn is_empty() -> bool {
    LEN.load(Ordering::Acquire) == 0
}

/// Copy the record out as bytes. Called from ordinary process context by
/// `/proc/oops`, where allocating is fine.
pub fn snapshot() -> alloc::vec::Vec<u8> {
    let len = LEN.load(Ordering::Acquire).min(CAP);
    let mut out = alloc::vec::Vec::with_capacity(len + 96);
    for b in BUF.iter().take(len) {
        out.push(b.load(Ordering::Relaxed));
    }
    if OVERFLOWED.load(Ordering::Acquire) {
        out.extend_from_slice(
            b"\n[oops-log] record full; later events were dropped (raise CAP in \
              kernel-hal/src/common/oops_log.rs)\n",
        );
    }
    out
}
