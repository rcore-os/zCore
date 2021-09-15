pub fn wait_for_interrupt() {
    unsafe {
        // enable interrupt and disable
        let sie = riscv::register::sstatus::read().sie();
        riscv::register::sstatus::set_sie();
        riscv::asm::wfi();
        if !sie {
            riscv::register::sstatus::clear_sie();
        }
    }
}
