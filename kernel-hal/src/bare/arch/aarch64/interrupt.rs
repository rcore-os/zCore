//! Interrupts management.
use cortex_a::asm::wfi;

hal_fn_impl! {
    impl mod crate::hal_fn::interrupt {
        fn wait_for_interrupt() {
            wfi();
        }

        fn handle_irq(vector: usize) {
            // TODO: timer and other devices with GIC interrupt controller
            use crate::IrqHandlerResult;
            crate::drivers::all_uart().first_unwrap().handle_irq(vector);
            if super::gic::handle_irq(vector) == IrqHandlerResult::Reschedule {
                debug!("Timer achieved");
            }
        }

        fn intr_off() {
            // TODO: off intr in aarch64
        }

        fn intr_on() {
            // TODO: open intr in aarch64
        }
    }
}
