#![allow(dead_code)]
#![allow(non_upper_case_globals)]
use trapframe::TrapFrame;
use spin::Mutex;

pub fn init() {
    irq_add_handle(Timer, timer);
    irq_add_handle(COM1, com1);
    irq_add_handle(Keyboard, keyboard);
    super::irq_enable(Keyboard);
    super::irq_enable(COM1);
}

#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    trace!("Interrupt: {:#x} @ CPU{}", tf.trap_num, 0); // TODO 0 should replace in multi-core case
    match tf.trap_num as u8 {
        Breakpoint => breakpoint(),
        DoubleFault => double_fault(tf),
        PageFault => page_fault(tf),
        IRQ0..=63 => irq_handle(tf.trap_num as u8 - IRQ0),
        _ => panic!("Unhandled interrupt {:x} {:#x?}", tf.trap_num, tf),
    }
}

lazy_static! {
    static ref IRQ_TABLE: Mutex<[Option<fn()>; 64]> = Mutex::new([None as Option<fn()>; 64]);
}

#[export_name = "hal_irq_handle"]
pub fn irq_handle(irq: u8) {
    use super::{phys_to_virt, LocalApic, XApic, LAPIC_ADDR};
    let mut lapic = unsafe { XApic::new(phys_to_virt(LAPIC_ADDR)) };
    lapic.eoi();
    let table = IRQ_TABLE.lock();
    match table[irq as usize] {
        Some(f) => f(),
        None => panic!("unhandled external IRQ number: {}", irq),
    }
}

#[export_name = "hal_irq_add_handle"]
pub fn irq_add_handle(irq: u8, handle: fn()) -> bool {
    let irq = irq as usize;
    let mut table = IRQ_TABLE.lock();
    match table[irq] {
        Some(_) => return false,
        None => {
            table[irq] = Some(handle);
            return true;
        }
    }
}

#[export_name = "hal_irq_remove_handle"]
pub fn irq_remove_handle(irq: u8) -> bool {
    let irq = irq as usize;
    let mut table = IRQ_TABLE.lock();
    match table[irq] {
        Some(_) => {
            table[irq] = None;
            false
        }
        None => true
    }
}

fn breakpoint() {
    panic!("\nEXCEPTION: Breakpoint");
}

fn double_fault(tf: &TrapFrame) {
    panic!("\nEXCEPTION: Double Fault\n{:#x?}", tf);
}

fn page_fault(tf: &mut TrapFrame) {
    panic!("\nEXCEPTION: Page Fault\n{:#x?}", tf);
}

fn timer() {
    super::timer_tick();
}

fn com1() {
    let c = super::COM1.lock().receive();
    super::serial_put(c);
}

fn keyboard() {
    use pc_keyboard::{DecodedKey, KeyCode};
    if let Some(key) = super::keyboard::receive() {
        match key {
            DecodedKey::Unicode(c) => super::serial_put(c as u8),
            DecodedKey::RawKey(code) => {
                let s = match code {
                    KeyCode::ArrowUp => "\u{1b}[A",
                    KeyCode::ArrowDown => "\u{1b}[B",
                    KeyCode::ArrowRight => "\u{1b}[C",
                    KeyCode::ArrowLeft => "\u{1b}[D",
                    _ => "",
                };
                for c in s.bytes() {
                    super::serial_put(c);
                }
            }
        }
    }
}

// Reference: https://wiki.osdev.org/Exceptions
const DivideError: u8 = 0;
const Debug: u8 = 1;
const NonMaskableInterrupt: u8 = 2;
const Breakpoint: u8 = 3;
const Overflow: u8 = 4;
const BoundRangeExceeded: u8 = 5;
const InvalidOpcode: u8 = 6;
const DeviceNotAvailable: u8 = 7;
const DoubleFault: u8 = 8;
const CoprocessorSegmentOverrun: u8 = 9;
const InvalidTSS: u8 = 10;
const SegmentNotPresent: u8 = 11;
const StackSegmentFault: u8 = 12;
const GeneralProtectionFault: u8 = 13;
const PageFault: u8 = 14;
const FloatingPointException: u8 = 16;
const AlignmentCheck: u8 = 17;
const MachineCheck: u8 = 18;
const SIMDFloatingPointException: u8 = 19;
const VirtualizationException: u8 = 20;
const SecurityException: u8 = 30;

const IRQ0: u8 = 32;

// IRQ
const Timer: u8 = 0;
const Keyboard: u8 = 1;
const COM2: u8 = 3;
const COM1: u8 = 4;
const IDE: u8 = 14;
const Error: u8 = 19;
const Spurious: u8 = 31;
