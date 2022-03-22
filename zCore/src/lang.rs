// Rust language features implementations

use core::alloc::Layout;
use core::panic::PanicInfo;
use log::*;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("\n\npanic cpu={}", kernel_hal::cpu::cpu_id());
    println!("\n\n{}", info);
    error!("\n\n{}", info);

    if cfg!(feature = "baremetal-test") {
        kernel_hal::cpu::reset();
    } else {
        loop {
            core::hint::spin_loop();
        }
    }
}

#[lang = "oom"]
fn oom(_: Layout) -> ! {
    panic!("out of memory");
}
