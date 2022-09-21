// Rust language features implementations

use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("\n\npanic cpu={}\n{}", kernel_hal::cpu::cpu_id(), info);
    error!("\n\n{info}");

    if cfg!(feature = "baremetal-test") {
        kernel_hal::cpu::reset();
    } else {
        loop {
            core::hint::spin_loop();
        }
    }
}
