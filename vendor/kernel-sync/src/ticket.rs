use core::{
    cell::UnsafeCell,
    default::Default,
    fmt,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::deadlock::report_deadlock;
use crate::interrupt::{pop_off, push_off};

pub struct TicketMutex<T: ?Sized> {
    next_ticket: AtomicUsize,
    next_serving: AtomicUsize,
    /// Deadlock forensics: `file.as_ptr()` of the current holder's
    /// `#[track_caller]` acquire site (0 = unheld). A waiter that crosses
    /// the deadlock spin threshold snapshots these and reports the HOLDER alongside its
    /// own stuck site — the spinners alone never identify the culprit (they
    /// are usually innocent readers; the interesting party is whoever took
    /// the lock and never released it).
    holder_file: AtomicUsize,
    /// Byte length of the holder's file string.
    holder_file_len: AtomicUsize,
    /// `(cpu << 32) | line` of the holder's acquire. The cpu is the one that
    /// ACQUIRED the lock; if the owning task migrated since, the acquire site
    /// is still the part that identifies the code path.
    holder_line_cpu: AtomicUsize,
    data: UnsafeCell<T>,
}

/// An RAII implementation of a “scoped lock” of a mutex.
/// When this structure is dropped (falls out of scope),
/// the lock will be unlocked.
///
pub struct TicketMutexGuard<'a, T: ?Sized + 'a> {
    next_serving: &'a AtomicUsize,
    /// Cleared on drop so the lock reads as unheld between owners.
    holder_file: &'a AtomicUsize,
    ticket: usize,
    data: &'a mut T,
}

unsafe impl<T: ?Sized + Send> Sync for TicketMutex<T> {}
unsafe impl<T: ?Sized + Send> Send for TicketMutex<T> {}

impl<T> TicketMutex<T> {
    #[inline(always)]
    pub const fn new(data: T) -> Self {
        TicketMutex {
            next_ticket: AtomicUsize::new(0),
            next_serving: AtomicUsize::new(0),
            holder_file: AtomicUsize::new(0),
            holder_file_len: AtomicUsize::new(0),
            holder_line_cpu: AtomicUsize::new(0),
            data: UnsafeCell::new(data),
        }
    }

    #[inline(always)]
    pub fn into_inner(self) -> T {
        // We know statically that there are no outstanding references to
        // `self` so there's no need to lock.
        self.data.into_inner()
    }

    #[inline(always)]
    pub fn as_mut_ptr(&self) -> *mut T {
        self.data.get()
    }
}

impl<T: ?Sized> TicketMutex<T> {
    /// Record this acquire as the current holder (for deadlock forensics).
    /// File ptr is stored LAST so a concurrent snapshotter that keys on
    /// `holder_file != 0` never reads a torn (ptr, len, line) triple.
    #[inline(always)]
    fn record_holder(&self, caller: &'static core::panic::Location<'static>) {
        let file = caller.file();
        self.holder_file_len.store(file.len(), Ordering::Relaxed);
        self.holder_line_cpu.store(
            ((crate::interrupt::current_cpu_id() as usize) << 32) | caller.line() as usize,
            Ordering::Relaxed,
        );
        self.holder_file
            .store(file.as_ptr() as usize, Ordering::Release);
    }

    #[track_caller]
    pub fn lock(&self) -> TicketMutexGuard<T> {
        push_off();
        let caller = core::panic::Location::caller();
        let mut spins: u64 = 0;
        let ticket = self.next_ticket.fetch_add(1, Ordering::Relaxed);
        while self.next_serving.load(Ordering::Acquire) != ticket {
            core::hint::spin_loop();
            spins += 1;
            // Spinning with IRQs off makes this CPU deaf to TLB-shootdown
            // IPIs, and someone else is spin-waiting for our ack — see
            // `set_spin_pump`. Drain our queue at a cadence that keeps ack
            // latency in microseconds while costing one relaxed load per 512
            // spins when there is nothing pending.
            if spins & 511 == 0 {
                crate::deadlock::spin_pump();
            }
            // Same-cpu re-entrancy is never contention: a ticket lock is not
            // re-entrant, so if the recorded holder is THIS cpu, this acquire
            // can only be nested inside the holder's critical section and will
            // spin forever. Detect it after a token number of spins (a real
            // release needs none of this cpu's cycles, so a handful of spins
            // with ourselves as holder is already conclusive) and report both
            // sites immediately instead of after the multi-second threshold —
            // turning a probabilistic wedge into an instant, named report.
            if spins == 1024 || spins == 131_072 {
                // Two sightings, well apart, both naming THIS cpu as holder.
                // One read is not proof: the holder record is stamped after the
                // acquire and cleared on release, so a single read can catch
                // the previous owner's stale record — and this cpu may BE that
                // previous owner. A same-cpu record still standing after 130k
                // further spins cannot be stale (a release needs none of our
                // cycles and clears it; a new owner restamps it), so only then
                // is it reported. Observed before the re-check: a banner with
                // holder==waiter and the run completing anyway.
                let hf = self.holder_file.load(Ordering::Acquire);
                if hf != 0 {
                    let lc = self.holder_line_cpu.load(Ordering::Relaxed);
                    if (lc >> 32) as u32 == crate::interrupt::current_cpu_id() as u32
                        && spins == 131_072
                    {
                        report_deadlock(caller.file(), caller.line());
                        let hl = self.holder_file_len.load(Ordering::Relaxed);
                        crate::deadlock::report_deadlock_holder(
                            hf,
                            hl,
                            (lc & 0xffff_ffff) as u32,
                            (lc >> 32) as u32,
                        );
                    }
                }
            }
            if spins == crate::deadlock::deadlock_spins() {
                // Many seconds of continuous spinning with IRQs off: this CPU
                // is almost certainly part of a deadlock. Self-report the stuck
                // call site (once), then keep spinning — if the holder ever
                // releases, behavior is unchanged.
                report_deadlock(caller.file(), caller.line());
                // Also report WHO holds the lock we are stuck on: after ~8s of
                // continuous spinning the recorded holder is the wedged owner,
                // not some transient contender. Best effort — a release racing
                // this snapshot just reads as "no holder" and is skipped.
                let hf = self.holder_file.load(Ordering::Acquire);
                if hf != 0 {
                    let hl = self.holder_file_len.load(Ordering::Relaxed);
                    let lc = self.holder_line_cpu.load(Ordering::Relaxed);
                    crate::deadlock::report_deadlock_holder(
                        hf,
                        hl,
                        (lc & 0xffff_ffff) as u32,
                        (lc >> 32) as u32,
                    );
                }
            }
        }
        self.record_holder(caller);
        TicketMutexGuard {
            next_serving: &self.next_serving,
            holder_file: &self.holder_file,
            ticket,
            // Safety
            // We know that we are the next ticket to be served,
            // so there's no other thread accessing the data.
            //
            // Every other thread has another ticket number so it's
            // definitely stuck in the spin loop above.
            data: unsafe { &mut *self.data.get() },
        }
    }

    #[inline]
    #[track_caller]
    pub fn try_lock(&self) -> Option<TicketMutexGuard<T>> {
        push_off();
        let ticket = self
            .next_ticket
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |ticket| {
                if self.next_serving.load(Ordering::Acquire) == ticket {
                    Some(ticket + 1)
                } else {
                    None
                }
            });
        if let Ok(ticket) = ticket {
            self.record_holder(core::panic::Location::caller());
            Some(TicketMutexGuard {
                next_serving: &self.next_serving,
                holder_file: &self.holder_file,
                ticket,
                // Safety
                // We have a ticket that is equal to the next_serving ticket, so we know:
                // - that no other thread can have the same ticket id as this thread
                // - that we are the next one to be served so we have exclusive access to the data
                data: unsafe { &mut *self.data.get() },
            })
        } else {
            pop_off();
            None
        }
    }

    #[inline(always)]
    pub fn get_mut(&mut self) -> &mut T {
        // We know statically that there are no other references to `self`, so
        // there's no need to lock the inner mutex.
        unsafe { &mut *self.data.get() }
    }

    #[inline(always)]
    pub fn is_locked(&self) -> bool {
        let ticket = self.next_ticket.load(Ordering::Relaxed);
        self.next_serving.load(Ordering::Relaxed) != ticket
    }
}

impl<'a, T: ?Sized> Drop for TicketMutexGuard<'a, T> {
    /// The dropping of the TicketMutexGuard will release the lock it was created from.
    fn drop(&mut self) {
        // Clear the holder record BEFORE handing the lock over, so a waiter's
        // deadlock snapshot never attributes the lock to an owner that
        // already released it. The Release store below publishes this too.
        self.holder_file.store(0, Ordering::Relaxed);
        let new_ticket = self.ticket + 1;
        self.next_serving.store(new_ticket, Ordering::Release);
        pop_off();
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for TicketMutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.try_lock() {
            Some(guard) => write!(f, "Mutex {{ data: ")
                .and_then(|()| (&*guard).fmt(f))
                .and_then(|()| write!(f, "}}")),
            None => write!(f, "Mutex {{ <locked> }}"),
        }
    }
}

impl<T: ?Sized + Default> Default for TicketMutex<T> {
    fn default() -> Self {
        TicketMutex::new(T::default())
    }
}

impl<T> From<T> for TicketMutex<T> {
    fn from(data: T) -> Self {
        Self::new(data)
    }
}

impl<'a, T: ?Sized + fmt::Display> fmt::Display for TicketMutexGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<'a, T: ?Sized + fmt::Debug> fmt::Debug for TicketMutexGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'a, T: ?Sized> Deref for TicketMutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.data
    }
}

impl<'a, T: ?Sized> DerefMut for TicketMutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.data
    }
}
