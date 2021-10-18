use alloc::{boxed::Box, vec::Vec};

use spin::Mutex;

pub type EventHandler<T = ()> = Box<dyn Fn(&T) + Send + Sync>;

pub struct EventListener<T = ()> {
    events: Mutex<Vec<(EventHandler<T>, bool)>>,
}

impl<T> EventListener<T> {
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    pub fn subscribe(&self, handler: EventHandler<T>, once: bool) {
        self.events.lock().push((handler, once));
    }

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
