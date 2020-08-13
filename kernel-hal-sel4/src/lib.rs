#![no_std]
#![feature(global_asm, alloc_error_handler)]
#![feature(linkage, const_btree_new, map_first_last, negative_impls)]

#[macro_use]
extern crate alloc;

#[macro_use]
mod console;
mod sys;
mod allocator;
mod cap;
mod types;
mod sync;
mod thread;
mod error;
mod pmem;
mod vm;
mod object;
mod kt;
mod kipc;
mod control;
mod timer;
mod zc;
mod futex;
mod user;
mod asid;
mod hal;
mod benchmark;

use alloc::boxed::Box;

pub unsafe fn boot() -> ! {
    println!("Initializing seL4 kernel HAL.");
    allocator::init();
    pmem::init();
    cap::init();
    futex::init();

    kt::spawn(|| {
        zc::zcore_main();
    }).expect("cannot spawn zcore_main");
    control::run();
}

#[panic_handler]
fn on_panic(info: &core::panic::PanicInfo) -> ! {
    println!("panic: {:?}", info);
    loop {}
}