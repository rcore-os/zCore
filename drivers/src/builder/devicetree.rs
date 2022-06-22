// 解析设备树，创建已知的设备并为它们注册中断。
//
// 涉及到中断的设备包括：
//
// - 接收中断的中断控制器
// - 发出中断的设备
//
// 有效的中断控制器应该具有下列三个属性：
//
// - `interrupt-controller`: 指示这是一个中断控制器
// - `interrupt-cells`: 只是要向此控制器注册中断需要几个参数
// - `phandle`: 向此控制器注册中断时使用的一个号码，如果没有设备需要向它注册，可能不存在
//
// 设备注册中断需要 `interrupts_extended` 属性，这是一个 `Vec<u32>`，形式为 `[{phandle, ...,}*]`，
// 即控制器引用和控制器指定数量的参数。
//! Probe devices and create drivers from device tree.
//!
//! Specification: <https://github.com/devicetree-org/devicetree-specification/releases/download/v0.3/devicetree-specification-v0.3.pdf>.

use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};

use super::IoMapper;
use crate::{
    utils::devicetree::{
        parse_interrupts, parse_reg, Devicetree, InheritProps, InterruptsProp, Node, StringList,
    },
    Device, DeviceError, DeviceResult, VirtAddr,
};

const MODULE: &str = "device-tree";

type DevWithInterrupt = (Device, InterruptsProp);

/// 设备树中中断控制器特有的属性
struct IntcProps {
    phandle: u32,
    interrupt_cells: u32,
}

/// 查找表保存的中断控制器信息
struct Intc {
    index: usize,
    cells: usize,
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
        let mut intc_map = BTreeMap::new(); // phandle -> intc
        let mut dev_list = Vec::new(); // devices

        // 解析设备树
        self.dt.walk(&mut |node, comp, props| {
            debug!(
                "{MODULE}: parsing node {:?} with compatible {comp:?}",
                node.name
            );
            // parse interrupt controller
            let res = if node.has_prop("interrupt-controller") {
                self.parse_intc(node, comp, props).map(|(dev, intc)| {
                    intc_map.insert(
                        intc.phandle,
                        Intc {
                            index: dev_list.len(),
                            cells: intc.interrupt_cells as _,
                        },
                    );
                    dev
                })
            } else {
                // parse other device
                match comp {
                    #[cfg(feature = "virtio")]
                    c if c.contains("virtio,mmio") => self.parse_virtio(node, props),
                    c if c.contains("allwinner,sunxi-gmac") => {
                        self.parse_ethernet(node, comp, props)
                    }
                    c if c.contains("ns16550a") || c.contains("allwinner,sun20i-uart") => {
                        self.parse_uart(node, comp, props)
                    }
                    _ => Err(DeviceError::NotSupported),
                }
            };
            match res {
                Ok(dev) => dev_list.push(dev),
                Err(DeviceError::NotSupported) => {}
                Err(err) => warn!("{MODULE}: failed to parsing node {:?}: {err:?}", node.name),
            }
        });

        // 注册中断
        for (device, interrupts_extended) in &dev_list {
            let mut extended = interrupts_extended.as_slice();
            // 分解 interrupts_extended
            while let [phandle, irq_num, ..] = extended {
                if let Some(Intc { index, cells }) = intc_map.get(phandle) {
                    let (intc, _) = &dev_list[*index];
                    extended = &extended[1 + cells..];
                    if let Device::Irq(irq) = intc {
                        if *irq_num != 0xffff_ffff {
                            info!("{MODULE}: register interrupts for {intc:?}: {device:?}, irq_num={irq_num}");
                            if irq.register_device(*irq_num as _, device.inner()).is_ok() {
                                irq.unmask(*irq_num as _)?;
                            }
                        }
                    } else {
                        warn!("{MODULE}: node with phandle {phandle:#x} is not an interrupt-controller");
                        return Err(DeviceError::InvalidParam);
                    }
                } else {
                    warn!(
                        "{MODULE}: no such node with phandle {phandle:#x} as the interrupt-parent"
                    );
                    return Err(DeviceError::InvalidParam);
                }
            }
        }

        // 丢弃中断信息
        Ok(dev_list.into_iter().map(|(dev, _)| dev).collect())
    }
}

#[allow(unused_imports)]
#[allow(unused_variables)]
#[allow(unreachable_code)]
impl<M: IoMapper> DevicetreeDriverBuilder<M> {
    /// Parse nodes for interrupt controllers.
    fn parse_intc(
        &self,
        node: &Node,
        comp: &StringList,
        props: &InheritProps,
    ) -> DeviceResult<(DevWithInterrupt, IntcProps)> {
        let phandle = node
            .prop_u32("phandle")
            .map_err(|_| DeviceError::InvalidParam)?;
        let interrupt_cells = node
            .prop_u32("#interrupt-cells")
            .map_err(|_| DeviceError::InvalidParam)?;
        let interrupts_extended = parse_interrupts(node, props)?;
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

        Ok((
            (dev, interrupts_extended),
            IntcProps {
                phandle,
                interrupt_cells,
            },
        ))
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
            "{MODULE}: detected virtio device: vendor_id={:#X}, type={:?}",
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

        Ok((dev, interrupts_extended))
    }

    /// Parse nodes for Ethernet devices.
    fn parse_ethernet(
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
        info!("Ethernet gmac init ...");

        let irq_num = interrupts_extended[1];
        use crate::net::*;
        let dev = Device::Net(match comp {
            #[cfg(target_arch = "riscv64")]
            c if c.contains("allwinner,sunxi-gmac") => {
                Arc::new(rtlx_init(irq_num as usize, |paddr, size| {
                    self.io_mapper.query_or_map(paddr, size)
                })?)
            }
            _ => return Err(DeviceError::NotSupported),
        });

        Ok((dev, interrupts_extended))
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
            c if c.contains("allwinner,sun20i-uart") => {
                Arc::new(unsafe { Uart16550Mmio::<u32>::new(base_vaddr?) })
            }
            _ => return Err(DeviceError::NotSupported),
        });

        Ok((dev, interrupts_extended))
    }
}
