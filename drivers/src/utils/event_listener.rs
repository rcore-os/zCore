use alloc::vec::Vec;

use crate::scheme::IrqHandler;

pub struct EventListener {
    events: Vec<(IrqHandler, bool)>,
}

impl EventListener {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    pub fn subscribe(&mut self, handler: IrqHandler, once: bool) {
        self.events.push((handler, once));
    }

    pub fn trigger(&mut self) {
        self.events.retain(|(f, once)| {
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
