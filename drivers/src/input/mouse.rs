use alloc::{boxed::Box, sync::Arc};

use spin::Mutex;

use crate::prelude::{CapabilityType, InputEvent, InputEventType};
use crate::scheme::{impl_event_scheme, InputScheme};
use crate::utils::EventListener;

bitflags::bitflags! {
    #[derive(Default)]
    pub struct MouseFlags: u8 {
        /// Whether or not the left mouse button is pressed.
        const LEFT_BTN = 1 << 0;
        /// Whether or not the right mouse button is pressed.
        const RIGHT_BTN = 1 << 1;
        /// Whether or not the middle mouse button is pressed.
        const MIDDLE_BTN = 1 << 2;
        /// Whether or not the packet is valid or not.
        const ALWAYS_ONE = 1 << 3;
        /// Whether or not the x delta is negative.
        const X_SIGN = 1 << 4;
        /// Whether or not the y delta is negative.
        const Y_SIGN = 1 << 5;
    }
}

#[derive(Default, Debug, Clone, Copy)]
pub struct MouseState {
    pub dx: i32,
    pub dy: i32,
    pub dz: i32,
    pub buttons: MouseFlags,
}

impl MouseState {
    pub fn as_ps2_buf(&self) -> [u8; 3] {
        let mut flags = self.buttons | MouseFlags::ALWAYS_ONE;
        let dx = self.dx.max(-127).min(127);
        let dy = self.dy.max(-127).min(127);
        if dx < 0 {
            flags |= MouseFlags::X_SIGN;
        }
        if dy < 0 {
            flags |= MouseFlags::Y_SIGN;
        }
        [flags.bits(), dx as u8, dy as u8]
    }
}

impl MouseState {
    fn update(&mut self, e: &InputEvent) -> Option<MouseState> {
        match e.event_type {
            InputEventType::Syn => {
                use super::input_event_codes::syn::*;
                if e.code == SYN_REPORT {
                    let saved = *self;
                    self.dx = 0;
                    self.dy = 0;
                    self.dz = 0;
                    return Some(saved);
                }
            }
            InputEventType::Key => {
                use super::input_event_codes::key::*;
                let btn = match e.code {
                    BTN_LEFT => MouseFlags::LEFT_BTN,
                    BTN_RIGHT => MouseFlags::RIGHT_BTN,
                    BTN_MIDDLE => MouseFlags::MIDDLE_BTN,
                    _ => return None,
                };
                if e.value == 0 {
                    self.buttons -= btn;
                } else {
                    self.buttons |= btn;
                }
            }
            InputEventType::RelAxis => {
                use super::input_event_codes::rel::*;
                match e.code {
                    REL_X => self.dx += e.value,
                    REL_Y => self.dy -= e.value,
                    REL_WHEEL => self.dz -= e.value,
                    _ => {}
                }
            }
            _ => {}
        }
        None
    }
}

pub struct Mouse {
    listener: EventListener<MouseState>,
    state: Mutex<MouseState>,
}

impl_event_scheme!(Mouse, MouseState);

impl Mouse {
    pub fn new(input: Arc<dyn InputScheme>) -> Arc<Self> {
        let ret = Arc::new(Self {
            listener: EventListener::new(),
            state: Mutex::new(MouseState::default()),
        });
        let cloned = ret.clone();
        input.subscribe(Box::new(move |e| cloned.handle_input_event(e)), false);
        ret
    }

    fn handle_input_event(&self, e: &InputEvent) {
        if let Some(p) = self.state.lock().update(e) {
            self.listener.trigger(p);
        }
    }

    pub fn compatible_with(input: &Arc<dyn InputScheme>) -> bool {
        // A mouse like device, at least one button, two relative axes.
        use super::input_event_codes::{ev::*, key::*, rel::*};
        let ev = input.capability(CapabilityType::Event);
        let key = input.capability(CapabilityType::Key);
        let rel = input.capability(CapabilityType::RelAxis);
        if !ev.contains_all(&[EV_KEY, EV_REL]) {
            return false;
        }
        if !key.contains(BTN_LEFT) {
            return false;
        }
        if !rel.contains_all(&[REL_X, REL_Y]) {
            return false;
        }
        true
    }
}
