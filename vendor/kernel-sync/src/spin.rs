use core::{
    cell::UnsafeCell,
    default::Default,
    fmt,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, Ordering},
};

use crate::deadlock::report_deadlock;
use crate::interrupt::{pop_off, push_off};

pub struct SpinMutex<T: ?Sized> {
    locked: AtomicBool,
    /// Deadlock forensics: `file.as_ptr()` of the current holder's
    /// `#[track_caller]` acquire site (0 = unheld). See `TicketMutex` for the
    /// full rationale — a stuck waiter reports the HOLDER alongside itself.
    holder_file: core::sync::atomic::AtomicUsize,
    /// Byte length of the holder's file string.
    holder_file_len: core::sync::atomic::AtomicUsize,
    /// `(cpu << 32) | line` of the holder's acquire.
    holder_line_cpu: core::sync::atomic::AtomicUsize,
    data: UnsafeCell<T>,
}

/// An RAII implementation of a “scoped lock” of a mutex.
/// When this structure is dropped (falls out of scope),
/// the lock will be unlocked.
///
pub struct SpinMutexGuard<'a, T: ?Sized + 'a> {
    lock: &'a AtomicBool,
    /// Cleared on drop so the lock reads as unheld between owners.
    holder_file: &'a core::sync::atomic::AtomicUsize,
    data: &'a mut T,
}

unsafe impl<T: ?Sized + Send> Sync for SpinMutex<T> {}
unsafe impl<T: ?Sized + Send> Send for SpinMutex<T> {}

impl<T> SpinMutex<T> {
    #[inline(always)]
    pub const fn new(data: T) -> Self {
        SpinMutex {
            locked: AtomicBool::new(false),
            holder_file: core::sync::atomic::AtomicUsize::new(0),
            holder_file_len: core::sync::atomic::AtomicUsize::new(0),
            holder_line_cpu: core::sync::atomic::AtomicUsize::new(0),
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

impl<T: ?Sized> SpinMutex<T> {
    /// Record this acquire as the current holder (deadlock forensics). File
    /// ptr stored LAST so a snapshotter keying on `holder_file != 0` never
    /// reads a torn triple.
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

    #[inline(always)]
    #[track_caller]
    pub fn lock(&self) -> SpinMutexGuard<T> {
        push_off();
        let caller = core::panic::Location::caller();
        let mut spins: u64 = 0;
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // Wait until the lock looks unlocked before retrying
            while self.is_locked() {
                core::hint::spin_loop();
                spins += 1;
                // Spinning with IRQs off makes this CPU deaf to TLB-shootdown
                // IPIs, and a peer may be spin-waiting for our ack — drain our
                // queue at a coarse cadence, exactly as the ticket lock does.
                if spins & 511 == 0 {
                    crate::deadlock::spin_pump();
                }
                if spins == crate::deadlock::deadlock_spins() {
                    // Many seconds of continuous spinning with IRQs off: this
                    // CPU is almost certainly part of a deadlock. Self-report
                    // the stuck call site (once), then keep spinning — if the
                    // holder ever releases, we still proceed correctly.
                    report_deadlock(caller.file(), caller.line());
                    // Also report WHO holds the lock (see TicketMutex::lock).
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
        }
        self.record_holder(caller);
        SpinMutexGuard {
            lock: &self.locked,
            holder_file: &self.holder_file,
            data: unsafe { &mut *self.data.get() },
        }
    }

    #[inline]
    #[track_caller]
    pub fn try_lock(&self) -> Option<SpinMutexGuard<T>> {
        push_off();
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            self.record_holder(core::panic::Location::caller());
            Some(SpinMutexGuard {
                lock: &self.locked,
                holder_file: &self.holder_file,
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
        self.locked.load(Ordering::Relaxed)
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for SpinMutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.try_lock() {
            Some(guard) => write!(f, "Mutex {{ data: ")
                .and_then(|()| (&*guard).fmt(f))
                .and_then(|()| write!(f, "}}")),
            None => write!(f, "Mutex {{ <locked> }}"),
        }
    }
}

impl<T: ?Sized + Default> Default for SpinMutex<T> {
    fn default() -> Self {
        SpinMutex::new(T::default())
    }
}

impl<T> From<T> for SpinMutex<T> {
    fn from(data: T) -> Self {
        Self::new(data)
    }
}

impl<'a, T: ?Sized> Drop for SpinMutexGuard<'a, T> {
    /// The dropping of the SpinMutexGuard will release the lock it was created from.
    fn drop(&mut self) {
        // Clear the holder record BEFORE releasing, so a waiter's deadlock
        // snapshot never blames an owner that already released.
        self.holder_file.store(0, Ordering::Relaxed);
        self.lock.store(false, Ordering::Release);
        pop_off();
    }
}

impl<'a, T: ?Sized> Deref for SpinMutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.data
    }
}

impl<'a, T: ?Sized> DerefMut for SpinMutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.data
    }
}

impl<'a, T: ?Sized + fmt::Debug> fmt::Debug for SpinMutexGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'a, T: ?Sized + fmt::Display> fmt::Display for SpinMutexGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}
