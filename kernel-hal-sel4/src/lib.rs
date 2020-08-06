#![no_std]
#![feature(asm, global_asm)]
#![feature(linkage)]

#[macro_use]
mod console;
mod sys;

pub unsafe fn boot() -> ! {
    println!("Hello from seL4 kernel HAL.");
    println!("Attempting to map one page.");
    loop {}
}

#[panic_handler]
fn on_panic(info: &core::panic::PanicInfo) -> ! {
    println!("panic: {:?}", info);
    loop {}
}