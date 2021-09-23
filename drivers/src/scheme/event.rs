use alloc::vec::Vec;

use spin::Mutex;

use super::{IrqHandler, Scheme, UartScheme};
use crate::DeviceResult;

pub struct EventListener<T: Scheme> {
    inner: T,
    events: Mutex<Vec<(IrqHandler, bool)>>,
}

impl<T: Scheme> EventListener<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            events: Mutex::new(Vec::new()),
        }
    }
}

impl<T: Scheme> Scheme for EventListener<T> {
    fn handle_irq(&self, irq_num: usize) {
        self.inner.handle_irq(irq_num);
        self.events.lock().retain(|(f, once)| {
            f();
            !once
        });
    }

    fn subscribe(&self, handler: IrqHandler, once: bool) {
        self.events.lock().push((handler, once));
    }
}

impl<T: UartScheme> UartScheme for EventListener<T> {
    fn try_recv(&self) -> DeviceResult<Option<u8>> {
        self.inner.try_recv()
    }
    fn send(&self, ch: u8) -> DeviceResult {
        self.inner.send(ch)
    }
    fn write_str(&self, s: &str) -> DeviceResult {
        self.inner.write_str(s)
    }
}
