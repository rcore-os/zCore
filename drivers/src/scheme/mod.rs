mod block;
mod display;
mod input;
mod irq;
mod net;
mod uart;

pub use block::BlockScheme;
pub use display::DisplayScheme;
pub use input::InputScheme;
pub use irq::{IrqHandler, IrqPolarity, IrqScheme, IrqTriggerMode};
pub use net::NetScheme;
pub use uart::UartScheme;

pub trait Scheme: AsScheme + Send + Sync {
    fn handle_irq(&self, _irq_num: usize) {}
}

pub trait AsScheme {
    fn as_scheme(&self) -> &dyn Scheme;
}

impl<T: Scheme> AsScheme for T {
    fn as_scheme(&self) -> &dyn Scheme {
        self
    }
}
