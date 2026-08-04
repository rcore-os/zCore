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
    /// EventBus callback
    callbacks: Vec<EventHandler>,
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
    pub fn change(&mut self, reset: Event, set: Event) {
        let orig = self.event;
        let mut new = self.event;
        new.remove(reset);
        new.insert(set);
        self.event = new;
        if new != orig {
            self.callbacks.retain(|f| !f(new));
        }
    }

    /// The currently set event flags. Callers that need a race-free
    /// check-then-subscribe must hold the same lock that writers use to
    /// `set()`/`change()` the bus while calling this and `subscribe`.
    pub fn events(&self) -> Event {
        self.event
    }

    /// push a EventHandler into the callback vector
    pub fn subscribe(&mut self, callback: EventHandler) {
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
            return;
        }
        if self.callbacks.len() >= MAX_EVENT_CALLBACKS {
            // The table only fills on a long-idle bus being poll-scanned
            // (poll/select/epoll park a fresh waker per scan and drop none, so
            // orphaned entries pile up until the next event drains them). Evict
            // the OLDEST entry instead of ignoring the newcomer: silently
            // dropping the incoming subscription loses the wakeup of a waiter
            // that may have no other entry — a blocking socket read parked
            // here forever froze the whole compositor.
            trace!(
                "EventBus: callback table full ({}), evicting oldest",
                MAX_EVENT_CALLBACKS
            );
            let _evicted = self.callbacks.remove(0);
        }
        self.callbacks.push(callback);
    }

    /// get the callback vector length
    pub fn get_callback_len(&self) -> usize {
        self.callbacks.len()
    }
}

/// wait for a event async
pub fn wait_for_event(bus: Arc<Mutex<EventBus>>, mask: Event) -> impl Future<Output = Event> {
    EventBusFuture {
        bus,
        mask,
        subscribed: false,
    }
}

/// EventBus future for async
#[must_use = "future does nothing unless polled/`await`-ed"]
struct EventBusFuture {
    bus: Arc<Mutex<EventBus>>,
    mask: Event,
    subscribed: bool,
}

impl Future for EventBusFuture {
    type Output = Event;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let this = self.as_mut().get_mut();
        let mut lock = this.bus.lock();
        if !(lock.event & this.mask).is_empty() {
            return Poll::Ready(lock.event);
        }
        if !this.subscribed {
            this.subscribed = true;
            let waker = cx.waker().clone();
            let mask = this.mask;
            lock.subscribe(Box::new(move |s| {
                if (s & mask).is_empty() {
                    return false;
                }
                waker.wake_by_ref();
                true
            }));
        }
        Poll::Pending
    }
}
