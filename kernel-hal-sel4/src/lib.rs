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
mod vm;
mod object;
mod kt;

use alloc::boxed::Box;

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

/*
    println!("Attempting to allocate one physical page 100000 times.");

    for i in 0..100000 {
        core::mem::forget(match pmem::Page::new() {
            Ok(x) => x,
            Err(e) => panic!("allocate failed at round {}: {:?}", i, e)
        });
    }
    println!("Mapped and released successfully");
*/
/*
    for i in 0..100000 {
        vm::K.lock().allocate_region(0x100ff0000usize..0x100ff2000usize).unwrap();
        unsafe {
            assert_eq!(core::ptr::read_volatile(0x100ff1000usize as *mut u32), 0);
            core::ptr::write_volatile(0x100ff1000usize as *mut u32, 10);
            assert_eq!(core::ptr::read_volatile(0x100ff1000usize as *mut u32), 10);
        }
        vm::K.lock().release_region(0x100ff0000usize);

    }
    println!("Testing ok.");
*/
    kt::KernelThread::new(Box::new(|kt| {
        loop {
            println!("(thread)");
        }
    })).expect("cannot start kernel thread");
    loop {
        println!("(boot)");
    }
}

#[panic_handler]
fn on_panic(info: &core::panic::PanicInfo) -> ! {
    println!("panic: {:?}", info);
    loop {}
}