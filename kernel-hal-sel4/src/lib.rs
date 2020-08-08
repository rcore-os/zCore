#![no_std]
#![feature(asm, global_asm, alloc_error_handler)]
#![feature(linkage, const_btree_new)]

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

pub unsafe fn boot() -> ! {
    println!("Hello from seL4 kernel HAL.");
    allocator::init();
    pmem::init();
    cap::init();

    println!("Testing allocation.");
    let mut result: u32 = 0;
    for i in 0..1000 {
        let mut v = vec![0u8; 1000];
        v[0] = 1;
        v[1] = 1;
        for i in 2..v.len() {
            v[i] = v[i - 2] + v[i - 1];
        }
        result += v[v.len() - 1] as u32;
    }
    println!("result: {}", result);
    println!("Attempting to allocate one physical page 100000 times.");

    for i in 0..100000 {
        println!("{} begin", i);
        core::mem::forget(match pmem::Page::allocate() {
            Ok(x) => x,
            Err(e) => panic!("allocate failed at round {}: {:?}", i, e)
        });
        println!("{} end", i);
    }
    println!("Mapped and released successfully");

    loop {}
}

#[panic_handler]
fn on_panic(info: &core::panic::PanicInfo) -> ! {
    println!("panic: {:?}", info);
    loop {}
}