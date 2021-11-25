//! Interrupts management.

use riscv::{asm, register::sstatus};

hal_fn_impl! {
    impl mod crate::hal_fn::interrupt {
        fn wait_for_interrupt() {
            unsafe {
                // enable interrupt and disable
                sstatus::set_sie();
                asm::wfi();
                sstatus::clear_sie();
            }
        }

        fn handle_irq(cause: usize) {
            // supervisor software interrupt and
            // supervisor timer interrupt
            if cause == 1 || cause == 5 {
                crate::drivers::all_irq().
                                find("riscv-intc").
                                expect("IRQ device 'riscv-intc' not initialized!")
                                .handle_irq(cause);
            } else {
                crate::drivers::all_irq().
                                find("riscv-plic").
                                expect("IRQ device 'riscv-intc' not initialized!")
                                .handle_irq(cause);
            }
        }
    }
}
