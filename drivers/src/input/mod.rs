mod mouse;
// PS/2 controller is x86-only hardware (port I/O 0x60/0x64).
#[cfg(target_arch = "x86_64")]
mod ps2_input;

pub mod input_event_codes;

pub use mouse::{Mouse, MouseFlags, MouseState};
#[cfg(target_arch = "x86_64")]
pub use ps2_input::Ps2Input;
