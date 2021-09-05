#![allow(dead_code)]
#![allow(non_upper_case_globals)]

use alloc::{boxed::Box, vec::Vec};

use apic::IoApic;
use spin::Mutex;

use super::super::mem::phys_to_virt;
use super::acpi_table::AcpiTable;

const IRQ0: u8 = 32;

// IRQ
const Timer: u8 = 0;
const Keyboard: u8 = 1;
const COM2: u8 = 3;
const COM1: u8 = 4;
const Mouse: u8 = 12;
const IDE: u8 = 14;
const Error: u8 = 19;
const Spurious: u8 = 31;

const IO_APIC_NUM_REDIRECTIONS: u8 = 120;
const TABLE_SIZE: usize = 256;

type InterruptHandler = Box<dyn Fn() + Send + Sync>;

lazy_static::lazy_static! {
    static ref IRQ_TABLE: Mutex<Vec<Option<InterruptHandler>>> = Default::default();
    static ref MAX_INSTR_TABLE: Mutex<Vec<(usize, u8)>> = Mutex::default();
}

/*
lazy_static! {
    static ref MOUSE: Mutex<Mouse> = Mutex::new(Mouse::new());
    static ref MOUSE_CALLBACK: Mutex<Vec<Box<dyn Fn([u8; 3]) + Send + Sync>>> =
        Mutex::new(Vec::new());
}

#[export_name = "hal_mouse_set_callback"]
pub fn mouse_set_callback(callback: Box<dyn Fn([u8; 3]) + Send + Sync>) {
    MOUSE_CALLBACK.lock().push(callback);
}

fn mouse_on_complete(mouse_state: MouseState) {
    debug!("mouse state: {:?}", mouse_state);
    MOUSE_CALLBACK.lock().iter().for_each(|callback| {
        callback([
            mouse_state.get_flags().bits(),
            mouse_state.get_x() as u8,
            mouse_state.get_y() as u8,
        ]);
    });
}

fn mouse() {
    use x86_64::instructions::port::PortReadOnly;
    let mut port = PortReadOnly::new(0x60);
    let packet = unsafe { port.read() };
    MOUSE.lock().process_packet(packet);
}
*/

fn ioapic_maxinstr(ioapic_addr: u32) -> Option<u8> {
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

unsafe fn init_ioapic() {
    for ioapic in AcpiTable::get_ioapic() {
        info!("Ioapic found: {:#x?}", ioapic);
        let mut ip = IoApic::new(phys_to_virt(ioapic.address as usize));
        ip.disable_all();
    }
    let mut ip = super::apic::get_ioapic();
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

/// Add a handler to IRQ table. Return the specified irq or an allocated irq on success
fn irq_add_handler(irq: u8, handler: InterruptHandler) -> Option<u8> {
    info!("IRQ add handler {:#x?}", irq);
    let mut table = IRQ_TABLE.lock();
    // allocate a valid irq number
    if irq == 0 {
        let mut id = 0x20;
        while id < table.len() {
            if table[id].is_none() {
                table[id] = Some(handler);
                return Some(id as u8);
            }
            id += 1;
        }
        return None;
    }
    match table[irq as usize] {
        Some(_) => None,
        None => {
            table[irq as usize] = Some(handler);
            Some(irq)
        }
    }
}

fn irq_remove_handler(irq: u8) -> bool {
    // TODO: ioapic redirection entries associated with this should be reset.
    info!("IRQ remove handler {:#x?}", irq);
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

fn irq_overwrite_handler(irq: u8, handler: Box<dyn Fn() + Send + Sync>) -> bool {
    info!("IRQ overwrite handle {:#x?}", irq);
    let mut table = IRQ_TABLE.lock();
    let set = table[irq as usize].is_none();
    table[irq as usize] = Some(handler);
    set
}

fn init_irq_table() {
    let mut table = IRQ_TABLE.lock();
    for _ in 0..TABLE_SIZE {
        table.push(None);
    }
}

hal_fn_impl! {
    impl mod crate::defs::interrupt {
        fn enable_irq(irq: u32) {
            info!("enable_irq irq={:#x?}", irq);
            // if irq == 1 {
            //     irq_enable_raw(irq as u8, irq as u8 + IRQ0);
            //     return;
            // }
            if let Some(x) = get_ioapic(irq) {
                let mut ioapic = ioapic_controller(&x);
                ioapic.enable((irq - x.global_system_interrupt_base) as u8, 0);
            }
        }

        fn disable_irq(irq: u32) {
            info!("disable_irq");
            if let Some(x) = get_ioapic(irq) {
                let mut ioapic = ioapic_controller(&x);
                ioapic.disable((irq - x.global_system_interrupt_base) as u8);
            }
        }

        fn is_valid_irq(irq: u32) -> bool {
            trace!("is_valid_irq: irq={:#x?}", irq);
            get_ioapic(irq).is_some()
        }

        fn configure_irq(vector: u32, trig_mode: bool, polarity: bool) -> bool {
            info!(
                "configure_irq: vector={:#x?}, trig_mode={:#x?}, polarity={:#x?}",
                vector, trig_mode, polarity
            );
            let dest = super::apic::lapic_id();
            get_ioapic(vector)
                .map(|x| {
                    let mut ioapic = ioapic_controller(&x);
                    ioapic.config(
                        (vector - x.global_system_interrupt_base) as u8,
                        0,
                        dest,
                        trig_mode,
                        polarity,
                        false, /* physical */
                        true,  /* mask */
                    );
                })
                .is_some()
        }

        fn register_irq_handler(global_irq: u32, handler: InterruptHandler) -> Option<u32> {
            info!("set_handle irq={:#x?}", global_irq);
            // if global_irq == 1 {
            //     irq_add_handler(global_irq as u8 + IRQ0, handler);
            //     return Some(global_irq as u8 + IRQ0);
            // }
            let ioapic_info = get_ioapic(global_irq)?;
            let mut ioapic = ioapic_controller(&ioapic_info);
            let offset = (global_irq - ioapic_info.global_system_interrupt_base) as u8;
            let irq = ioapic.irq_vector(offset);
            let new_handler = if global_irq == 0x1 {
                Box::new(move || {
                    handler();
                    // keyboard();
                    // mouse();
                })
            } else {
                handler
            };
            irq_add_handler(irq, new_handler).map(|x| {
                info!(
                    "irq_set_handle: mapping from {:#x?} to {:#x?}",
                    global_irq, x
                );
                ioapic.set_irq_vector(offset, x);
                x as u32
            })
        }

        fn unregister_irq_handler(global_irq: u32) -> bool {
            info!("reset_handle");
            let ioapic_info = if let Some(x) = get_ioapic(global_irq) {
                x
            } else {
                return false;
            };
            let mut ioapic = ioapic_controller(&ioapic_info);
            let offset = (global_irq - ioapic_info.global_system_interrupt_base) as u8;
            let irq = ioapic.irq_vector(offset);
            if !irq_remove_handler(irq) {
                ioapic.set_irq_vector(offset, 0);
                true
            } else {
                false
            }
        }

        fn handle_irq(irq: u32) {
            use apic::LocalApic;
            let mut lapic = super::apic::get_lapic();
            lapic.eoi();
            let table = IRQ_TABLE.lock();
            match &table[irq as usize] {
                Some(f) => f(),
                None => panic!("unhandled external IRQ number: {}", irq),
            }
        }

        fn msi_allocate_block(irq_num: u32) -> Option<(usize, usize)> {
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

        fn msi_free_block(irq_start: u32, irq_num: u32) {
            let mut table = IRQ_TABLE.lock();
            for i in irq_start..irq_start + irq_num {
                table[i as usize] = None;
            }
        }

        fn msi_register_handler(
            irq_start: u32,
            _irq_num: u32,
            msi_id: u32,
            handler: Box<dyn Fn() + Send + Sync>,
        ) {
            irq_overwrite_handler((irq_start + msi_id) as u8, handler);
        }
    }
}

fn irq57test() {
    warn!("irq 57");
    // poll_ifaces();
}

fn timer() {
    crate::timer::timer_tick();
}

fn com1() {
    let c = super::serial::COM1.lock().receive();
    super::serial::serial_put(c);
}

/*
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
*/

fn irq_enable_raw(irq: u8, vector: u8) {
    info!("irq_enable_raw: irq={:#x?}, vector={:#x?}", irq, vector);
    let mut ioapic = super::apic::get_ioapic();
    ioapic.set_irq_vector(irq, vector);
    ioapic.enable(irq, 0)
}

pub(super) fn init() {
    // MOUSE.lock().init().unwrap();
    // MOUSE.lock().set_on_complete(mouse_on_complete);
    unsafe {
        init_ioapic();
    }
    init_irq_table();
    irq_add_handler(Timer + IRQ0, Box::new(timer));
    // irq_add_handler(Keyboard + IRQ0, Box::new(keyboard));
    // irq_add_handler(Mouse + IRQ0, Box::new(mouse));
    irq_add_handler(COM1 + IRQ0, Box::new(com1));
    irq_add_handler(57u8, Box::new(irq57test));
    // irq_enable_raw(Keyboard, Keyboard + IRQ0);
    // irq_enable_raw(Mouse, Mouse + IRQ0);
    irq_enable_raw(COM1, COM1 + IRQ0);
}
