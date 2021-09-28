mod block;
mod display;
mod input;
mod irq;
mod net;
mod uart;

use alloc::sync::Arc;

pub use block::BlockScheme;
pub use display::DisplayScheme;
pub use input::InputScheme;
pub use irq::{IrqHandler, IrqPolarity, IrqScheme, IrqTriggerMode};
pub use net::NetScheme;
pub use uart::UartScheme;

pub trait Scheme: SchemeUpcast + Send + Sync {
    fn name(&self) -> &'static str;
    fn handle_irq(&self, _irq_num: usize) {}
}

pub trait SchemeUpcast {
    fn upcast<'a>(self: Arc<Self>) -> Arc<dyn Scheme + 'a>
    where
        Self: 'a;
}

impl<T: Scheme + Sized> SchemeUpcast for T {
    fn upcast<'a>(self: Arc<Self>) -> Arc<dyn Scheme + 'a>
    where
        Self: 'a,
    {
        self
    }
}
