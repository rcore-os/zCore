//! Interrupts management.

hal_fn_impl! {
    impl mod crate::hal_fn::interrupt {
        fn wait_for_interrupt() {
            unimplemented!()
        }

        fn handle_irq(_vector: usize) {
            // TODO: timer and other devices with GIC interrupt controller
            use crate::arch::trap::IrqHandlerResult;
            if super::gic::handle_irq() == IrqHandlerResult::Reschedule {
                debug!("Timer achieved");
            }
        }
    }
}
