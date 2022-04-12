use alloc::{boxed::Box, vec::Vec};

use lock::Mutex;

/// A type alias for the closure to handle device event.
pub type EventHandler<T = ()> = Box<dyn Fn(&T) + Send + Sync>;

/// Device event listener.
///
/// It keeps a series of [`EventHandler`]s that handle events of one single type.
pub struct EventListener<T = ()> {
    events: Mutex<Vec<(EventHandler<T>, bool)>>,
}

impl<T> EventListener<T> {
    /// Construct a new, empty `EventListener`.
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    /// Register a new `handler` into this `EventListener`.
    ///
    /// If `once` is `true`, the `handler` will be removed once it handles an event.
    pub fn subscribe(&self, handler: EventHandler<T>, once: bool) {
        self.events.lock().push((handler, once));
    }

    /// Send an event to the `EventListener`.
    ///
    /// All the handlers handle the event, and those marked `once` will be removed immediately.
    pub fn trigger(&self, event: T) {
        self.events.lock().retain(|(f, once)| {
            f(&event);
            !once
        });
    }
}

impl<T> Default for EventListener<T> {
    fn default() -> Self {
        Self::new()
    }
}
