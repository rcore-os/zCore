use alloc::sync::Arc;
use core::ops::Range;

use super::Scheme;
use crate::DeviceResult;

/// An interrupt handler closure.
///
/// `Arc`, not `Box`, so a dispatcher can clone the handler out of its table,
/// release the table lock, and only THEN invoke it — the clone keeps the
/// closure alive even if another CPU unregisters it mid-call. Running a handler
/// while still holding the dispatch lock deadlocks: the x86 timer handler
/// re-enters the IRQ path and re-takes the same global APIC lock on the same
/// CPU, freezing it (and the timer heap lock) forever. See `x86_apic::handle_irq`.
pub type IrqHandler = Arc<dyn Fn() + Send + Sync>;

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
        self.register_handler(irq_num, Arc::new(move || dev.handle_irq(irq_num)))
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

    /// Init irq for current cpu.
    /// Some IRQ hardware requires per-CPU initialization.
    fn init_hart(&self) {
        unimplemented!()
    }

    /// [for x86_64] enable apic timer
    fn apic_timer_enable(&self) {
        unimplemented!()
    }
}
