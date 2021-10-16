use core::fmt;

use super::{event::EventScheme, Scheme};
use crate::input::input_event_codes::ev::*;

numeric_enum_macro::numeric_enum! {
    #[repr(u16)]
    #[derive(Clone, Copy, Debug)]
    /// Linux input event codes.
    ///
    /// Reference: <https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/uapi/linux/input-event-codes.h>
    pub enum InputEventType {
        /// Used as markers to separate events. Events may be separated in time or in space,
        /// such as with the multitouch protocol.
        Syn = EV_SYN,
        /// Used to describe state changes of keyboards, buttons, or other key-like devices.
        Key = EV_KEY,
        /// Used to describe relative axis value changes, e.g. moving the mouse 5 units
        /// to the left.
        RelAxis = EV_REL,
        /// Used to describe absolute axis value changes, e.g. describing the coordinates
        /// of a touch on a touchscreen.
        AbsAxis = EV_ABS,
        /// Used to describe miscellaneous input data that do not fit into other types.
        Misc = EV_MSC,
        /// Used to describe binary state input switches.
        Switch = EV_SW,
        /// Used to turn LEDs on devices on and off.
        Led = EV_LED,
        /// Used to output sound to devices.
        Sound = EV_SND,
        /// Used for autorepeating devices.
        Repeat = EV_REP,
        /// Used to send force feedback commands to an input device.
        FeedBack = EV_FF,
        /// A special type for power button and switch input.
        Power = EV_PWR,
        /// Used to receive force feedback device status.
        FeedBackStatus = EV_FF_STATUS,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct InputEvent {
    pub event_type: InputEventType,
    pub code: u16,
    pub value: i32,
}

#[repr(u16)]
#[derive(Clone, Copy, Debug)]
pub enum CapabilityType {
    Key = EV_KEY,
    RelAxis = EV_REL,
    AbsAxis = EV_ABS,
    Misc = EV_MSC,
    Switch = EV_SW,
    Led = EV_LED,
    Sound = EV_SND,
    FeedBack = EV_FF,
    Event,
    InputProp,
}

pub struct InputCapability {
    /// bitmap to support up to 1024 bits.
    bitmap: [u64; 16],
}

impl InputCapability {
    pub fn empty() -> Self {
        Self { bitmap: [0; 16] }
    }

    pub fn from_bitmap(bitmap: &[u8]) -> Self {
        let mut cap = Self::empty();
        let bitcount = bitmap.len() as u16 * 8;
        for i in 0..bitcount as usize {
            if bitmap[i / 8] & (1 << (i % 64)) != 0 {
                cap.set(i as u16);
            }
        }
        cap
    }

    pub fn set(&mut self, code: u16) {
        self.bitmap[code as usize / 64] |= 1 << (code % 64);
    }

    pub fn set_all(&mut self, codes: &[u16]) {
        for &c in codes {
            self.set(c);
        }
    }

    pub fn contains(&self, code: u16) -> bool {
        self.bitmap[code as usize / 64] & (1 << (code % 64)) != 0
    }

    pub fn contains_all(&self, codes: &[u16]) -> bool {
        for &c in codes {
            if !self.contains(c) {
                return false;
            }
        }
        true
    }
}

impl fmt::Debug for InputCapability {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut skip_empty = true;
        write!(f, "[")?;
        for i in (0..16).rev() {
            if self.bitmap[i] > 0 || !skip_empty {
                write!(f, "{:#016x}", self.bitmap[i])?;
                if i > 0 {
                    write!(f, ", ")?;
                }
                skip_empty = false;
            }
        }
        write!(f, "]")?;
        Ok(())
    }
}

pub trait InputScheme: Scheme + EventScheme<Event = InputEvent> {
    /// Returns the capability bitmap of the specific kind of event.
    fn capability(&self, cap_type: CapabilityType) -> InputCapability;
}
