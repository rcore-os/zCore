//! Interrupts management.
use cortex_a::asm::wfi;

hal_fn_impl! {
    impl mod crate::hal_fn::interrupt {
        fn wait_for_interrupt() {
            intr_on();
            wfi();
            intr_off();
        }

        fn handle_irq(vector: usize) {
            // TODO: timer and other devices with GIC interrupt controller
            crate::drivers::all_irq().first_unwrap().handle_irq(vector);
            if vector == 30 {
                debug!("Timer");
            }
        }

        fn intr_off() {
            unsafe {
                core::arch::asm!("msr daifset, #2");
            }
        }

        fn intr_on() {
            unsafe {
                core::arch::asm!("msr daifclr, #2");
            }
        }
    }
}
