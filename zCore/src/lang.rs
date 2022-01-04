// Rust language features implementations

use core::alloc::Layout;
use core::panic::PanicInfo;
use log::*;
//use zircon_object::util::kcounter::KCounterDescriptorArray;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("\n\npanic cpu={}", kernel_hal::cpu::cpu_id());
    println!("\n\n{}", info);
    error!("\n\n{}", info);
    backtrace();
    //error!("{:#?}", KCounterDescriptorArray::get());
    loop {
        core::hint::spin_loop();
    }
}

#[lang = "oom"]
fn oom(_: Layout) -> ! {
    panic!("out of memory");
}

fn backtrace() {
    let s0: u64;
    unsafe {asm!("mv {0}, fp", out(reg) s0);}
    let mut fp = s0;
    let x = 5;
    println!("fp=0x{:x}", fp);
    for _ in 0..5 {
        unsafe {
            println!("fn addr=0x{:x}", *((fp - 8) as *const u64));
            fp = *((fp - 16) as *const u64)
        }
    }
}
