#![no_std]

extern "C" {
    fn l4bridge_putchar(c: u8);
}

#[no_mangle]
pub unsafe extern "C" fn rust_start() -> ! {
    print_str("Hello from zCore!\n");
    loop {}
}

fn print_str(s: &str) {
    let s = s.as_bytes();
    for c in s {
        unsafe {
            l4bridge_putchar(*c);
        }
    }
}

#[panic_handler]
fn on_panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}