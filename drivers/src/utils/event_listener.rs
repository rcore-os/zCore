use alloc::vec::Vec;

use spin::Mutex;

use crate::scheme::IrqHandler;

pub struct EventListener {
    events: Mutex<Vec<(IrqHandler, bool)>>,
}

impl EventListener {
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    pub fn subscribe(&self, handler: IrqHandler, once: bool) {
        self.events.lock().push((handler, once));
    }

    pub fn trigger(&self) {
        self.events.lock().retain(|(f, once)| {
            f();
            !once
        });
    }
}

impl Default for EventListener {
    fn default() -> Self {
        Self::new()
    }
}
