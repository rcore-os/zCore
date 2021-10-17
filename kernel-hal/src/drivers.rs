use alloc::{sync::Arc, vec::Vec};
use core::convert::From;

use spin::{RwLock, RwLockReadGuard};

use zcore_drivers::scheme::{
    BlockScheme, DisplayScheme, InputScheme, IrqScheme, NetScheme, UartScheme,
};
use zcore_drivers::{Device, DeviceError};

pub use zcore_drivers::{prelude, scheme};

lazy_static! {
    static ref DEVICES: DeviceList = DeviceList::default();
}

#[derive(Default)]
struct DeviceList {
    block: RwLock<Vec<Arc<dyn BlockScheme>>>,
    display: RwLock<Vec<Arc<dyn DisplayScheme>>>,
    input: RwLock<Vec<Arc<dyn InputScheme>>>,
    irq: RwLock<Vec<Arc<dyn IrqScheme>>>,
    net: RwLock<Vec<Arc<dyn NetScheme>>>,
    uart: RwLock<Vec<Arc<dyn UartScheme>>>,
}

impl DeviceList {
    pub fn add_device(&self, dev: Device) {
        match dev {
            Device::Block(d) => self.block.write().push(d),
            Device::Display(d) => self.display.write().push(d),
            Device::Input(d) => self.input.write().push(d),
            Device::Irq(d) => self.irq.write().push(d),
            Device::Net(d) => self.net.write().push(d),
            Device::Uart(d) => self.uart.write().push(d),
        }
    }
}

macro_rules! device_fn_def {
    ($dev:ident, $scheme:path) => {
        pub mod $dev {
            use super::*;

            pub fn all<'a>() -> RwLockReadGuard<'a, Vec<Arc<dyn $scheme>>> {
                DEVICES.$dev.read()
            }

            pub fn try_get(idx: usize) -> Option<Arc<dyn $scheme>> {
                all().get(idx).cloned()
            }

            pub fn find(name: &str) -> Option<Arc<dyn $scheme>> {
                all().iter().find(|d| d.name() == name).cloned()
            }

            pub fn first() -> Option<Arc<dyn $scheme>> {
                try_get(0)
            }

            pub fn first_unwrap() -> Arc<dyn $scheme> {
                first().expect(concat!(stringify!($dev), " device not initialized!"))
            }
        }
    };
}

pub(crate) fn add_device(dev: Device) {
    DEVICES.add_device(dev)
}

device_fn_def!(block, BlockScheme);
device_fn_def!(display, DisplayScheme);
device_fn_def!(input, InputScheme);
device_fn_def!(irq, IrqScheme);
device_fn_def!(net, NetScheme);
device_fn_def!(uart, UartScheme);

impl From<DeviceError> for crate::HalError {
    fn from(err: DeviceError) -> Self {
        warn!("{:?}", err);
        Self
    }
}

#[cfg(not(feature = "libos"))]
mod virtio_drivers_ffi {
    use crate::{PhysAddr, VirtAddr, KCONFIG, KHANDLER, PAGE_SIZE};

    #[no_mangle]
    extern "C" fn virtio_dma_alloc(pages: usize) -> PhysAddr {
        let paddr = KHANDLER.frame_alloc_contiguous(pages, 0).unwrap();
        trace!("alloc DMA: paddr={:#x}, pages={}", paddr, pages);
        paddr
    }

    #[no_mangle]
    extern "C" fn virtio_dma_dealloc(paddr: PhysAddr, pages: usize) -> i32 {
        for i in 0..pages {
            KHANDLER.frame_dealloc(paddr + i * PAGE_SIZE);
        }
        trace!("dealloc DMA: paddr={:#x}, pages={}", paddr, pages);
        0
    }

    #[no_mangle]
    extern "C" fn virtio_phys_to_virt(paddr: PhysAddr) -> VirtAddr {
        paddr + KCONFIG.phys_to_virt_offset
    }

    #[no_mangle]
    extern "C" fn virtio_virt_to_phys(vaddr: VirtAddr) -> PhysAddr {
        vaddr - KCONFIG.phys_to_virt_offset
    }
}
