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
            crate::drivers::IRQ.handle_irq(cause)
        }
    }
}
