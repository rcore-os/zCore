#![no_std]
#![no_main]

use core::arch::asm;

/// Write a byte to COM1 serial port (0x3F8).
fn serial_putchar(c: u8) {
    unsafe {
        asm!(
            "out dx, al",
            in("dx") 0x3F8u16,
            in("al") c,
        );
    }
}

/// Write a string to serial port.
fn serial_print(s: &str) {
    for b in s.bytes() {
        serial_putchar(b);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    serial_print("\n[test-kernel] Hello from rboot test kernel!\n");
    serial_print("[test-kernel] rboot is working correctly.\n");

    // Shutdown QEMU via ISA debug exit device (port 0x501)
    unsafe {
        asm!("out dx, al", in("dx") 0x501u16, in("al") 0x31u8);
    }

    loop {
        unsafe { asm!("hlt") };
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    serial_print("[test-kernel] PANIC!\n");
    loop {
        unsafe { asm!("hlt") };
    }
}
