use alloc::{boxed::Box, vec::Vec};

use lock::Mutex;

/// A type alias for the closure to handle device event.
pub type EventHandler<T = ()> = Box<dyn Fn(&T) + Send + Sync>;

/// Upper bound on the number of pending one-shot (`once`) handlers kept at a
/// time. Each `poll(2)`/`select(2)` iteration on an input device fd registers a
/// fresh one-shot waker; while the device is idle nothing fires them (there is
/// no event to drain the list), so without a cap they accumulate without bound
/// and every later `trigger` pays an O(n) walk over the whole list — under the
/// lock, and from IRQ context for HID. Dropping the oldest stale waker is safe:
/// the io-wait loop re-polls on a timer, so at worst a missed wake costs one
/// tick of latency.
const MAX_ONCE_HANDLERS: usize = 64;

/// Device event listener.
///
/// It keeps a series of [`EventHandler`]s that handle events of one single type.
pub struct EventListener<T = ()> {
    events: Mutex<Vec<(u64, EventHandler<T>, bool)>>,
    next_id: Mutex<u64>,
}

impl<T> EventListener<T> {
    /// Construct a new, empty `EventListener`.
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            next_id: Mutex::new(0),
        }
    }

    /// Register a new `handler` into this `EventListener`.
    ///
    /// If `once` is `true`, the `handler` will be removed once it handles an event.
    /// One-shot handlers are capped at [`MAX_ONCE_HANDLERS`] to keep an idle
    /// device's waker list from growing without bound (see the constant docs);
    /// persistent (`once == false`) handlers are never dropped.
    ///
    /// Returns a subscription id for [`Self::unsubscribe`]. Callers that park
    /// a waker in an async future **must** unsubscribe on Ready/`Drop`, or a
    /// later `trigger` wakes freed task memory (UAF → delayed KERNEL PAGE FAULT
    /// after a long compositor session).
    pub fn subscribe(&self, handler: EventHandler<T>, once: bool) -> Option<u64> {
        let mut events = self.events.lock();
        if once {
            let once_count = events.iter().filter(|(_, _, o)| *o).count();
            if once_count >= MAX_ONCE_HANDLERS {
                if let Some(pos) = events.iter().position(|(_, _, o)| *o) {
                    drop(events.remove(pos));
                }
            }
        }
        let mut next = self.next_id.lock();
        let id = *next;
        *next = next.wrapping_add(1);
        events.push((id, handler, once));
        Some(id)
    }

    /// Remove a previously registered handler by id (no-op if already fired).
    pub fn unsubscribe(&self, id: u64) {
        self.events.lock().retain(|(item_id, _, _)| *item_id != id);
    }

    /// Send an event to the `EventListener`.
    ///
    /// All the handlers handle the event, and those marked `once` will be removed immediately.
    /// Handlers whose fat pointer is clearly dead (null data or vtable, or a
    /// vtable that is not a kernel address — typical after a UAF of a once-waker
    /// or a coroutine stack overflow that smashes the heap) are skipped and
    /// *leaked*: calling them OR running `Drop` would both be null-range EXECUTE
    /// from IRQ/`timer_tick` → xHCI poll with no current thread.
    pub fn trigger(&self, event: T) {
        let mut guard = self.events.lock();
        let mut kept = Vec::with_capacity(guard.len());
        for (id, f, once) in guard.drain(..) {
            if !handler_fat_ptr_live(&f) {
                // Do not Drop: the destructor goes through the same null vtable.
                core::mem::forget(f);
                continue;
            }
            f(&event);
            if !once {
                kept.push((id, f, once));
            }
        }
        *guard = kept;
    }
}

/// `Box<dyn Fn…>` has the same representation as a trait-object fat pointer
/// `(data, vtable)`. Both words must be non-null and the vtable must look like
/// a real kernel address for a live handler.
///
/// A null data or vtable is the classic post-UAF residue that became `call *0`.
///
/// A non-null but non-kernel vtable (e.g. `0x13446` — the low 32 bits of a
/// `.text` address left behind by a coroutine stack overflow that smashed the
/// heap) also produces a null EXECUTE #PF: `f(&event)` dereferences the fake
/// vtable, finds zeros at the call-offset, and executes `call *0x0`. This fault
/// fires from any IRQ path (PS/2, UART, …) with `in_timer_callback=false` and
/// `no current thread`, bypassing the timer-tick guard in `irq_should_skip_heavy_work`.
/// Checking that the vtable high bits match the kernel address space catches
/// these corrupted-but-non-null pointers before the indirect call is attempted.
/// The kernel-range check is skipped in test builds (which run in user-space
/// where vtable addresses do not carry kernel high bits).
#[inline]
fn handler_fat_ptr_live<T>(f: &EventHandler<T>) -> bool {
    #[repr(C)]
    struct FatPtr {
        data: *const (),
        vtable: *const (),
    }
    // SAFETY: `EventHandler` is `Box<dyn Fn(&T) + Send + Sync>`; its value
    // representation is a two-word fat pointer. We only read the words.
    let fat: FatPtr = unsafe { core::mem::transmute_copy(f) };
    if fat.data.is_null() || fat.vtable.is_null() {
        return false;
    }
    // Vtables live in kernel `.rodata`; their address must carry the kernel
    // high-bits signature. A heap-smash residue such as `0x13446` (truncated
    // `.text` address, upper 32 bits zero) is non-null but not a kernel
    // pointer and must be rejected. Not checked in test builds: vtables in
    // a user-space test binary do not carry kernel high bits.
    #[cfg(all(target_arch = "x86_64", not(test)))]
    {
        let vtable_addr = fat.vtable as u64;
        if (vtable_addr >> 32) != 0xffff_ff00 {
            return false;
        }
    }
    true
}

impl<T> Default for EventListener<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, feature = "mock"))]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicUsize, Ordering};

    /// Idle poll/epoll used to park a fresh once-waker every re-scan; without
    /// unsubscribe that filled the listener until trigger UAF'd freed tasks
    /// (~30–40 min desktop sessions). Cap + unsubscribe must keep growth flat.
    #[test]
    fn once_handlers_capped() {
        let listener = EventListener::<()>::new();
        for _ in 0..(MAX_ONCE_HANDLERS + 100) {
            let _ = listener.subscribe(Box::new(|_| {}), true);
        }
        let n = listener.events.lock().iter().filter(|(_, _, o)| *o).count();
        assert!(
            n <= MAX_ONCE_HANDLERS,
            "once handlers must stay ≤ {}, got {}",
            MAX_ONCE_HANDLERS,
            n
        );
    }

    #[test]
    fn unsubscribe_clears_parked_once_handler() {
        let listener = EventListener::<()>::new();
        let id = listener.subscribe(Box::new(|_| {}), true).unwrap();
        assert_eq!(listener.events.lock().len(), 1);
        listener.unsubscribe(id);
        assert_eq!(listener.events.lock().len(), 0);
    }

    #[test]
    fn subscribe_drop_cycle_does_not_accumulate() {
        let listener = EventListener::<()>::new();
        for _ in 0..10_000 {
            let id = listener.subscribe(Box::new(|_| {}), true).unwrap();
            listener.unsubscribe(id);
        }
        assert_eq!(
            listener.events.lock().len(),
            0,
            "park+unsubscribe cycles must leave the listener empty"
        );
    }

    #[test]
    fn trigger_removes_once_and_keeps_persistent() {
        let hits = Arc::new(AtomicUsize::new(0));
        let listener = EventListener::<()>::new();
        let h = hits.clone();
        let _ = listener.subscribe(Box::new(move |_| { h.fetch_add(1, Ordering::SeqCst); }), false);
        let h2 = hits.clone();
        let _ = listener.subscribe(Box::new(move |_| { h2.fetch_add(1, Ordering::SeqCst); }), true);
        listener.trigger(());
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert_eq!(listener.events.lock().len(), 1, "persistent handler remains");
        listener.trigger(());
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }
}
