use {
    trapframe::{init as init_interrupt, TrapFrame},
    x86_64::registers::control::*,
};

pub fn init() {
    check_and_set_cpu_features();
    unsafe {
        init_interrupt();
    }
}

fn check_and_set_cpu_features() {
    unsafe {
        // Enable NX bit.
        Efer::update(|f| f.insert(EferFlags::NO_EXECUTE_ENABLE));

        // By default the page of CR3 have write protect.
        // We have to remove that before editing page table.
        Cr0::update(|f| f.remove(Cr0Flags::WRITE_PROTECT));
    }
}

#[no_mangle]
pub extern "C" fn rust_trap(tf: &TrapFrame) {
    info!("{:#x?}", tf);
}
