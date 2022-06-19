//! Device drivers.

use alloc::{sync::Arc, vec::Vec};
use core::convert::From;

use lock::{RwLock, RwLockReadGuard};

use zcore_drivers::scheme::{
    BlockScheme, DisplayScheme, InputScheme, IrqScheme, NetScheme, Scheme, UartScheme,
};
use zcore_drivers::{Device, DeviceError};

/// Re-exported modules from crate [`zcore_drivers`].
pub use zcore_drivers::{prelude, scheme};

/// A wrapper of a device array with the same [`Scheme`].
pub struct DeviceList<T: Scheme + ?Sized>(RwLock<Vec<Arc<T>>>);

impl<T: Scheme + ?Sized> DeviceList<T> {
    fn add(&self, dev: Arc<T>) {
        self.0.write().push(dev);
    }

    /// Convert self into a vector.
    pub fn as_vec(&self) -> RwLockReadGuard<'_, Vec<Arc<T>>> {
        self.0.read()
    }

    /// Returns the device at given position, or `None` if out of bounds.
    pub fn try_get(&self, idx: usize) -> Option<Arc<T>> {
        self.0.read().get(idx).cloned()
    }

    /// Returns the device with the given name, or `None` if not found.
    pub fn find(&self, name: &str) -> Option<Arc<T>> {
        self.0.read().iter().find(|d| d.name() == name).cloned()
    }

    /// Returns the first device of this device array, or `None` if it is empty.
    pub fn first(&self) -> Option<Arc<T>> {
        self.try_get(0)
    }

    /// Returns the first device of this device array.
    ///
    /// # Panic
    ///
    /// Panics if the array is empty.
    pub fn first_unwrap(&self) -> Arc<T> {
        self.first()
            .unwrap_or_else(|| panic!("device not initialized: {}", core::any::type_name::<T>()))
    }
}

impl<T: Scheme + ?Sized> Default for DeviceList<T> {
    fn default() -> Self {
        Self(RwLock::new(Vec::new()))
    }
}

#[derive(Default)]
struct AllDeviceList {
    block: DeviceList<dyn BlockScheme>,
    display: DeviceList<dyn DisplayScheme>,
    input: DeviceList<dyn InputScheme>,
    irq: DeviceList<dyn IrqScheme>,
    net: DeviceList<dyn NetScheme>,
    uart: DeviceList<dyn UartScheme>,
}

impl AllDeviceList {
    pub fn add_device(&self, dev: Device) {
        match dev {
            Device::Block(d) => self.block.add(d),
            Device::Display(d) => self.display.add(d),
            Device::Input(d) => self.input.add(d),
            Device::Irq(d) => self.irq.add(d),
            Device::Net(d) => self.net.add(d),
            Device::Uart(d) => self.uart.add(d),
        }
    }
}

lazy_static! {
    static ref DEVICES: AllDeviceList = AllDeviceList::default();
}

pub(crate) fn add_device(dev: Device) {
    DEVICES.add_device(dev)
}

/// Returns all devices which implement the [`BlockScheme`].
pub fn all_block() -> &'static DeviceList<dyn BlockScheme> {
    &DEVICES.block
}

/// Returns all devices which implement the [`DisplayScheme`].
pub fn all_display() -> &'static DeviceList<dyn DisplayScheme> {
    &DEVICES.display
}

/// Returns all devices which implement the [`InputScheme`].
pub fn all_input() -> &'static DeviceList<dyn InputScheme> {
    &DEVICES.input
}

/// Returns all devices which implement the [`IrqScheme`].
pub fn all_irq() -> &'static DeviceList<dyn IrqScheme> {
    &DEVICES.irq
}

/// Returns all devices which implement the [`NetScheme`].
pub fn all_net() -> &'static DeviceList<dyn NetScheme> {
    &DEVICES.net
}

/// Returns all devices which implement the [`UartScheme`].
pub fn all_uart() -> &'static DeviceList<dyn UartScheme> {
    &DEVICES.uart
}

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

#[cfg(not(feature = "libos"))]
mod drivers_ffi {
    use crate::{PhysAddr, VirtAddr, KCONFIG, KHANDLER, PAGE_SIZE};

    #[no_mangle]
    extern "C" fn drivers_dma_alloc(pages: usize) -> PhysAddr {
        let paddr = KHANDLER.frame_alloc_contiguous(pages, 0).unwrap();
        trace!("alloc DMA: paddr={:#x}, pages={}", paddr, pages);
        paddr
    }

    #[no_mangle]
    extern "C" fn drivers_dma_dealloc(paddr: PhysAddr, pages: usize) -> i32 {
        for i in 0..pages {
            KHANDLER.frame_dealloc(paddr + i * PAGE_SIZE);
        }
        trace!("dealloc DMA: paddr={:#x}, pages={}", paddr, pages);
        0
    }

    #[no_mangle]
    extern "C" fn drivers_phys_to_virt(paddr: PhysAddr) -> VirtAddr {
        paddr + KCONFIG.phys_to_virt_offset
    }

    #[no_mangle]
    extern "C" fn drivers_virt_to_phys(vaddr: VirtAddr) -> PhysAddr {
        vaddr - KCONFIG.phys_to_virt_offset
    }

    use crate::hal_fn::timer::timer_now;
    #[no_mangle]
    extern "C" fn drivers_timer_now_as_micros() -> u64 {
        timer_now().as_micros() as _
    }
}
