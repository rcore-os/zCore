// Rust language features implementations

use core::alloc::Layout;
use core::panic::PanicInfo;
use log::*;

#[lang = "eh_personality"]
extern "C" fn eh_personality() {}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    error!("\n\n{}", info);
    loop {}
}

#[lang = "oom"]
fn oom(_: Layout) -> ! {
    panic!("out of memory");
}
