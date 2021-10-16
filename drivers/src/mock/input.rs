use crate::input::input_event_codes::{ev::*, key::*, rel::*};
use crate::prelude::{CapabilityType, InputCapability, InputEvent};
use crate::scheme::{impl_event_scheme, InputScheme, Scheme};
use crate::utils::EventListener;

#[derive(Default)]
pub struct MockMouse {
    listener: EventListener<InputEvent>,
}

impl_event_scheme!(MockMouse, InputEvent);

impl Scheme for MockMouse {
    fn name(&self) -> &str {
        "mock-mouse-input"
    }
}

impl InputScheme for MockMouse {
    fn capability(&self, cap_type: CapabilityType) -> InputCapability {
        let mut cap = InputCapability::empty();
        match cap_type {
            CapabilityType::Event => cap.set_all(&[EV_KEY, EV_REL]),
            CapabilityType::Key => cap.set_all(&[BTN_LEFT, BTN_RIGHT, BTN_MIDDLE]),
            CapabilityType::RelAxis => cap.set_all(&[REL_X, REL_Y, REL_HWHEEL]),
            _ => {}
        }
        cap
    }
}

#[derive(Default)]
pub struct MockKeyboard {
    listener: EventListener<InputEvent>,
}

impl_event_scheme!(MockKeyboard, InputEvent);

impl Scheme for MockKeyboard {
    fn name(&self) -> &str {
        "mock-keyboard-input"
    }
}

impl InputScheme for MockKeyboard {
    fn capability(&self, cap_type: CapabilityType) -> InputCapability {
        let mut cap = InputCapability::empty();
        match cap_type {
            CapabilityType::Event => cap.set(EV_KEY),
            CapabilityType::Key => {
                // TODO
            }
            _ => {}
        }
        cap
    }
}
