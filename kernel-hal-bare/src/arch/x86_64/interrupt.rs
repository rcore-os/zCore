#![allow(dead_code)]
#![allow(non_upper_case_globals)]

use super::{acpi_table::*, phys_to_virt};
use alloc::boxed::Box;
use alloc::vec::Vec;
use apic::IoApic;
use spin::Mutex;
use trapframe::TrapFrame;

const IO_APIC_NUM_REDIRECTIONS: u8 = 120;
const TABLE_SIZE: usize = 256;
pub type InterruptHandle = Box<dyn Fn() + Send + Sync>;
lazy_static! {
    static ref IRQ_TABLE: Mutex<Vec<Option<InterruptHandle>>> = Default::default();
}

pub fn init() {
    unsafe {
        init_ioapic();
    }
    init_irq_table();
    irq_add_handle(Timer + IRQ0, Box::new(timer));
    irq_add_handle(Keyboard + IRQ0, Box::new(keyboard));
    irq_add_handle(COM1 + IRQ0, Box::new(com1));
    irq_enable_raw(Keyboard, Keyboard + IRQ0);
    irq_enable_raw(COM1, COM1 + IRQ0);
}

fn init_irq_table() {
    let mut table = IRQ_TABLE.lock();
    for _ in 0..TABLE_SIZE {
        table.push(None);
    }
}

unsafe fn init_ioapic() {
    for ioapic in AcpiTable::get_ioapic() {
        info!("Ioapic found: {:#x?}", ioapic);
        let mut ip = IoApic::new(phys_to_virt(ioapic.address as usize));
        ip.disable_all();
    }
    let mut ip = IoApic::new(phys_to_virt(super::IOAPIC_ADDR));
    ip.disable_all();
}

fn get_ioapic(irq: u32) -> Option<acpi::interrupt::IoApic> {
    for i in AcpiTable::get_ioapic() {
        let num_instr = core::cmp::min(
            ioapic_maxinstr(i.address).unwrap(),
            IO_APIC_NUM_REDIRECTIONS - 1,
        );
        if i.global_system_interrupt_base <= irq
            && irq <= i.global_system_interrupt_base + num_instr as u32
        {
            return Some(i);
        }
    }
    None
}

fn ioapic_controller(i: &acpi::interrupt::IoApic) -> IoApic {
    unsafe { IoApic::new(phys_to_virt(i.address as usize)) }
}

#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    trace!("Interrupt: {:#x} @ CPU{}", tf.trap_num, 0); // TODO 0 should replace in multi-core case
    match tf.trap_num as u8 {
        Breakpoint => breakpoint(),
        DoubleFault => double_fault(tf),
        PageFault => page_fault(tf),
        IRQ0..=63 => irq_handle(tf.trap_num as u8),
        _ => panic!("Unhandled interrupt {:x} {:#x?}", tf.trap_num, tf),
    }
}

#[export_name = "hal_irq_handle"]
pub fn irq_handle(irq: u8) {
    use super::{LocalApic, XApic, LAPIC_ADDR};
    let mut lapic = unsafe { XApic::new(phys_to_virt(LAPIC_ADDR)) };
    lapic.eoi();
    let table = IRQ_TABLE.lock();
    match &table[irq as usize] {
        Some(f) => f(),
        None => panic!("unhandled external IRQ number: {}", irq),
    }
}

#[export_name = "hal_ioapic_set_handle"]
pub fn set_handle(global_irq: u32, handle: InterruptHandle) -> Option<u8> {
    info!("set_handle irq={:#x?}", global_irq);
    // if global_irq == 1 {
    //     irq_add_handle(global_irq as u8 + IRQ0, handle);
    //     return Some(global_irq as u8 + IRQ0);
    // }
    let ioapic_info = get_ioapic(global_irq)?;
    let mut ioapic = ioapic_controller(&ioapic_info);
    let offset = (global_irq - ioapic_info.global_system_interrupt_base) as u8;
    let irq = ioapic.irq_vector(offset);
    let new_handle = if global_irq == 0x1 {
        Box::new(move || {
            handle();
            keyboard();
        })
    } else {
        handle
    };
    irq_add_handle(irq, new_handle).map(|x| {
        info!(
            "irq_set_handle: mapping from {:#x?} to {:#x?}",
            global_irq, x
        );
        ioapic.set_irq_vector(offset, x);
        x
    })
}

#[export_name = "hal_ioapic_reset_handle"]
pub fn reset_handle(global_irq: u32) -> bool {
    info!("reset_handle");
    let ioapic_info = if let Some(x) = get_ioapic(global_irq) {
        x
    } else {
        return false;
    };
    let mut ioapic = ioapic_controller(&ioapic_info);
    let offset = (global_irq - ioapic_info.global_system_interrupt_base) as u8;
    let irq = ioapic.irq_vector(offset);
    if !irq_remove_handle(irq) {
        ioapic.set_irq_vector(offset, 0);
        true
    } else {
        false
    }
}

/// Add a handle to IRQ table. Return the specified irq or an allocated irq on success
#[export_name = "hal_irq_add_handle"]
pub fn irq_add_handle(irq: u8, handle: InterruptHandle) -> Option<u8> {
    info!("IRQ add handle {:#x?}", irq);
    let mut table = IRQ_TABLE.lock();
    // allocate a valid irq number
    if irq == 0 {
        let mut id = 0x20;
        while id < table.len() {
            if table[id].is_none() {
                table[id] = Some(handle);
                return Some(id as u8);
            }
            id += 1;
        }
        return None;
    }
    match table[irq as usize] {
        Some(_) => None,
        None => {
            table[irq as usize] = Some(handle);
            Some(irq)
        }
    }
}

#[export_name = "hal_irq_remove_handle"]
pub fn irq_remove_handle(irq: u8) -> bool {
    // TODO: ioapic redirection entries associated with this should be reset.
    info!("IRQ remove handle {:#x?}", irq);
    let irq = irq as usize;
    let mut table = IRQ_TABLE.lock();
    match table[irq] {
        Some(_) => {
            table[irq] = None;
            false
        }
        None => true,
    }
}

#[export_name = "hal_irq_allocate_block"]
pub fn allocate_block(irq_num: u32) -> Option<(usize, usize)> {
    info!("hal_irq_allocate_block: count={:#x?}", irq_num);
    let irq_num = u32::next_power_of_two(irq_num) as usize;
    let mut irq_start = 0x20;
    let mut irq_cur = irq_start;
    let mut table = IRQ_TABLE.lock();
    while irq_cur < TABLE_SIZE && irq_cur < irq_start + irq_num {
        if table[irq_cur].is_none() {
            irq_cur += 1;
        } else {
            irq_start = (irq_cur - irq_cur % irq_num) + irq_num;
            irq_cur = irq_start;
        }
    }
    for i in irq_start..irq_start + irq_num {
        table[i] = Some(Box::new(|| {}));
    }
    info!(
        "hal_irq_allocate_block: start={:#x?} num={:#x?}",
        irq_start, irq_num
    );
    Some((irq_start, irq_num))
}

#[export_name = "hal_irq_free_block"]
pub fn free_block(irq_start: u32, irq_num: u32) {
    let mut table = IRQ_TABLE.lock();
    for i in irq_start..irq_start + irq_num {
        table[i as usize] = None;
    }
}

#[export_name = "hal_irq_overwrite_handler"]
pub fn overwrite_handler(msi_id: u32, handle: Box<dyn Fn() + Send + Sync>) -> bool {
    info!("IRQ overwrite handle {:#x?}", msi_id);
    let mut table = IRQ_TABLE.lock();
    let set = table[msi_id as usize].is_none();
    table[msi_id as usize] = Some(handle);
    set
}

#[export_name = "hal_irq_enable"]
pub fn irq_enable(irq: u32) {
    info!("irq_enable irq={:#x?}", irq);
    // if irq == 1 {
    //     irq_enable_raw(irq as u8, irq as u8 + IRQ0);
    //     return;
    // }
    if let Some(x) = get_ioapic(irq) {
        let mut ioapic = ioapic_controller(&x);
        ioapic.enable((irq - x.global_system_interrupt_base) as u8, 0);
    }
}

fn irq_enable_raw(irq: u8, vector: u8) {
    info!("irq_enable_raw: irq={:#x?}, vector={:#x?}", irq, vector);
    let mut ioapic = unsafe { IoApic::new(phys_to_virt(super::IOAPIC_ADDR)) };
    ioapic.set_irq_vector(irq, vector);
    ioapic.enable(irq, 0)
}

#[export_name = "hal_irq_disable"]
pub fn irq_disable(irq: u32) {
    info!("irq_disable");
    if let Some(x) = get_ioapic(irq) {
        let mut ioapic = ioapic_controller(&x);
        ioapic.disable((irq - x.global_system_interrupt_base) as u8);
    }
}

#[export_name = "hal_irq_configure"]
pub fn irq_configure(
    global_irq: u32,
    vector: u8,
    dest: u8,
    level_trig: bool,
    active_high: bool,
) -> bool {
    info!(
        "irq_configure: irq={:#x?}, vector={:#x?}, dest={:#x?}, level_trig={:#x?}, active_high={:#x?}",
        global_irq, vector, dest, level_trig, active_high
    );
    get_ioapic(global_irq)
        .map(|x| {
            let mut ioapic = ioapic_controller(&x);
            ioapic.config(
                (global_irq - x.global_system_interrupt_base) as u8,
                vector,
                dest,
                level_trig,
                active_high,
                false, /* physical */
                true,  /* mask */
            );
        })
        .is_some()
}

#[export_name = "hal_irq_maxinstr"]
pub fn ioapic_maxinstr(ioapic_addr: u32) -> Option<u8> {
    let mut table = MAX_INSTR_TABLE.lock();
    for (addr, v) in table.iter() {
        if *addr == ioapic_addr as usize {
            return Some(*v);
        }
    }
    let mut ioapic = unsafe { IoApic::new(phys_to_virt(ioapic_addr as usize)) };
    let v = ioapic.maxintr();
    table.push((ioapic_addr as usize, v));
    Some(v)
}

lazy_static! {
    static ref MAX_INSTR_TABLE: Mutex<Vec<(usize, u8)>> = Mutex::default();
}

#[export_name = "hal_irq_isvalid"]
pub fn irq_is_valid(irq: u32) -> bool {
    trace!("irq_is_valid: irq={:#x?}", irq);
    get_ioapic(irq).is_some()
}

#[export_name = "hal_wait_for_interrupt"]
pub fn wait_for_interrupt() {
    x86_64::instructions::interrupts::enable_interrupts_and_hlt();
    x86_64::instructions::interrupts::disable();
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
