//! Re-export most commonly used driver types.

pub use crate::scheme::display::{ColorFormat, DisplayInfo, FrameBuffer, Rectangle, RgbColor};
pub use crate::scheme::input::{CapabilityType, InputCapability, InputEvent, InputEventType};
pub use crate::scheme::irq::{IrqHandler, IrqPolarity, IrqTriggerMode};
pub use crate::{Device, DeviceError, DeviceResult};

/// Re-export types from [`input`](crate::input).
pub mod input {
    pub use crate::input::{Mouse, MouseFlags, MouseState};
}
