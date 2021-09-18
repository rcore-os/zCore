mod block;
mod display;
mod event;
mod input;
mod net;
mod uart;

pub use block::BlockScheme;
pub use display::DisplayScheme;
pub use event::EventListener;
pub use input::InputScheme;
pub use net::NetScheme;
pub use uart::UartScheme;

pub trait Scheme: Send + Sync {
    fn handle_irq(&self, _irq_num: u32) {}
    fn subscribe(&self, _handler: crate::IrqHandler, _once: bool) {
        unimplemented!("please call `subscribe()` with the `EventListener` wrapper")
    }
}
