use crate::builder::IoMapper;
use crate::bus::pci_drivers::PciDriver;
use crate::bus::phys_to_virt;
use crate::{Device, DeviceError, DeviceResult};
use alloc::sync::Arc;
use pci::{PCIDevice, BAR};

pub struct VirtIoPciDriver;

impl PciDriver for VirtIoPciDriver {
    fn name(&self) -> &str {
        "virtio-pci"
    }

    fn matched(&self, vendor_id: u16, _device_id: u16) -> bool {
        vendor_id == 0x1af4
    }

    fn init(
        &self,
        dev: &PCIDevice,
        mapper: &Option<Arc<dyn IoMapper>>,
        _irq: Option<usize>,
    ) -> DeviceResult<Device> {
        let device_id = dev.id.device_id;

        warn!("VirtIO device {:x} found!", device_id);

        #[cfg(feature = "virtio")]
        {
            use crate::bus::pci::{PortOpsImpl, PCI_ACCESS};
            let ops = &PortOpsImpl;
            let am = PCI_ACCESS;
            let mut cap_ptr = unsafe { am.read8(ops, dev.loc, 0x34) } as u16;

            let mut common_cfg = None;
            let mut device_cfg = None;
            let mut notify_cfg = None;

            while cap_ptr > 0 {
                let cap_id = unsafe { am.read8(ops, dev.loc, cap_ptr) };
                if cap_id == 0x09 {
                    // Vendor Specific
                    let cfg_type = unsafe { am.read8(ops, dev.loc, cap_ptr + 3) };
                    let bar = unsafe { am.read8(ops, dev.loc, cap_ptr + 4) };
                    let offset = unsafe { am.read32(ops, dev.loc, cap_ptr + 8) };
                    let length = unsafe { am.read32(ops, dev.loc, cap_ptr + 12) };

                    match cfg_type {
                        1 => common_cfg = Some((bar, offset, length)),
                        2 => notify_cfg = Some((bar, offset, length)),
                        4 => device_cfg = Some((bar, offset, length)),
                        _ => {}
                    }
                    warn!(
                        "VirtIO Cap: type={}, bar={}, offset={:#x}, len={}",
                        cfg_type, bar, offset, length
                    );
                }
                cap_ptr = unsafe { am.read8(ops, dev.loc, cap_ptr + 1) } as u16;
            }

            if let Some((bar, offset, _len)) = common_cfg {
                if let Some(BAR::Memory(addr, bar_len, _, _)) = dev.bars[bar as usize] {
                    // Resolve a capability's MMIO virtual address through the
                    // *returned* mapper base, not `phys_to_virt`: on riscv a high
                    // BAR is mapped at a non-linear sv39 vaddr, so `phys_to_virt`
                    // would point at unmapped memory and every MMIO access would
                    // fault. `query_or_map` is idempotent (it queries an existing
                    // mapping first), so calling it per capability is safe even
                    // when several share one BAR. On x86 it returns the linear
                    // `phys_to_virt`, so `base + offset` is identical there.
                    let resolve = |pa: usize, len: usize, off: usize| -> usize {
                        match mapper {
                            Some(m) => m
                                .query_or_map(pa, len)
                                .map(|base| base + off)
                                .unwrap_or_else(|| phys_to_virt(pa + off)),
                            None => phys_to_virt(pa + off),
                        }
                    };

                    let common_vaddr = resolve(addr as usize, bar_len as usize, offset as usize);

                    let device_vaddr = if let Some((d_bar, d_offset, _)) = device_cfg {
                        if let Some(BAR::Memory(d_addr, d_len, _, _)) = dev.bars[d_bar as usize] {
                            resolve(d_addr as usize, d_len as usize, d_offset as usize)
                        } else {
                            0
                        }
                    } else {
                        0
                    };

                    let notify_vaddr = if let Some((n_bar, n_offset, _)) = notify_cfg {
                        if let Some(BAR::Memory(n_addr, n_len, _, _)) = dev.bars[n_bar as usize] {
                            resolve(n_addr as usize, n_len as usize, n_offset as usize)
                        } else {
                            0
                        }
                    } else {
                        0
                    };

                    let (fb_vaddr, fb_size) =
                        if let Some(BAR::Memory(fb_addr, fb_len, _, _)) = dev.bars[0] {
                            (
                                resolve(fb_addr as usize, fb_len as usize, 0),
                                fb_len as usize,
                            )
                        } else {
                            (0, 0)
                        };

                    if device_id == 0x1050 {
                        match crate::virtio::VirtIoGpu::new_modern(
                            common_vaddr,
                            device_vaddr,
                            notify_vaddr,
                            fb_vaddr,
                            fb_size,
                        ) {
                            Ok(gpu) => {
                                warn!("VirtIO Modern GPU initialized successfully!");
                                return Ok(Device::Drm(Arc::new(gpu)));
                            }
                            Err(e) => warn!("VirtIO Modern GPU init failed: {:?}", e),
                        }
                    }
                }
            }

            // Fallback to legacy if no modern caps found or failed
            if let Some(BAR::Memory(addr, len, _, _)) = dev.bars[0] {
                if let Some(m) = mapper {
                    m.query_or_map(addr as usize, len as usize);
                }
                let vaddr = phys_to_virt(addr as usize);
                let header = unsafe { &mut *(vaddr as *mut crate::virtio::VirtIOHeader) };

                match device_id {
                    0x1050 => {
                        if let Ok(gpu) = crate::virtio::VirtIoGpu::new(header) {
                            return Ok(Device::Drm(Arc::new(gpu)));
                        }
                    }
                    0x1001 | 0x1042 => {
                        if let Ok(blk) = crate::virtio::VirtIoBlk::new(header) {
                            return Ok(Device::Block(Arc::new(blk)));
                        }
                    }
                    0x1003 | 0x1043 => {
                        if let Ok(console) = crate::virtio::VirtIoConsole::new(header) {
                            return Ok(Device::Uart(Arc::new(console)));
                        }
                    }
                    0x1012 | 0x1052 => {
                        if let Ok(input) = crate::virtio::VirtIoInput::new(header) {
                            return Ok(Device::Input(Arc::new(input)));
                        }
                    }
                    _ => {
                        warn!("VirtIO legacy device {:x} is not yet supported", device_id);
                    }
                }
            }

            Err(DeviceError::NotSupported)
        }
        #[cfg(not(feature = "virtio"))]
        {
            Err(DeviceError::NotSupported)
        }
    }
}
