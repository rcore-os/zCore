//! Event bus implement
//!
//! An Eventbus is a mechanism that allows different components to communicate with each other without knowing about each other.
use alloc::boxed::Box;
use alloc::{sync::Arc, vec::Vec};
use bitflags::bitflags;
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use lock::Mutex;

const MAX_EVENT_CALLBACKS: usize = 4096;

/// Cap on parked EventBus callbacks (see [`EventBus::subscribe`]). Exposed for
/// soak/regression tests that assert long poll/epoll sessions cannot fill the
/// bus via orphaned wakers.
#[cfg(test)]
pub(crate) const TEST_MAX_EVENT_CALLBACKS: usize = MAX_EVENT_CALLBACKS;

bitflags! {
    #[derive(Default)]
    /// event bus Event flags
    pub struct Event: u32 {
        /// File: is readable
        const READABLE                      = 1 << 0;
        /// File: is writeable
        const WRITABLE                      = 1 << 1;
        /// File: has error
        const ERROR                         = 1 << 2;
        /// File: is closed
        const CLOSED                        = 1 << 3;

        /// Process: is Quit
        const PROCESS_QUIT                  = 1 << 10;
        /// Process: child process is Quit
        const CHILD_PROCESS_QUIT            = 1 << 11;
        /// Process: received signal
        const RECEIVE_SIGNAL                = 1 << 12;

        /// Semaphore: is removed
        const SEMAPHORE_REMOVED             = 1 << 20;
        /// Semaphore: can acquired a resource of this semaphore
        const SEMAPHORE_CAN_ACQUIRE         = 1 << 21;
    }
}

/// handler of event in the event bus
pub type EventHandler = Box<dyn Fn(Event) -> bool + Send>;

/// event bus struct
#[derive(Default)]
pub struct EventBus {
    /// event type
    event: Event,
    /// EventBus callbacks paired with unique subscription IDs
    callbacks: Vec<(u64, EventHandler)>,
    /// counter for subscription IDs
    next_id: u64,
}
impl core::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EventBus")
            .field("event", &self.event)
            .field("callbacks_len", &self.callbacks.len())
            .finish()
    }
}

impl EventBus {
    /// create an event bus
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::default()))
    }

    /// set event flag
    pub fn set(&mut self, set: Event) {
        self.change(Event::empty(), set);
    }

    /// clear all event flag
    pub fn clear(&mut self, set: Event) {
        self.change(set, Event::empty());
    }

    /// change event flag
    /// - `reset`: flag to remove
    /// - `set`: flag to insert
    ///
    /// Callbacks are taken out of the table before firing so a re-entrant
    /// `set`/`change` on the same bus (common when a waker touches the fd again)
    /// cannot deadlock holding this mutex, and late `subscribe`s during fire
    /// are preserved.
    pub fn change(&mut self, reset: Event, set: Event) {
        let orig = self.event;
        let mut new = self.event;
        new.remove(reset);
        new.insert(set);
        self.event = new;
        if new != orig {
            let pending = core::mem::take(&mut self.callbacks);
            let mut kept = Vec::with_capacity(pending.len());
            for (id, f) in pending {
                if !f(new) {
                    kept.push((id, f));
                }
            }
            // Subscriptions that arrived while we were firing.
            let mut late = core::mem::take(&mut self.callbacks);
            kept.append(&mut late);
            self.callbacks = kept;
        }
    }

    /// The currently set event flags. Callers that need a race-free
    /// check-then-subscribe must hold the same lock that writers use to
    /// `set()`/`change()` the bus while calling this and `subscribe`.
    pub fn events(&self) -> Event {
        self.event
    }

    /// push a EventHandler into the callback vector, returning a subscription ID if registered
    pub fn subscribe(&mut self, callback: EventHandler) -> Option<u64> {
        // A subscriber arriving while events are already active must observe
        // them NOW, not wait for the next transition. `change` fires callbacks
        // only when the flag set CHANGES, and the flags are latched — so a
        // waiter that checked readiness, lost the race to a producer that set
        // the flag in between, and then subscribed, would otherwise sleep on an
        // event that is already on and will never re-fire: a second `set` of an
        // already-set flag is not a transition. That was observable as both
        // ends of a pipe ping-pong asleep forever (the reader missed READABLE
        // by microseconds; the writer then blocked reading the reply) with the
        // whole machine idle around them — a silent, un-diagnosable hang,
        // roughly once per two benchmark rounds under load.
        //
        // Same contract as `change`: a callback returning true is one-shot and
        // is not retained after firing.
        if !self.event.is_empty() && callback(self.event) {
            return None;
        }
        if self.callbacks.len() >= MAX_EVENT_CALLBACKS {
            // The table only fills on a long-idle bus being poll-scanned
            // (poll/select/epoll park a fresh waker per scan and drop none, so
            // orphaned entries pile up until the next event drains them). Evict
            // the OLDEST entry instead of ignoring the newcomer: silently
            // dropping the incoming subscription loses the wakeup of a waiter
            // that may have no other entry — a blocking socket read parked
            // here forever froze the whole compositor.
            //
            // Wake the oldest *before* drop: otherwise a live waiter whose
            // only subscription was evicted sleeps forever, and under Wayland
            // bring-up that pushed more timer_set backstops / heap churn.
            trace!(
                "EventBus: callback table full ({}), waking+evicting oldest",
                MAX_EVENT_CALLBACKS
            );
            let (_id, oldest) = self.callbacks.remove(0);
            let _ = oldest(self.event);
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.callbacks.push((id, callback));
        Some(id)
    }

    /// Unsubscribe a previously registered callback by its ID.
    pub fn unsubscribe(&mut self, id: u64) {
        self.callbacks.retain(|(item_id, _)| *item_id != id);
    }

    /// get the callback vector length
    pub fn get_callback_len(&self) -> usize {
        self.callbacks.len()
    }
}

/// RAII handle for a waker parked on some file's event source (see
/// `FileLike::subscribe_readiness`). Dropping it unregisters the waker so a
/// poller that stops waiting (completed, timed out, or was killed) does not
/// leave a callback behind to wake a recycled task slot later.
///
/// Carries a type-erased unsubscribe closure rather than an
/// `Arc<Mutex<EventBus>>` because the bus lives behind a different owner per
/// file type (a bare field inside the pipe's data mutex, an `Arc<Mutex<_>>`
/// on eventfd/timerfd, a global for the DRM device); each subscriber captures
/// whatever handle it needs.
pub struct ReadinessSub {
    unsub: Option<alloc::boxed::Box<dyn FnOnce() + Send>>,
}

impl ReadinessSub {
    /// A subscription whose cleanup runs `unsub` on drop.
    pub fn new(unsub: alloc::boxed::Box<dyn FnOnce() + Send>) -> Self {
        Self { unsub: Some(unsub) }
    }

    /// A subscription with nothing to clean up: the waker already fired at
    /// subscribe time (events were pending), so no callback was stored.
    pub fn noop() -> Self {
        Self { unsub: None }
    }
}

impl Drop for ReadinessSub {
    fn drop(&mut self) {
        if let Some(f) = self.unsub.take() {
            f();
        }
    }
}

/// Park `waker` on `bus` as a one-shot callback for any event in `mask`.
///
/// Returns the subscription id, or `None` when events in `mask` were already
/// pending — in that case the waker has been woken right here (the latched
/// flags + subscribe-time fire in [`EventBus::subscribe`] make
/// check-then-subscribe race-free) and nothing was stored.
pub fn subscribe_waker(
    bus: &mut EventBus,
    mask: Event,
    waker: &core::task::Waker,
) -> Option<u64> {
    let waker = waker.clone();
    bus.subscribe(Box::new(move |events| {
        if (events & mask).is_empty() {
            return false;
        }
        waker.wake_by_ref();
        true
    }))
}

/// [`subscribe_waker`] + RAII handle for the common `Arc<Mutex<EventBus>>`
/// bus owner (eventfd, timerfd, signalfd, pty buses, the DRM device bus).
pub fn subscribe_readiness_on(
    bus: &Arc<Mutex<EventBus>>,
    mask: Event,
    waker: &core::task::Waker,
) -> ReadinessSub {
    match subscribe_waker(&mut bus.lock(), mask, waker) {
        Some(id) => {
            let bus = bus.clone();
            ReadinessSub::new(Box::new(move || {
                bus.lock().unsubscribe(id);
            }))
        }
        None => ReadinessSub::noop(),
    }
}

/// wait for a event async
pub fn wait_for_event(bus: Arc<Mutex<EventBus>>, mask: Event) -> impl Future<Output = Event> {
    EventBusFuture {
        bus,
        mask,
        sub_id: None,
    }
}

/// EventBus future for async
#[must_use = "future does nothing unless polled/`await`-ed"]
struct EventBusFuture {
    bus: Arc<Mutex<EventBus>>,
    mask: Event,
    sub_id: Option<u64>,
}

impl Drop for EventBusFuture {
    fn drop(&mut self) {
        if let Some(id) = self.sub_id.take() {
            self.bus.lock().unsubscribe(id);
        }
    }
}

impl Future for EventBusFuture {
    type Output = Event;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let this = self.as_mut().get_mut();
        let mut lock = this.bus.lock();
        if !(lock.event & this.mask).is_empty() {
            if let Some(id) = this.sub_id.take() {
                lock.unsubscribe(id);
            }
            return Poll::Ready(lock.event);
        }
        if this.sub_id.is_none() {
            let waker = cx.waker().clone();
            let mask = this.mask;
            let sub_id = lock.subscribe(Box::new(move |s| {
                if (s & mask).is_empty() {
                    return false;
                }
                waker.wake_by_ref();
                true
            }));
            this.sub_id = sub_id;
        }
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicBool, Ordering};
    use core::task::{RawWaker, RawWakerVTable, Waker};
    use lock::Mutex;

    fn flag_waker(flag: &'static AtomicBool) -> Waker {
        fn raw(ptr: *const ()) -> RawWaker {
            unsafe fn clone(ptr: *const ()) -> RawWaker {
                raw(ptr)
            }
            unsafe fn wake(ptr: *const ()) {
                (*(ptr as *const AtomicBool)).store(true, Ordering::SeqCst);
            }
            unsafe fn wake_by_ref(ptr: *const ()) {
                (*(ptr as *const AtomicBool)).store(true, Ordering::SeqCst);
            }
            unsafe fn drop(_: *const ()) {}
            RawWaker::new(ptr, &RawWakerVTable::new(clone, wake, wake_by_ref, drop))
        }
        unsafe { Waker::from_raw(raw(flag as *const AtomicBool as *const ())) }
    }

    /// Simulate ~minutes of poll/epoll re-scans: each parks then drops.
    /// Without Drop-unsubscribe this would climb to [`MAX_EVENT_CALLBACKS`] and
    /// then thrash (the 30–40 min KERNEL PAGE FAULT class).
    #[test]
    fn wait_for_event_drop_does_not_fill_bus() {
        static WOKE: AtomicBool = AtomicBool::new(false);
        let bus = EventBus::new();
        let waker = flag_waker(&WOKE);
        let mut cx = Context::from_waker(&waker);

        // Compressed soak: 50k cycles ≫ 4096 cap; must stay near-empty.
        for _ in 0..50_000 {
            let mut fut = wait_for_event(bus.clone(), Event::READABLE);
            assert!(matches!(Pin::new(&mut fut).poll(&mut cx), Poll::Pending));
            assert_eq!(bus.lock().get_callback_len(), 1);
            drop(fut);
            assert_eq!(
                bus.lock().get_callback_len(),
                0,
                "Drop must clear the parked waiter every cycle"
            );
        }
        assert!(!WOKE.load(Ordering::SeqCst));
    }

    #[test]
    fn subscribe_cap_never_exceeds_max() {
        let mut bus = EventBus::default();
        for _ in 0..(TEST_MAX_EVENT_CALLBACKS + 200) {
            let _ = bus.subscribe(Box::new(|_| false));
        }
        assert!(
            bus.get_callback_len() <= TEST_MAX_EVENT_CALLBACKS,
            "callback table must stay ≤ {}, got {}",
            TEST_MAX_EVENT_CALLBACKS,
            bus.get_callback_len()
        );
    }

    #[test]
    fn unsubscribe_removes_exact_id() {
        let mut bus = EventBus::default();
        let a = bus.subscribe(Box::new(|_| false)).unwrap();
        let b = bus.subscribe(Box::new(|_| false)).unwrap();
        assert_eq!(bus.get_callback_len(), 2);
        bus.unsubscribe(a);
        assert_eq!(bus.get_callback_len(), 1);
        bus.unsubscribe(b);
        assert_eq!(bus.get_callback_len(), 0);
    }

    #[test]
    fn wait_for_event_ready_unsubscribes() {
        static WOKE: AtomicBool = AtomicBool::new(false);
        let bus: Arc<Mutex<EventBus>> = EventBus::new();
        bus.lock().set(Event::READABLE);
        let waker = flag_waker(&WOKE);
        let mut cx = Context::from_waker(&waker);
        let mut fut = wait_for_event(bus.clone(), Event::READABLE);
        assert!(matches!(
            Pin::new(&mut fut).poll(&mut cx),
            Poll::Ready(Event::READABLE)
        ));
        assert_eq!(bus.lock().get_callback_len(), 0);
    }
}
