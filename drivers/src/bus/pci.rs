use super::PAGE_SIZE;
use crate::builder::IoMapper;
const PCI_COMMAND: u16 = 0x04;
use crate::{Device, DeviceError, DeviceResult};
use alloc::{sync::Arc, vec::Vec};
use pci::*;
const BAR0: u16 = 0x10;
#[allow(dead_code)]
pub(crate) const BAR5_REG: u16 = 0x24;
#[allow(dead_code)]
const PCI_CAP_PTR: u16 = 0x34;
const PCI_INTERRUPT_LINE: u16 = 0x3c;
#[allow(dead_code)]
const PCI_INTERRUPT_PIN: u16 = 0x3d;

#[allow(dead_code)]
const PCI_MSI_CTRL_CAP: u16 = 0x00;
#[allow(dead_code)]
const PCI_MSI_ADDR: u16 = 0x04;
#[allow(dead_code)]
const PCI_MSI_UPPER_ADDR: u16 = 0x08;
#[allow(dead_code)]
const PCI_MSI_DATA_32: u16 = 0x08;
#[allow(dead_code)]
const PCI_MSI_DATA_64: u16 = 0x0C;

#[allow(dead_code)]
const PCI_COMMAND_INTX_DISABLE: u16 = 0x0400;

#[allow(dead_code)]
const PCI_CAP_ID_MSI: u8 = 0x05;
const PCI_MAX_CAP_TRAVERSAL: usize = 64;

pub struct PortOpsImpl;

/// Read a BAR's physical base address directly from PCI config space.
/// Handles both 32-bit and 64-bit memory BARs without probing (no side effects).
/// `bar_reg` is the config-space byte offset (0x10 for BAR0, 0x14 for BAR1, etc.).
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub(crate) unsafe fn read_bar_addr(
    ops: &PortOpsImpl,
    am: CSpaceAccessMethod,
    loc: Location,
    bar_reg: u16,
) -> u64 {
    let lo = am.read32(ops, loc, bar_reg);
    if lo == 0 {
        return 0;
    }
    if (lo & 0x1) != 0 {
        // I/O space BAR: bits 31:2 = port, bits 1:0 = flags
        (lo & !0x3u32) as u64
    } else if (lo & 0x6) == 0x4 {
        // 64-bit memory BAR: combine with the next 32 bits
        let hi = am.read32(ops, loc, bar_reg + 4);
        ((lo & !0xFu32) as u64) | ((hi as u64) << 32)
    } else {
        // 32-bit memory BAR
        (lo & !0xFu32) as u64
    }
}

/// Probe the size of a BAR by writing all-ones and reading back.
/// Handles both 32-bit and 64-bit memory BARs.
/// Temporarily disables Memory/IO decoding (per PCI spec) to avoid bus errors.
/// Returns 0 if the BAR is not implemented or the size cannot be determined.
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub(crate) unsafe fn probe_bar_size(
    ops: &PortOpsImpl,
    am: CSpaceAccessMethod,
    loc: Location,
    bar_reg: u16,
) -> u64 {
    let orig_cmd = am.read16(ops, loc, PCI_COMMAND);
    // Disable Memory and I/O decoding while probing (PCI spec requirement)
    am.write16(ops, loc, PCI_COMMAND, orig_cmd & !0x03u16);

    let orig_lo = am.read32(ops, loc, bar_reg);
    am.write32(ops, loc, bar_reg, 0xFFFF_FFFF);
    let mask_lo = am.read32(ops, loc, bar_reg);
    am.write32(ops, loc, bar_reg, orig_lo);

    let size = if (orig_lo & 0x1) != 0 {
        // I/O space BAR
        let s = !(mask_lo & !0x3u32).wrapping_add(1);
        s as u64 & 0xFFFF_FFFF
    } else if (orig_lo & 0x6) == 0x4 {
        // 64-bit memory BAR: must probe both halves to get full size
        let orig_hi = am.read32(ops, loc, bar_reg + 4);
        am.write32(ops, loc, bar_reg + 4, 0xFFFF_FFFF);
        let mask_hi = am.read32(ops, loc, bar_reg + 4);
        am.write32(ops, loc, bar_reg + 4, orig_hi);
        let full_mask = ((mask_hi as u64) << 32) | (mask_lo as u64);
        let sz_mask = full_mask & !0xFu64;
        if sz_mask == 0 {
            0
        } else {
            (!sz_mask).wrapping_add(1)
        }
    } else {
        // 32-bit memory BAR
        let sz_mask = mask_lo & !0xFu32;
        if sz_mask == 0 {
            0
        } else {
            (!(sz_mask)).wrapping_add(1) as u64 & 0xFFFF_FFFF
        }
    };

    am.write16(ops, loc, PCI_COMMAND, orig_cmd);
    size
}

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

#[cfg(any(target_arch = "mips", target_arch = "riscv64", target_arch = "aarch64"))]
use super::{phys_to_virt, read, write};

#[cfg(feature = "board_malta")]
const PCI_BASE: usize = 0xbbe00000;

#[cfg(target_arch = "riscv64")]
const PCI_BASE: usize = 0x30000000;
#[cfg(target_arch = "riscv64")]
#[allow(dead_code)]
const E1000_BASE: usize = 0x40000000;
// riscv64 Qemu

// aarch64 QEMU `virt` PCIe ECAM configuration space base.
#[cfg(target_arch = "aarch64")]
const PCI_BASE: usize = 0x40_1000_0000;

#[cfg(target_arch = "x86_64")]
pub const PCI_ACCESS: CSpaceAccessMethod = CSpaceAccessMethod::IO;
#[cfg(not(target_arch = "x86_64"))]
pub const PCI_ACCESS: CSpaceAccessMethod = CSpaceAccessMethod::MemoryMapped(PCI_BASE as *mut u8);

#[cfg(any(target_arch = "mips", target_arch = "riscv64", target_arch = "aarch64"))]
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
#[allow(dead_code)]
unsafe fn enable(loc: Location, paddr: u64) -> Option<usize> {
    let ops = &PortOpsImpl;
    //let am = CSpaceAccessMethod::IO;
    let am = PCI_ACCESS;

    if paddr != 0 {
        // reveal PCI regs by setting paddr
        let bar0_raw = am.read32(ops, loc, BAR0);
        am.write32(ops, loc, BAR0, (paddr & !0xfff) as u32); //Only for 32-bit decoding
        warn!(
            "BAR0 set from {:#x} to {:#x}",
            bar0_raw,
            am.read32(ops, loc, BAR0)
        );
    }

    // 23 and lower are used. Atomic, not `static mut`: device probe can run
    // concurrently, and a plain `+=` on a `static mut` is a data race (UB).
    static MSI_IRQ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(23);

    let orig = am.read16(ops, loc, PCI_COMMAND);
    // Always enable MEM space + Bus Mastering so DMA devices (e.g. AHCI) work
    // regardless of whether MSI is available.
    am.write16(ops, loc, PCI_COMMAND, orig | 0x7);

    // find MSI cap
    let mut msi_found = false;
    let mut cap_ptr = am.read8(ops, loc, PCI_CAP_PTR) as u16;
    let mut assigned_irq = None;
    let mut cap_steps = 0usize;
    let mut prev_cap_ptr = 0u16;
    while cap_ptr > 0 {
        if cap_steps >= PCI_MAX_CAP_TRAVERSAL {
            warn!(
                "PCI capability chain too long or cyclic at {:?}, aborting traversal",
                loc
            );
            break;
        }
        if cap_ptr == prev_cap_ptr {
            warn!(
                "PCI capability chain stuck at {:#x} for {:?}, aborting traversal",
                cap_ptr, loc
            );
            break;
        }
        if cap_ptr < 0x40 {
            warn!(
                "PCI capability pointer out of spec ({:#x}) for {:?}, aborting traversal",
                cap_ptr, loc
            );
            break;
        }

        cap_steps = cap_steps.saturating_add(1);
        let cap_id = am.read8(ops, loc, cap_ptr);
        if cap_id == PCI_CAP_ID_MSI {
            let orig_ctrl = am.read32(ops, loc, cap_ptr + PCI_MSI_CTRL_CAP);
            // The manual Volume 3 Chapter 10.11 Message Signalled Interrupts
            // 0 is (usually) the apic id of the bsp.
            //am.write32(ops, loc, cap_ptr + PCI_MSI_ADDR, 0xfee00000 | (0 << 12));
            am.write32(ops, loc, cap_ptr + PCI_MSI_ADDR, 0xfee00000);
            let irq = MSI_IRQ.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
            assigned_irq = Some(irq as usize);
            // we offset all our irq numbers by 32
            if (orig_ctrl >> 16) & (1 << 7) != 0 {
                // 64bit: the message-address upper dword must be programmed too
                // (to 0 for the 0xFEE00000 LAPIC window), or the device latches
                // an undefined high address and MSIs go astray.
                am.write32(ops, loc, cap_ptr + PCI_MSI_UPPER_ADDR, 0);
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
        prev_cap_ptr = cap_ptr;
        cap_ptr = am.read8(ops, loc, cap_ptr + 1) as u16;
    }

    if !msi_found {
        am.write32(ops, loc, PCI_INTERRUPT_LINE, 33);
        debug!("MSI not found, using PCI interrupt");
    }

    // debug!, not warn!: this fires once per PCI FUNCTION. At the default
    // LOG=warn every line goes out the 115200-baud UART with a per-byte spin
    // (~87 µs/byte), and the per-device chatter alone was a visible slice of
    // boot wall-time on boards with a physical COM port. The line stays in
    // dmesg via the klog ring at debug level.
    debug!("pci device enable done");

    assigned_irq
}

pub fn init_driver(dev: &PCIDevice, mapper: &Option<Arc<dyn IoMapper>>) -> DeviceResult<Device> {
    // Enable Memory Space and Bus Mastering
    let irq = unsafe { enable(dev.loc, 0) };

    // Try modular PCI drivers (ArceOS style)
    if let Ok(device) = super::pci_drivers::probe_pci_device(dev, mapper, irq) {
        return Ok(device);
    }

    Err(DeviceError::NotSupported)
}

pub fn detach_driver(_loc: &Location) -> bool {
    false
}

pub fn init(mapper: Option<Arc<dyn IoMapper>>) -> DeviceResult<Vec<Device>> {
    let _mapper_driver = if let Some(m) = mapper.clone() {
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
        let res = init_driver(&dev, &mapper);
        match res {
            Ok(d) => dev_list.push(d),
            // debug!, not warn!: `NotSupported` is the NORMAL outcome for every
            // PCI function without a driver (bridges, SMBus, audio, …), and at
            // LOG=warn each line costs ~10 ms of per-byte UART spin. Real
            // driver failures log at their own probe sites.
            Err(e) => debug!(
                "{:?}, failed to initialize PCI device: {:04x}:{:04x}",
                e, dev.id.vendor_id, dev.id.device_id
            ),
        }
    }
    // One AHCI controller can host several disks but the probe only returns the
    // first as a `Device`; collect the remaining SATA disks it parked.
    for extra in crate::ata::ahci::take_extra_disks() {
        dev_list.push(extra);
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
