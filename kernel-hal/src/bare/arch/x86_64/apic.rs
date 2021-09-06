use apic::{IoApic, LocalApic, XApic};

use crate::mem::phys_to_virt;

const LAPIC_ADDR: usize = 0xfee0_0000;
const IOAPIC_ADDR: usize = 0xfec0_0000;

pub fn get_lapic() -> XApic {
    unsafe { XApic::new(phys_to_virt(LAPIC_ADDR)) }
}

pub fn get_ioapic() -> IoApic {
    unsafe { IoApic::new(phys_to_virt(IOAPIC_ADDR)) }
}

pub fn lapic_id() -> u8 {
    get_lapic().id() as u8
}

pub fn init() {
    get_lapic().cpu_init();
}
