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

    /// push a EventHandler into the callback vector
    pub fn subscribe(&mut self, callback: EventHandler) {
        self.callbacks.push(callback);
    }

    /// get the callback vector length
    pub fn get_callback_len(&self) -> usize {
        self.callbacks.len()
    }
}

/// wait for a event async
pub fn wait_for_event(bus: Arc<Mutex<EventBus>>, mask: Event) -> impl Future<Output = Event> {
    EventBusFuture { bus, mask }
}

/// EventBus future for async
#[must_use = "future does nothing unless polled/`await`-ed"]
struct EventBusFuture {
    bus: Arc<Mutex<EventBus>>,
    mask: Event,
}

impl Future for EventBusFuture {
    type Output = Event;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let mut lock = self.bus.lock();
        if !(lock.event & self.mask).is_empty() {
            return Poll::Ready(lock.event);
        }
        let waker = cx.waker().clone();
        let mask = self.mask;
        lock.subscribe(Box::new(move |s| {
            if (s & mask).is_empty() {
                return false;
            }
            waker.wake_by_ref();
            true
        }));
        Poll::Pending
    }
}
