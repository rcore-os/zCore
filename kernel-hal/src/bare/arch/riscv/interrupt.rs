pub(super) fn init() {
    unsafe { riscv::register::sstatus::set_sie() };
    info!("+++ setup interrupt OK +++");
}

hal_fn_impl! {
    impl mod crate::hal_fn::interrupt {
        fn handle_irq(cause: u32) {
            crate::drivers::IRQ.handle_irq(cause as usize)
        }
    }
}
