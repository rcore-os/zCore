use core::fmt::{Result, Write};

use spin::Mutex;
use virtio_drivers::{VirtIOConsole as InnerDriver, VirtIOHeader};

use crate::scheme::{Scheme, UartScheme};
use crate::{utils::EventListener, DeviceResult};

pub struct VirtIoConsole<'a> {
    inner: Mutex<InnerDriver<'a>>,
    listener: Mutex<Option<EventListener>>,
}

impl<'a> VirtIoConsole<'a> {
    pub fn new(header: &'static mut VirtIOHeader) -> DeviceResult<Self> {
        Ok(Self {
            inner: Mutex::new(InnerDriver::new(header)?),
            listener: Mutex::new(None),
        })
    }
}

impl<'a> Scheme for VirtIoConsole<'a> {
    fn name(&self) -> &'static str {
        "virtio-console"
    }

    fn handle_irq(&self, _irq_num: usize) {
        self.inner.lock().ack_interrupt().unwrap();
        if let Some(l) = self.listener.lock().as_mut() {
            l.trigger();
        }
    }
}

impl<'a> UartScheme for VirtIoConsole<'a> {
    fn try_recv(&self) -> DeviceResult<Option<u8>> {
        Ok(self.inner.lock().recv(true)?)
    }

    fn send(&self, ch: u8) -> DeviceResult {
        self.inner.lock().send(ch)?;
        Ok(())
    }

    fn bind_listener(&self, listener: EventListener) {
        *self.listener.lock() = Some(listener);
    }
}

impl<'a> Write for VirtIoConsole<'a> {
    fn write_str(&mut self, s: &str) -> Result {
        for b in s.bytes() {
            self.send(b).unwrap()
        }
        Ok(())
    }
}
