#![allow(dead_code)]
#![allow(non_upper_case_globals)]

use alloc::{boxed::Box, vec::Vec};
use core::ops::Range;

use apic::IoApic;
use spin::Mutex;

use super::acpi_table::AcpiTable;
use crate::utils::irq_manager::{IrqHandler, IrqManager};
use crate::{mem::phys_to_virt, HalError, HalResult};

const IRQ0: u32 = 32;

const IRQ_MIN_ID: u32 = 0x20;
const IRQ_MAX_ID: u32 = 0xff;

// IRQ
const Timer: u32 = 0;
const Keyboard: u32 = 1;
const COM2: u32 = 3;
const COM1: u32 = 4;
const Mouse: u32 = 12;
const IDE: u32 = 14;
const Error: u32 = 19;
const Spurious: u32 = 31;

const IO_APIC_NUM_REDIRECTIONS: u8 = 120;

lazy_static! {
    static ref IRQ_MANAGER: Mutex<IrqManager> = Mutex::new(IrqManager::new(0x20, 0xff));
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

hal_fn_impl! {
    impl mod crate::hal_fn::interrupt {
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

        fn configure_irq(vector: u32, trig_mode: bool, polarity: bool) -> HalResult {
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
                .ok_or(HalError)
        }

        fn register_irq_handler(global_irq: u32, handler: IrqHandler) -> HalResult<u32> {
            info!("register_irq_handler irq={:#x?}", global_irq);
            // if global_irq == 1 {
            //     irq_add_handler(global_irq as u8 + IRQ0, handler);
            //     return Some(global_irq as u8 + IRQ0);
            // }
            let ioapic_info = get_ioapic(global_irq).ok_or(HalError)?;
            let mut ioapic = ioapic_controller(&ioapic_info);
            let offset = (global_irq - ioapic_info.global_system_interrupt_base) as u8;
            let x86_vector = ioapic.irq_vector(offset);
            let new_handler = if global_irq == 0x1 {
                Box::new(move || {
                    handler();
                    // keyboard();
                    // mouse();
                })
            } else {
                handler
            };
            let x86_vector = IRQ_MANAGER
                .lock()
                .register_handler(x86_vector as u32, new_handler)?;
            info!(
                "irq_set_handle: mapping from {:#x?} to {:#x?}",
                global_irq, x86_vector
            );
            ioapic.set_irq_vector(offset, x86_vector as u8);
            Ok(x86_vector)
        }

        fn unregister_irq_handler(global_irq: u32) -> HalResult {
            info!("unregister_irq_handler irq={:#x}", global_irq);
            let ioapic_info = if let Some(x) = get_ioapic(global_irq) {
                x
            } else {
                return Err(HalError);
            };
            let mut ioapic = ioapic_controller(&ioapic_info);
            let offset = (global_irq - ioapic_info.global_system_interrupt_base) as u8;
            let x86_vector = ioapic.irq_vector(offset);
            // TODO: ioapic redirection entries associated with this should be reset.
            IRQ_MANAGER.lock().unregister_handler(x86_vector as u32)
        }

        fn handle_irq(vector: u32) {
            use apic::LocalApic;
            let mut lapic = super::apic::get_lapic();
            lapic.eoi();
            IRQ_MANAGER.lock().handle(vector);
        }

        fn msi_allocate_block(requested_irqs: u32) -> HalResult<Range<u32>> {
            let alloc_size = requested_irqs.next_power_of_two();
            let start = IRQ_MANAGER.lock().alloc_block(alloc_size)?;
            Ok(start..start + alloc_size)
        }

        fn msi_free_block(block: Range<u32>) -> HalResult {
            IRQ_MANAGER
                .lock()
                .free_block(block.start, block.len() as u32)
        }

        fn msi_register_handler(
            block: Range<u32>,
            msi_id: u32,
            handler: Box<dyn Fn() + Send + Sync>,
        ) -> HalResult {
            IRQ_MANAGER
                .lock()
                .overwrite_handler(block.start + msi_id, handler)
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

fn irq_enable_raw(irq: u32, vector: u32) {
    info!("irq_enable_raw: irq={:#x?}, vector={:#x?}", irq, vector);
    let mut ioapic = super::apic::get_ioapic();
    ioapic.set_irq_vector(irq as u8, vector as u8);
    ioapic.enable(irq as u8, 0)
}

pub(super) fn init() {
    // MOUSE.lock().init().unwrap();
    // MOUSE.lock().set_on_complete(mouse_on_complete);
    unsafe {
        init_ioapic();
    }

    let mut im = IRQ_MANAGER.lock();
    im.register_handler(Timer + IRQ_MIN_ID, Box::new(timer))
        .ok();
    // im.register_handler(Keyboard + IRQ_MIN_ID, Box::new(keyboard));
    // im.register_handler(Mouse + IRQ_MIN_ID, Box::new(mouse));
    im.register_handler(
        COM1 + IRQ_MIN_ID,
        Box::new(|| crate::drivers::UART.handle_irq(COM1 as usize)),
    )
    .ok();
    im.register_handler(57u32, Box::new(irq57test)).ok();
    // register_handler(Keyboard, Keyboard + IRQ_MIN_ID);
    // register_handler(Mouse, Mouse + IRQ_MIN_ID);
    irq_enable_raw(COM1, COM1 + IRQ_MIN_ID);
}
