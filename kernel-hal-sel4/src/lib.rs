#![no_std]
#![feature(llvm_asm, global_asm, alloc_error_handler)]
#![feature(core_intrinsics, maybe_uninit_extra, linkage, const_btree_new, map_first_last, negative_impls)]

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
pub mod hal;
mod benchmark;
mod executor;

use alloc::boxed::Box;

pub unsafe fn boot() -> ! {
    let stack_probe: u32 = 0;
    println!("Initializing seL4 kernel HAL. Boot stack: {:p}", &stack_probe);
    allocator::init();
    pmem::init();
    cap::init();
    futex::init();
    control::init();
    executor::init();

    kt::spawn(|| {
        zc::zcore_main();
    }).expect("cannot spawn zcore_main");

    // It's not safe to do anything useful on the boot thread, since we're not in
    // control of our stack
    control::idle();
}

#[panic_handler]
fn on_panic(info: &core::panic::PanicInfo) -> ! {
    println!("panic: {:?}", info);
    loop {}
}