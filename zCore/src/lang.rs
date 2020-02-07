// Rust language features implementations

use core::alloc::Layout;
use core::panic::PanicInfo;
use log::*;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    error!("\n\n{}", info);
    #[allow(clippy::empty_loop)]
    loop {}
}

#[lang = "oom"]
fn oom(_: Layout) -> ! {
    panic!("out of memory");
}
