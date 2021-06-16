use crate::devices::NET_DRIVERS;
use crate::devices::net::e1000;
use crate::phys_to_virt;
use alloc::format;
use kernel_hal::PAGE_SIZE;
use pci::*;

const PCI_COMMAND: u16 = 0x04;
const PCI_CAP_PTR: u16 = 0x34;

const PCI_MSI_CTRL_CAP: u16 = 0x00;
const PCI_MSI_ADDR: u16 = 0x04;
const PCI_MSI_DATA_32: u16 = 0x08;
const PCI_MSI_DATA_64: u16 = 0x0C;

const PCI_CAP_ID_MSI: u8 = 0x05;

struct PortOpsImpl;

#[cfg(target_arch = "x86_64")]
use x86_64::instructions::port::Port;

#[cfg(target_arch = "x86_64")]
impl PortOps for PortOpsImpl {
    unsafe fn read8(&self, port: u16) -> u8 {
        Port::new(port).read()
    }
    unsafe fn read16(&self, port: u16) -> u16 {
        Port::new(port).read()
    }
    unsafe fn read32(&self, port: u16) -> u32 {
        Port::new(port).read()
    }
    unsafe fn write8(&self, port: u16, val: u8) {
        Port::new(port).write(val);
    }
    unsafe fn write16(&self, port: u16, val: u16) {
        Port::new(port).write(val);
    }
    unsafe fn write32(&self, port: u16, val: u32) {
        Port::new(port).write(val);
    }
}

#[export_name = "hal_bus_pci_init"]
pub fn init() {
    warn!("pci init");
    let pci_iter = unsafe { scan_bus(&PortOpsImpl, CSpaceAccessMethod::IO) };
    for dev in pci_iter {
        info!(
            "pci: {:02x}:{:02x}.{} {:#x} {:#x} ({} {}) irq: {}:{:?}",
            dev.loc.bus,
            dev.loc.device,
            dev.loc.function,
            dev.id.vendor_id,
            dev.id.device_id,
            dev.id.class,
            dev.id.subclass,
            dev.pic_interrupt_line,
            dev.interrupt_pin,
        );
        init_driver(&dev);
    }
}

pub fn init_driver(dev: &PCIDevice) {
    let name = format!("enp{}s{}f{}", dev.loc.bus, dev.loc.device, dev.loc.function);
    warn!("driver name {}", name);
    match (dev.id.vendor_id, dev.id.device_id) {
        (0x8086, 0x100e) | (0x8086, 0x100f) | (0x8086, 0x10d3) => {
            // 0x100e
            // 82540EM Gigabit Ethernet Controller
            // 0x100f
            // 82545EM Gigabit Ethernet Controller (Copper)
            // 0x10d3
            // 82574L Gigabit Network Connection
            if let Some(BAR::Memory(addr, len, _, _)) = dev.bars[0] {
                let irq = unsafe { enable(dev.loc) };
                let vaddr = phys_to_virt(addr as usize);
                let index = NET_DRIVERS.read().len();
                warn!("e1000");
                e1000::init(name, irq, vaddr, len as usize, index);
                return;
            }
        }
        (0x8086, 0x10fb) => {
            // 82599ES 10-Gigabit SFI/SFP+ Network Connection
            if let Some(BAR::Memory(_addr, _len, _, _)) = dev.bars[0] {
                let _irq = unsafe { enable(dev.loc) };
                let _vaddr = phys_to_virt(_addr as usize);
                let _index = NET_DRIVERS.read().len();
                // PCI_DRIVERS.lock().insert(
                //     dev.loc,
                //     ixgbe::ixgbe_init(name, irq, vaddr, len as usize, index),
                // );
                return;
            }
        }
        _ => {}
    }
    if dev.id.class == 0x01 && dev.id.subclass == 0x06 {
        // Mass storage class
        // SATA subclass
        if let Some(BAR::Memory(_addr, _len, _, _)) = dev.bars[5] {
            info!("Found AHCI dev {:?} BAR5 {:x?}", dev, _addr);
            let _irq = unsafe { enable(dev.loc) };
            assert!(_len as usize <= PAGE_SIZE);
            let _vaddr = phys_to_virt(_addr as usize);
            // if let Some(driver) = ahci::init(irq, vaddr, len as usize) {
            //     PCI_DRIVERS.lock().insert(dev.loc, driver);
            // }
        }
    }
}

/// Enable the pci device and its interrupt
/// Return assigned MSI interrupt number when applicable
unsafe fn enable(loc: Location) -> Option<usize> {
    let ops = &PortOpsImpl;
    let am = CSpaceAccessMethod::IO;

    // 23 and lower are used
    static mut MSI_IRQ: u32 = 23;

    let orig = am.read16(ops, loc, PCI_COMMAND);
    // IO Space | MEM Space | Bus Mastering | Special Cycles | PCI Interrupt Disable
    am.write32(ops, loc, PCI_COMMAND, (orig | 0x40f) as u32);

    // find MSI cap
    let mut msi_found = false;
    let mut cap_ptr = am.read8(ops, loc, PCI_CAP_PTR) as u16;
    let mut assigned_irq = None;
    while cap_ptr > 0 {
        let cap_id = am.read8(ops, loc, cap_ptr);
        if cap_id == PCI_CAP_ID_MSI {
            let orig_ctrl = am.read32(ops, loc, cap_ptr + PCI_MSI_CTRL_CAP);
            // The manual Volume 3 Chapter 10.11 Message Signalled Interrupts
            // 0 is (usually) the apic id of the bsp.
            am.write32(ops, loc, cap_ptr + PCI_MSI_ADDR, 0xfee00000 | (0 << 12));
            MSI_IRQ += 1;
            let irq = MSI_IRQ;
            assigned_irq = Some(irq as usize);
            // we offset all our irq numbers by 32
            if (orig_ctrl >> 16) & (1 << 7) != 0 {
                // 64bit
                am.write32(ops, loc, cap_ptr + PCI_MSI_DATA_64, irq + 32);
            } else {
                // 32bit
                am.write32(ops, loc, cap_ptr + PCI_MSI_DATA_32, irq + 32);
            }

            // enable MSI interrupt, assuming 64bit for now
            am.write32(ops, loc, cap_ptr + PCI_MSI_CTRL_CAP, orig_ctrl | 0x10000);
            debug!(
                "MSI control {:#b}, enabling MSI interrupt {}",
                orig_ctrl >> 16,
                irq
            );
            msi_found = true;
        }
        debug!("PCI device has cap id {} at {:#X}", cap_id, cap_ptr);
        cap_ptr = am.read8(ops, loc, cap_ptr + 1) as u16;
    }

    if !msi_found {
        // Use PCI legacy interrupt instead
        // IO Space | MEM Space | Bus Mastering | Special Cycles
        am.write32(ops, loc, PCI_COMMAND, (orig | 0xf) as u32);
        debug!("MSI not found, using PCI interrupt");
    }

    info!("pci device enable done");

    assigned_irq
}