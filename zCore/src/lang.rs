// Rust language features implementations

use core::alloc::Layout;
use core::panic::PanicInfo;
use core::sync::atomic::spin_loop_hint;
use log::*;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    error!("\n\n{}", info);
    loop {
        spin_loop_hint();
    }
}

#[lang = "oom"]
fn oom(_: Layout) -> ! {
    panic!("out of memory");
}
