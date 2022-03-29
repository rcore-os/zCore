//! The [`Scheme`] describe some functions must be implemented for different type of devices,
//! there are many [`Scheme`] traits in this mod.
//!
//! If you need to develop a new device, just implement the corresponding trait.
//!
//! The [`Scheme`] trait is suitable for any architecture.

pub(super) mod block;
pub(super) mod display;
pub(super) mod input;
pub(super) mod irq;
pub(super) mod net;
pub(super) mod uart;

#[macro_use]
pub(super) mod event;
pub(super) use impl_event_scheme;

use alloc::sync::Arc;

pub use block::BlockScheme;
pub use display::DisplayScheme;
pub use event::EventScheme;
pub use input::InputScheme;
pub use irq::IrqScheme;
pub use net::NetScheme;
pub use uart::UartScheme;

/// Common of all device drivers.
///
/// Every device must says its name and handles interrupts.
pub trait Scheme: SchemeUpcast + Send + Sync {
    /// Returns name of the driver.
    fn name(&self) -> &str;

    /// Handles an interrupt.
    fn handle_irq(&self, _irq_num: usize) {}
}

/// Used to convert a concrete type pointer to a general [`Scheme`] pointer.
pub trait SchemeUpcast {
    /// Performs the conversion.
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
