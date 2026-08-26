//! Minimal IDT to make early faults visible on real hardware.
//!
//! After ExitBootServices, firmware exception reporting may disappear. If the kernel
//! faults on the very first instruction fetch, the screen can freeze at the last
//! drawn progress value (e.g. 51%). Installing our own IDT lets us paint a marker.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use uefi::proto::console::gop::ModeInfo;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use crate::progress;

static FB_ADDR: AtomicU64 = AtomicU64::new(0);
static STRIDE: AtomicUsize = AtomicUsize::new(0);
static SW: AtomicUsize = AtomicUsize::new(0);
static SH: AtomicUsize = AtomicUsize::new(0);

static mut IDT: InterruptDescriptorTable = InterruptDescriptorTable::new();

pub fn init(mode: ModeInfo, fb_addr: u64) {
    let (sw, sh) = mode.resolution();
    FB_ADDR.store(fb_addr, Ordering::SeqCst);
    STRIDE.store(mode.stride() as usize, Ordering::SeqCst);
    SW.store(sw as usize, Ordering::SeqCst);
    SH.store(sh as usize, Ordering::SeqCst);

    unsafe {
        IDT.page_fault.set_handler_fn(page_fault_handler);
        IDT.general_protection_fault
            .set_handler_fn(gp_fault_handler);
        // UEFI firmware may be using interrupts/its own IDT while we're running.
        // Disable interrupts briefly while we load our temporary IDT to avoid
        // taking an interrupt with a half-updated setup.
        x86_64::instructions::interrupts::disable();
        IDT.load();
    }
}

extern "x86-interrupt" fn page_fault_handler(
    _stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    paint_fault_marker(0xAF00_0000u32, error_code.bits() as u32);
    loop {
        x86_64::instructions::hlt();
    }
}

extern "x86-interrupt" fn gp_fault_handler(_stack_frame: InterruptStackFrame, error_code: u64) {
    paint_fault_marker(0xA600_0000u32, error_code as u32);
    loop {
        x86_64::instructions::hlt();
    }
}

fn paint_fault_marker(tag: u32, code: u32) {
    let fb = FB_ADDR.load(Ordering::SeqCst);
    let stride = STRIDE.load(Ordering::SeqCst);
    let sw = SW.load(Ordering::SeqCst);
    let sh = SH.load(Ordering::SeqCst);
    if fb == 0 || stride == 0 || sw == 0 || sh == 0 {
        return;
    }
    // Show "99%" + a top-left colored block; also encode tag/code in pixels.
    progress::bar_raw(fb, stride, sw, sh, 99);
    progress::fault_block_raw(fb, stride, sw, sh, tag, code);
}
