pub(super) mod block;
pub(super) mod display;
pub(super) mod input;
pub(super) mod irq;
pub(super) mod net;
pub(super) mod uart;

use alloc::sync::Arc;

pub use block::BlockScheme;
pub use display::DisplayScheme;
pub use input::InputScheme;
pub use irq::IrqScheme;
pub use net::NetScheme;
pub use uart::UartScheme;

pub trait Scheme: SchemeUpcast + Send + Sync {
    fn name(&self) -> &str;
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
