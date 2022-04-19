//! Interrupts management.
use cortex_a::asm::wfi;

hal_fn_impl! {
    impl mod crate::hal_fn::interrupt {
        fn wait_for_interrupt() {
            wfi();
        }

        fn handle_irq(_vector: usize) {
            // TODO: timer and other devices with GIC interrupt controller
            use crate::IrqHandlerResult;
            if super::gic::handle_irq() == IrqHandlerResult::Reschedule {
                debug!("Timer achieved");
            }
        }
    }
}
