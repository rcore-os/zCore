// Rust language features implementations

use core::alloc::Layout;
use core::panic::PanicInfo;
use log::*;
//use zircon_object::util::kcounter::KCounterDescriptorArray;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("\n\n{}", info);
    error!("\n\n{}", info);
    //error!("{:#?}", KCounterDescriptorArray::get());
    loop {
        core::hint::spin_loop();
    }
}

#[lang = "oom"]
fn oom(_: Layout) -> ! {
    panic!("out of memory");
}
