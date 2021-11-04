use alloc::boxed::Box;
use alloc::sync::Arc;
use core::ops::Range;

use super::Scheme;
use crate::DeviceResult;

pub type IrqHandler = Box<dyn Fn() + Send + Sync>;

#[derive(Debug)]
pub enum IrqTriggerMode {
    Edge,
    Level,
}

#[derive(Debug)]
pub enum IrqPolarity {
    ActiveHigh,
    ActiveLow,
}

pub trait IrqScheme: Scheme {
    /// Is a valid IRQ number.
    fn is_valid_irq(&self, irq_num: usize) -> bool;

    /// Disable IRQ.
    fn mask(&self, irq_num: usize) -> DeviceResult;

    /// Enable IRQ.
    fn unmask(&self, irq_num: usize) -> DeviceResult;

    /// Configure the specified interrupt vector. If it is invoked, it must be
    /// invoked prior to interrupt registration.
    fn configure(&self, _irq_num: usize, _tm: IrqTriggerMode, _pol: IrqPolarity) -> DeviceResult {
        unimplemented!()
    }

    /// Add an interrupt handler to an IRQ.
    fn register_handler(&self, irq_num: usize, handler: IrqHandler) -> DeviceResult;

    /// Register the device to delivery an IRQ.
    fn register_device(&self, irq_num: usize, dev: Arc<dyn Scheme>) -> DeviceResult {
        self.register_handler(irq_num, Box::new(move || dev.handle_irq(irq_num)))
    }

    /// Remove the interrupt handler to an IRQ.
    fn unregister(&self, irq_num: usize) -> DeviceResult;

    /// Method used for platform allocation of blocks of MSI and MSI-X compatible
    /// IRQ targets.
    fn msi_alloc_block(&self, _requested_irqs: usize) -> DeviceResult<Range<usize>> {
        unimplemented!()
    }

    /// Method used to free a block of MSI IRQs previously allocated by msi_alloc_block().
    /// This does not unregister IRQ handlers.
    fn msi_free_block(&self, _block: Range<usize>) -> DeviceResult {
        unimplemented!()
    }

    /// Register a handler function for a given msi_id within an msi_block_t. Passing a
    /// NULL handler will effectively unregister a handler for a given msi_id within the
    /// block.
    fn msi_register_handler(
        &self,
        _block: Range<usize>,
        _msi_id: usize,
        _handler: IrqHandler,
    ) -> DeviceResult {
        unimplemented!()
    }
}
