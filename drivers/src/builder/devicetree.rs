//! Probe devices and create drivers from device tree.
//!
//! Specification: <https://github.com/devicetree-org/devicetree-specification/releases/download/v0.3/devicetree-specification-v0.3.pdf>.

use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};

use super::IoMapper;
use crate::utils::devicetree::{parse_interrupts, parse_reg};
use crate::utils::devicetree::{Devicetree, InheritProps, InterruptsProp, Node, StringList};
use crate::{Device, DeviceError, DeviceResult, VirtAddr};

/// A wrapper of [`Device`] which provides interrupt information additionally.
#[derive(Debug)]
struct DevWithInterrupt {
    /// For interrupt controller, represent the `phandle` property, otherwise
    /// is `None`.
    phandle: Option<u32>,
    /// For interrupt controller, represent the `interrupt_cells` property,
    /// otherwise is `None`.
    interrupt_cells: Option<u32>,
    /// A unified representation of the `interrupts` and `interrupts_extended`
    /// properties for any interrupt generating device.
    interrupts_extended: InterruptsProp,
    /// The inner [`Device`] structure.
    dev: Device,
}

/// A builder to probe devices and create drivers from device tree.
pub struct DevicetreeDriverBuilder<M: IoMapper> {
    dt: Devicetree,
    io_mapper: M,
}

impl<M: IoMapper> DevicetreeDriverBuilder<M> {
    /// Prepare to parse DTB from the given virtual address.
    pub fn new(dtb_base_vaddr: VirtAddr, io_mapper: M) -> DeviceResult<Self> {
        Ok(Self {
            dt: Devicetree::from(dtb_base_vaddr)?,
            io_mapper,
        })
    }

    /// Parse the device tree from root, and returns an array of [`Device`] it found.
    pub fn build(&self) -> DeviceResult<Vec<Device>> {
        let mut intc_map = BTreeMap::new();
        let mut dev_list = Vec::new();

        self.dt.walk(&mut |node, comp, props| {
            if let Ok(dev) = self.parse_device(node, comp, props) {
                // create the phandle-device mapping
                if node.has_prop("interrupt-controller") {
                    if let Some(phandle) = dev.phandle {
                        intc_map.insert(phandle, dev_list.len());
                    }
                }
                dev_list.push(dev);
            }
        });

        for dev in &dev_list {
            register_interrupt(dev, &dev_list, &intc_map).ok();
        }

        Ok(dev_list.into_iter().map(|d| d.dev).collect())
    }
}

#[allow(dead_code)]
#[allow(unused_imports)]
#[allow(unused_variables)]
#[allow(unreachable_code)]
impl<M: IoMapper> DevicetreeDriverBuilder<M> {
    /// Parse device nodes
    fn parse_device(
        &self,
        node: &Node,
        comp: &StringList,
        props: &InheritProps,
    ) -> DeviceResult<DevWithInterrupt> {
        debug!(
            "device-tree: parsing node {:?} with compatible {:?}",
            node.name, comp
        );
        // parse interrupt controller
        let res = if node.has_prop("interrupt-controller") {
            self.parse_intc(node, comp, props)
        } else {
            // parse other device
            match comp {
                #[cfg(feature = "virtio")]
                c if c.contains("virtio,mmio") => self.parse_virtio(node, props),
                c if c.contains("ns16550a") => self.parse_uart(node, comp, props),
                _ => Err(DeviceError::NotSupported),
            }
        };

        if let Err(err) = &res {
            if !matches!(err, DeviceError::NotSupported) {
                warn!(
                    "device-tree: failed to parsing node {:?}: {:?}",
                    node.name, err
                );
            }
        }
        res
    }

    /// Parse nodes for interrupt controllers.
    fn parse_intc(
        &self,
        node: &Node,
        comp: &StringList,
        props: &InheritProps,
    ) -> DeviceResult<DevWithInterrupt> {
        let phandle = node.prop_u32("phandle").ok();
        let interrupt_cells = node.prop_u32("#interrupt-cells").ok();
        let interrupts_extended = parse_interrupts(node, props)?;
        if phandle.is_none() || interrupt_cells.is_none() {
            return Err(DeviceError::InvalidParam);
        }
        let base_vaddr = parse_reg(node, props).and_then(|(paddr, size)| {
            self.io_mapper
                .query_or_map(paddr as usize, size as usize)
                .ok_or(DeviceError::NoResources)
        });

        use crate::irq::*;
        let dev = Device::Irq(match comp {
            #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
            c if c.contains("riscv,cpu-intc") => Arc::new(riscv::Intc::new()),
            #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
            c if c.contains("riscv,plic0") => Arc::new(riscv::Plic::new(base_vaddr?)),
            _ => return Err(DeviceError::NotSupported),
        });

        Ok(DevWithInterrupt {
            phandle,
            interrupt_cells,
            interrupts_extended,
            dev,
        })
    }

    /// Parse nodes for virtio devices over MMIO.
    #[cfg(feature = "virtio")]
    fn parse_virtio(&self, node: &Node, props: &InheritProps) -> DeviceResult<DevWithInterrupt> {
        use crate::virtio::*;
        use virtio_drivers::{DeviceType, VirtIOHeader};

        let interrupts_extended = parse_interrupts(node, props)?;
        let base_vaddr = parse_reg(node, props).and_then(|(paddr, size)| {
            self.io_mapper
                .query_or_map(paddr as usize, size as usize)
                .ok_or(DeviceError::NoResources)
        })?;
        let header = unsafe { &mut *(base_vaddr as *mut VirtIOHeader) };
        if !header.verify() {
            return Err(DeviceError::NotSupported);
        }
        info!(
            "device-tree: detected virtio device: vendor_id={:#X}, type={:?}",
            header.vendor_id(),
            header.device_type()
        );

        let dev = match header.device_type() {
            DeviceType::Block => Device::Block(Arc::new(VirtIoBlk::new(header)?)),
            DeviceType::GPU => Device::Display(Arc::new(VirtIoGpu::new(header)?)),
            DeviceType::Input => Device::Input(Arc::new(VirtIoInput::new(header)?)),
            DeviceType::Console => Device::Uart(Arc::new(VirtIoConsole::new(header)?)),
            _ => return Err(DeviceError::NotSupported),
        };

        Ok(DevWithInterrupt {
            phandle: None,
            interrupt_cells: None,
            interrupts_extended,
            dev,
        })
    }

    /// Parse nodes for UART devices.
    fn parse_uart(
        &self,
        node: &Node,
        comp: &StringList,
        props: &InheritProps,
    ) -> DeviceResult<DevWithInterrupt> {
        let interrupts_extended = parse_interrupts(node, props)?;
        let base_vaddr = parse_reg(node, props).and_then(|(paddr, size)| {
            self.io_mapper
                .query_or_map(paddr as usize, size as usize)
                .ok_or(DeviceError::NoResources)
        });

        use crate::uart::*;
        let dev = Device::Uart(match comp {
            c if c.contains("ns16550a") => {
                Arc::new(unsafe { Uart16550Mmio::<u8>::new(base_vaddr?) })
            }
            _ => return Err(DeviceError::NotSupported),
        });

        Ok(DevWithInterrupt {
            phandle: None,
            interrupt_cells: None,
            interrupts_extended,
            dev,
        })
    }
}

/// Register interrupts for `dev` according to its interrupt parent, which can
/// be found from the phandle-device mapping.
fn register_interrupt(
    dev: &DevWithInterrupt,
    dev_list: &[DevWithInterrupt],
    intc_map: &BTreeMap<u32, usize>,
) -> DeviceResult {
    let mut pos = 0;
    while pos < dev.interrupts_extended.len() {
        let parent = dev.interrupts_extended[pos];
        // find the interrupt parent in `dev_list`
        if let Some(intc) = intc_map.get(&parent).map(|&i| &dev_list[i]) {
            let cells = intc.interrupt_cells.ok_or(DeviceError::InvalidParam)?;
            if let Device::Irq(irq) = &intc.dev {
                // get irq_num from the `interrupts_extended` property
                let irq_num = dev.interrupts_extended[pos + 1] as usize;
                if irq_num != 0xffff_ffff {
                    info!(
                        "device-tree: register interrupts for {:?}: {:?}, irq_num={:#x}",
                        intc.dev, dev.dev, irq_num
                    );
                    irq.register_device(irq_num, dev.dev.inner())?;
                    // enable the interrupt after registration
                    irq.unmask(irq_num)?;
                }
            } else {
                warn!(
                    "device-tree: node with phandle {:#x} is not an interrupt-controller",
                    parent
                );
                return Err(DeviceError::InvalidParam);
            }
            // process the next interrupt parent
            pos += 1 + cells as usize;
        } else {
            warn!(
                "device-tree: no such node with phandle {:#x} as the interrupt-parent",
                parent
            );
            return Err(DeviceError::InvalidParam);
        }
    }
    Ok(())
}
