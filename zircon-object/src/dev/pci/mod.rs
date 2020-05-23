// use super::*;
mod acpi_table;
mod bus;
mod nodes;
mod pio;

pub use bus::*;
pub use nodes::*;
pub use pio::*;

#[derive(PartialEq)]
pub enum PciAddrSpace {
    MMIO,
    PIO,
}

#[repr(C)]
pub struct PciInitArgsAddrWindows {
    pub base: u64,
    pub size: usize,
    pub bus_start: u8,
    pub bus_end: u8,
    pub cfg_space_type: u8,
    pub has_ecam: bool,
    pub padding1: [u8; 4],
}

#[repr(C)]
pub struct PciInitArgsIrqs {
    pub global_irq: u32,
    pub level_triggered: bool,
    pub active_high: bool,
    pub padding1: [u8; 2],
}

pub const PCI_MAX_DEVICES_PER_BUS: usize = 32;
pub const PCI_MAX_FUNCTIONS_PER_DEVICE: usize = 8;
pub const PCI_MAX_LEGACY_IRQ_PINS: usize = 4;
pub const PCI_MAX_FUNCTIONS_PER_BUS: usize = PCI_MAX_FUNCTIONS_PER_DEVICE * PCI_MAX_DEVICES_PER_BUS;
pub const PCI_MAX_IRQS: usize = 224;
pub const PCI_INIT_ARG_MAX_ECAM_WINDOWS: usize = 2;

#[repr(transparent)]
#[derive(Clone)]
pub struct PciIrqSwizzleLut(
    [[[u32; PCI_MAX_LEGACY_IRQ_PINS]; PCI_MAX_FUNCTIONS_PER_DEVICE]; PCI_MAX_DEVICES_PER_BUS],
);

#[repr(C)]
pub struct PciInitArgsHeader {
    pub dev_pin_to_global_irq: PciIrqSwizzleLut,
    pub num_irqs: u32,
    pub irqs: [PciInitArgsIrqs; PCI_MAX_IRQS],
    pub addr_window_count: u32,
}

pub struct PciEcamRegion {
    pub phys_base: u64,
    pub size: usize,
    pub bus_start: u8,
    pub bus_end: u8,
}

pub struct MappedEcamRegion {
    pub ecam: PciEcamRegion,
    pub vaddr: u64,
}

pub const PCI_INIT_ARG_MAX_SIZE: usize = core::mem::size_of::<PciInitArgsAddrWindows>()
    * PCI_INIT_ARG_MAX_ECAM_WINDOWS
    + core::mem::size_of::<PciInitArgsHeader>();
pub const PCI_NO_IRQ_MAPPING: u32 = u32::MAX;
pub const PCIE_PIO_ADDR_SPACE_MASK: u64 = 0xFFFFFFFF; // (1 << 32) - 1
pub const PCIE_MAX_BUSSES: usize = 256;
pub const PCIE_ECAM_BYTES_PER_BUS: usize =
    4096 * PCI_MAX_DEVICES_PER_BUS * PCI_MAX_FUNCTIONS_PER_DEVICE;
pub const PCIE_INVALID_VENDOR_ID: usize = 0xFFFF;

pub const PCI_CFG_SPACE_TYPE_PIO: u8 = 0;
pub const PCI_CFG_SPACE_TYPE_MMIO: u8 = 1;
const IO_APIC_NUM_REDIRECTIONS: u8 = 120;
use super::*;
use alloc::sync::*;

pub fn pci_configure_interrupt(arg_header: &mut PciInitArgsHeader) -> ZxResult {
    for i in 0..arg_header.num_irqs as usize {
        let irq = &mut arg_header.irqs[i];
        let global_irq = irq.global_irq;
        if !is_valid_interrupt(global_irq) {
            irq.global_irq = PCI_NO_IRQ_MAPPING;
            pci_irq_swizzle_lut_remove_irq(&mut arg_header.dev_pin_to_global_irq, global_irq);
        } else {
            irq_configure(
                global_irq,
                irq.level_triggered, /* Trigger mode */
                irq.active_high,     /* Polarity */
            )?;
        }
    }
    Ok(())
}
fn get_irq(irq: u32) -> Option<acpi::interrupt::IoApic> {
    for i in acpi_table::AcpiTable::get_ioapic() {
        let num_instr = core::cmp::min(
            kernel_hal::ioapic_maxinstr(i.address as usize),
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

fn is_valid_interrupt(irq: u32) -> bool {
    get_irq(irq).is_some()
}

fn irq_configure(irq: u32, level_trigger: bool, active_high: bool) -> ZxResult {
    let irq_obj = get_irq(irq).ok_or(ZxError::INVALID_ARGS)?;
    // In fuchsia source code, 'BSP' stands for bootstrap processor
    let dest = kernel_hal::apic_local_id();
    kernel_hal::irq_configure(
        irq_obj.address as usize,
        (irq - irq_obj.global_system_interrupt_base) as u8,
        dest,
        level_trigger,
        active_high,
    );
    Ok(())
}

pub struct PcieRootLUTSwizzle(PciIrqSwizzleLut);

pub trait PcieRootSwizzle {
    fn swizzle(&self, dev_id: usize, func_id: usize, pin: usize) -> ZxResult<usize>;
}

impl PcieRootLUTSwizzle {
    pub fn new(
        pcie: Weak<PCIeBusDriver>,
        managed_bus_id: usize,
        lut: &PciIrqSwizzleLut,
    ) -> PcieRoot {
        PcieRoot {
            device: pcie,
            managed_bus_id,
            inner: Arc::new(PcieRootLUTSwizzle(lut.clone())),
        }
    }
}

impl PcieRootSwizzle for PcieRootLUTSwizzle {
    fn swizzle(&self, dev_id: usize, func_id: usize, pin: usize) -> ZxResult<usize> {
        if dev_id >= PCI_MAX_DEVICES_PER_BUS
            || func_id >= PCI_MAX_FUNCTIONS_PER_DEVICE
            || pin >= PCI_MAX_LEGACY_IRQ_PINS
        {
            return Err(ZxError::INVALID_ARGS);
        }
        let irq = (self.0).0[dev_id][func_id][pin];
        if irq == PCI_NO_IRQ_MAPPING {
            Err(ZxError::NOT_FOUND)
        } else {
            Ok(irq as usize)
        }
    }
}

fn pci_irq_swizzle_lut_remove_irq(lut: &mut PciIrqSwizzleLut, irq: u32) {
    for dev in lut.0.iter_mut() {
        for func in dev.iter_mut() {
            for pin in func.iter_mut() {
                if *pin == irq {
                    *pin = PCI_NO_IRQ_MAPPING;
                }
            }
        }
    }
}
