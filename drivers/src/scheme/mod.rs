mod block;
mod display;
mod input;
mod net;
mod uart;

pub use block::BlockScheme;
pub use display::DisplayScheme;
pub use input::InputScheme;
pub use net::NetScheme;
pub use uart::UartScheme;

pub trait Scheme {
    fn init(&mut self) -> crate::DeviceResult {
        Ok(())
    }
}
