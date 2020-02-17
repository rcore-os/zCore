use alloc::boxed::Box;
use x86_64::registers::model_specific::FsBase;
use x86_64::VirtAddr;

pub fn init() {
    // setup TLS
    // TODO: parse TLS program header?
    let tbss = Box::leak(Box::new([0usize, 0]));
    tbss[1] = &tbss[1] as *const _ as usize;
    FsBase::write(VirtAddr::new(tbss[1] as _));

    executor::spawn(async {
        info!("Hello! Async Rust!");
        // GG deadlock
        executor::spawn(async {
            info!("Nested task!");
        });
    });
}
