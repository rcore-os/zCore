use core::convert::TryFrom;

use spin::Mutex;
use virtio_drivers::{InputConfigSelect, VirtIOHeader, VirtIOInput as InnerDriver};

use crate::prelude::{CapabilityType, InputCapability, InputEvent, InputEventType};
use crate::scheme::{impl_event_scheme, InputScheme, Scheme};
use crate::utils::EventListener;
use crate::DeviceResult;

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

impl_event_scheme!(VirtIoInput<'_>, InputEvent);

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
                    value: e.value as i32,
                });
            }
        }
    }
}

impl<'a> InputScheme for VirtIoInput<'a> {
    fn capability(&self, cap_type: CapabilityType) -> InputCapability {
        let mut inner = self.inner.lock();
        let mut bitmap = [0u8; 128];
        match cap_type {
            CapabilityType::InputProp => {
                let size = inner.query_config_select(InputConfigSelect::PropBits, 0, &mut bitmap);
                InputCapability::from_bitmap(&bitmap[..size as usize])
            }
            CapabilityType::Event => {
                let mut cap = InputCapability::empty();
                for i in 0..crate::input::input_event_codes::ev::EV_CNT {
                    let size =
                        inner.query_config_select(InputConfigSelect::EvBits, i as u8, &mut bitmap);
                    if size > 0 {
                        cap.set(i);
                    }
                }
                cap
            }
            _ => {
                let size = inner.query_config_select(
                    InputConfigSelect::EvBits,
                    cap_type as u8,
                    &mut bitmap,
                );
                InputCapability::from_bitmap(&bitmap[..size as usize])
            }
        }
    }
}
