use super::Scheme;
use crate::input::input_event_codes::ev::*;
use crate::utils::EventHandler;

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
    pub value: u32,
}

pub trait InputScheme: Scheme {
    fn subscribe(&self, handler: EventHandler<InputEvent>, once: bool);
}
