use core::convert::TryFrom;

use spin::Mutex;
use virtio_drivers::{VirtIOHeader, VirtIOInput as InnerDriver};

use crate::prelude::{DeviceResult, InputEvent, InputEventType};
use crate::scheme::{InputScheme, Scheme};
use crate::utils::{EventHandler, EventListener};

pub struct VirtIoInput<'a> {
    inner: Mutex<InnerDriver<'a>>,
    listener: EventListener<InputEvent>,
}

impl<'a> VirtIoInput<'a> {
    pub fn new(header: &'static mut VirtIOHeader) -> DeviceResult<Self> {
        let inner = Mutex::new(InnerDriver::new(header)?);
        Ok(Self {
            inner,
            listener: EventListener::new(),
        })
    }
}

impl<'a> Scheme for VirtIoInput<'a> {
    fn name(&self) -> &str {
        "virtio-input"
    }

    fn handle_irq(&self, _irq_num: usize) {
        let mut inner = self.inner.lock();
        inner.ack_interrupt();
        while let Some(e) = inner.pop_pending_event() {
            if let Ok(event_type) = InputEventType::try_from(e.event_type) {
                self.listener.trigger(InputEvent {
                    event_type,
                    code: e.code,
                    value: e.value,
                });
            }
        }
    }
}

impl<'a> InputScheme for VirtIoInput<'a> {
    fn subscribe(&self, handler: EventHandler<InputEvent>, once: bool) {
        self.listener.subscribe(handler, once);
    }
}
