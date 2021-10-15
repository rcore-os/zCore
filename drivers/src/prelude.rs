pub use crate::scheme::display::{ColorFormat, DisplayInfo, FrameBuffer, Rectangle, RgbColor};
pub use crate::scheme::input::{CapabilityType, InputCapability, InputEvent, InputEventType};
pub use crate::scheme::irq::{IrqHandler, IrqPolarity, IrqTriggerMode};
pub use crate::{Device, DeviceError, DeviceResult};

pub mod input {
    pub use crate::input::{Mouse, MouseFlags, MouseState};
}
