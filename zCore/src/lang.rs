// Rust language features implementations

use core::alloc::Layout;
use core::panic::PanicInfo;
use core::sync::atomic::spin_loop_hint;
use log::*;
use zircon_object::util::kcounter::KCounterDescriptorArray;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    error!("\n\n{}", info);
    error!("{:#?}", KCounterDescriptorArray::get());
    loop {
        spin_loop_hint();
    }
}

#[lang = "oom"]
fn oom(_: Layout) -> ! {
    panic!("out of memory");
}
