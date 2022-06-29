use super::{phys_to_virt, PAGE_SIZE};
use crate::builder::IoMapper;
use crate::{Device, DeviceError, DeviceResult, VirtAddr};
use alloc::{collections::BTreeMap, format, sync::Arc, vec::Vec};
use lock::Mutex;
use pci::*;

const PCI_COMMAND: u16 = 0x04;
const BAR0: u16 = 0x10;
const PCI_CAP_PTR: u16 = 0x34;
const PCI_INTERRUPT_LINE: u16 = 0x3c;
const PCI_INTERRUPT_PIN: u16 = 0x3d;

const PCI_MSI_CTRL_CAP: u16 = 0x00;
const PCI_MSI_ADDR: u16 = 0x04;
const PCI_MSI_UPPER_ADDR: u16 = 0x08;
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
    unsafe fn read32(&self, port: u32) -> u32 {
        Port::new(port as u16).read()
    }
    unsafe fn write8(&self, port: u16, val: u8) {
        Port::new(port).write(val);
    }
    unsafe fn write16(&self, port: u16, val: u16) {
        Port::new(port).write(val);
    }
    unsafe fn write32(&self, port: u32, val: u32) {
        Port::new(port as u16).write(val);
    }
}

#[cfg(target_arch = "x86_64")]
const PCI_BASE: usize = 0; //Fix me

#[cfg(any(target_arch = "mips", target_arch = "riscv64"))]
use super::{read, write};

#[cfg(feature = "board_malta")]
const PCI_BASE: usize = 0xbbe00000;

#[cfg(target_arch = "riscv64")]
const PCI_BASE: usize = 0x30000000;
#[cfg(target_arch = "riscv64")]
const E1000_BASE: usize = 0x40000000;
// riscv64 Qemu

#[cfg(target_arch = "x86_64")]
const PCI_ACCESS: CSpaceAccessMethod = CSpaceAccessMethod::IO;
#[cfg(not(target_arch = "x86_64"))]
const PCI_ACCESS: CSpaceAccessMethod = CSpaceAccessMethod::MemoryMapped(PCI_BASE as *mut u8);

#[cfg(any(target_arch = "mips", target_arch = "riscv64"))]
impl PortOps for PortOpsImpl {
    unsafe fn read8(&self, port: u16) -> u8 {
        read(phys_to_virt(PCI_BASE) + port as usize)
    }
    unsafe fn read16(&self, port: u16) -> u16 {
        read(phys_to_virt(PCI_BASE) + port as usize)
    }
    unsafe fn read32(&self, port: u32) -> u32 {
        read(phys_to_virt(PCI_BASE) + port as usize)
    }
    unsafe fn write8(&self, port: u16, val: u8) {
        write(phys_to_virt(PCI_BASE) + port as usize, val);
    }
    unsafe fn write16(&self, port: u16, val: u16) {
        write(phys_to_virt(PCI_BASE) + port as usize, val);
    }
    unsafe fn write32(&self, port: u32, val: u32) {
        write(phys_to_virt(PCI_BASE) + port as usize, val);
    }
}

/// Enable the pci device and its interrupt
/// Return assigned MSI interrupt number when applicable
unsafe fn enable(loc: Location, paddr: u64) -> Option<usize> {
    let ops = &PortOpsImpl;
    //let am = CSpaceAccessMethod::IO;
    let am = PCI_ACCESS;

    if paddr != 0 {
        // reveal PCI regs by setting paddr
        let bar0_raw = am.read32(ops, loc, BAR0);
        am.write32(ops, loc, BAR0, (paddr & !0xfff) as u32); //Only for 32-bit decoding
        debug!(
            "BAR0 set from {:#x} to {:#x}",
            bar0_raw,
            am.read32(ops, loc, BAR0)
        );
    }

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
            //am.write32(ops, loc, cap_ptr + PCI_MSI_ADDR, 0xfee00000 | (0 << 12));
            am.write32(ops, loc, cap_ptr + PCI_MSI_ADDR, 0xfee00000);
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

pub fn init_driver(dev: &PCIDevice, mapper: &Option<Arc<dyn IoMapper>>) -> DeviceResult<Device> {
    let name = format!("enp{}s{}f{}", dev.loc.bus, dev.loc.device, dev.loc.function);
    match (dev.id.vendor_id, dev.id.device_id) {
        (0x8086, 0x100e) | (0x8086, 0x100f) | (0x8086, 0x10d3) => {
            // 0x100e
            // 82540EM Gigabit Ethernet Controller
            // 0x100f
            // 82545EM Gigabit Ethernet Controller (Copper)
            // 0x10d3
            // 82574L Gigabit Network Connection
            // (e1000e 8086:10d3)
            if let Some(BAR::Memory(addr, len, _, _)) = dev.bars[0] {
                info!("Found e1000e dev {:?} BAR0 {:#x?}", dev, addr);
                #[cfg(target_arch = "riscv64")]
                let addr = if addr == 0 { E1000_BASE as u64 } else { addr };

                if let Some(m) = mapper {
                    m.query_or_map(addr as usize, PAGE_SIZE * 8);
                }
                let irq = unsafe { enable(dev.loc, addr) };
                let vaddr = phys_to_virt(addr as usize);
                let dev = Device::Net(Arc::new(crate::net::e1000::init(
                    name,
                    irq.unwrap_or(0),
                    vaddr,
                    len as usize,
                    0,
                )?));
                return Ok(dev);
            }
        }
        (0x8086, 0x10fb) => {
            // 82599ES 10-Gigabit SFI/SFP+ Network Connection
            if let Some(BAR::Memory(addr, len, _, _)) = dev.bars[0] {
                let irq = unsafe { enable(dev.loc, 0) };
                let vaddr = phys_to_virt(addr as usize);
                info!("Found ixgbe dev {:#x}, irq: {:?}", vaddr, irq);
                /*
                let index = NET_DRIVERS.read().len();
                PCI_DRIVERS.lock().insert(
                    dev.loc,
                    ixgbe::ixgbe_init(name, irq, vaddr, len as usize, index),
                );
                */
                return Err(DeviceError::NotSupported);
            }
        }
        (0x8086, 0x1533) => {
            if let Some(BAR::Memory(addr, len, _, _)) = dev.bars[0] {
                info!("Intel Corporation I210 Gigabit Network Connection");
                info!("DEV: {:?}, BAR0: {:#x}", dev, addr);
                return Err(DeviceError::NotSupported);
            }
        }
        (0x8086, 0x1539) => {
            if let Some(BAR::Memory(addr, len, _, _)) = dev.bars[0] {
                info!(
                    "Found Intel I211 ethernet controller dev {:?}, addr: {:x?}",
                    dev, addr
                );
                return Err(DeviceError::NotSupported);
            }
        }
        _ => {}
    }
    if dev.id.class == 0x01 && dev.id.subclass == 0x06 {
        // Mass storage class
        // SATA subclass
        if let Some(BAR::Memory(addr, len, _, _)) = dev.bars[5] {
            info!("Found AHCI dev {:?} BAR5 {:x?}", dev, addr);
            /*
            let irq = unsafe { enable(dev.loc) };
            assert!(len as usize <= PAGE_SIZE);
            let vaddr = phys_to_virt(addr as usize);
            if let Some(driver) = ahci::init(irq, vaddr, len as usize) {
                PCI_DRIVERS.lock().insert(dev.loc, driver);
            }
            */
            return Err(DeviceError::NotSupported);
        }
    }

    Err(DeviceError::NoResources)
}

pub fn detach_driver(loc: &Location) -> bool {
    /*
    match PCI_DRIVERS.lock().remove(loc) {
        Some(driver) => {
            DRIVERS
                .write()
                .retain(|dri| dri.get_id() != driver.get_id());
            NET_DRIVERS
                .write()
                .retain(|dri| dri.get_id() != driver.get_id());
            true
        }
        None => false,
    }
    */
    false
}

pub fn init(mapper: Option<Arc<dyn IoMapper>>) -> DeviceResult<Vec<Device>> {
    let mapper_driver = if let Some(m) = mapper {
        m.query_or_map(PCI_BASE, PAGE_SIZE * 256 * 32 * 8);
        Some(m)
    } else {
        None
    };

    let mut dev_list = Vec::new();
    let pci_iter = unsafe { scan_bus(&PortOpsImpl, PCI_ACCESS) };
    info!("");
    info!("--------- PCI bus:device:function ---------");
    for dev in pci_iter {
        info!(
            "pci: {}:{}:{} {:04x}:{:04x} ({} {}) irq: {}:{:?}",
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
        let res = init_driver(&dev, &mapper_driver);
        match res {
            Ok(d) => dev_list.push(d),
            Err(e) => warn!(
                "{:?}, failed to initialize PCI device: {:04x}:{:04x}",
                e, dev.id.vendor_id, dev.id.device_id
            ),
        }
    }
    info!("---------");
    info!("");

    Ok(dev_list)
}

pub fn find_device(vendor: u16, product: u16) -> Option<Location> {
    let pci_iter = unsafe { scan_bus(&PortOpsImpl, PCI_ACCESS) };
    for dev in pci_iter {
        if dev.id.vendor_id == vendor && dev.id.device_id == product {
            return Some(dev.loc);
        }
    }
    None
}

pub fn get_bar0_mem(loc: Location) -> Option<(usize, usize)> {
    unsafe { probe_function(&PortOpsImpl, loc, PCI_ACCESS) }
        .and_then(|dev| dev.bars[0])
        .map(|bar| match bar {
            BAR::Memory(addr, len, _, _) => (addr as usize, len as usize),
            _ => unimplemented!(),
        })
}

// all devices stored in：AllDeviceList
