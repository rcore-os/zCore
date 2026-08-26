use crate::net::{Provider, ProviderImpl};
use crate::scheme::{NetScheme, Scheme};
use crate::{Device, DeviceResult};
use crate::bus::pci_drivers::PciDriver;
use crate::builder::IoMapper;
use pci::{PCIDevice, BAR};
use alloc::sync::Arc;
use core::ptr::NonNull;
use ixgbe_driver::{IxgbeHal, IxgbeNic, PhysAddr as IxgbePhysAddr};
use lock::Mutex;

pub struct IxgbeHalImpl;

unsafe impl IxgbeHal for IxgbeHalImpl {
    fn dma_alloc(size: usize) -> (IxgbePhysAddr, NonNull<u8>) {
        let (vaddr, paddr) = ProviderImpl::alloc_dma(size);
        (paddr, NonNull::new(vaddr as *mut u8).unwrap())
    }

    unsafe fn dma_dealloc(paddr: IxgbePhysAddr, vaddr: NonNull<u8>, size: usize) -> i32 {
        ProviderImpl::dealloc_dma(vaddr.as_ptr() as usize, size);
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: IxgbePhysAddr, _size: usize) -> NonNull<u8> {
        NonNull::new(crate::net::phys_to_virt(paddr) as *mut u8).unwrap()
    }

    unsafe fn mmio_virt_to_phys(vaddr: NonNull<u8>, _size: usize) -> IxgbePhysAddr {
        crate::net::virt_to_phys(vaddr.as_ptr() as usize)
    }

    fn wait_until(duration: core::time::Duration) -> Result<(), &'static str> {
        let start = crate::net::timer_now_as_micros();
        let target = start + duration.as_micros() as u64;
        while crate::net::timer_now_as_micros() < target {
            core::hint::spin_loop();
        }
        Ok(())
    }
}

pub struct IxgbeInterface {
    nic: Mutex<IxgbeNic<IxgbeHalImpl, 1024, 1>>,
    name: alloc::string::String,
}

impl IxgbeInterface {
    pub fn init(
        name: alloc::string::String,
        _irq: usize,
        vaddr: usize,
        size: usize,
    ) -> DeviceResult<Self> {
        let nic = IxgbeNic::<IxgbeHalImpl, 1024, 1>::init(NonNull::new(vaddr as *mut u8).unwrap(), size)
            .map_err(|_| crate::DeviceError::NotSupported)?;
        Ok(Self {
            nic: Mutex::new(nic),
            name,
        })
    }
}

impl Scheme for IxgbeInterface {
    fn name(&self) -> &str {
        &self.name
    }

    fn handle_irq(&self, _vector: usize) {
        // ixgbe interrupt handling
    }
}

impl NetScheme for IxgbeInterface {
    fn recv(&self, buf: &mut [u8]) -> DeviceResult<usize> {
        let mut nic = self.nic.lock();
        if let Some(packet) = nic.recv() {
            let len = packet.len().min(buf.len());
            buf[..len].copy_from_slice(&packet[..len]);
            Ok(len)
        } else {
            Err(crate::DeviceError::NotReady)
        }
    }

    fn send(&self, buf: &[u8]) -> DeviceResult<usize> {
        let mut nic = self.nic.lock();
        nic.send(buf).map_err(|_| crate::DeviceError::NotReady)?;
        Ok(buf.len())
    }

    fn mac_address(&self) -> [u8; 6] {
        self.nic.lock().mac_address()
    }

    fn interface_name(&self) -> alloc::string::String {
        self.name.clone()
    }
}

pub struct IxgbeDriver;

impl PciDriver for IxgbeDriver {
    fn name(&self) -> &str {
        "ixgbe"
    }

    fn matched(&self, vendor_id: u16, device_id: u16) -> bool {
        vendor_id == 0x8086 && device_id == 0x10fb
    }

    fn init(&self, dev: &PCIDevice, mapper: &Option<Arc<dyn IoMapper>>, irq: Option<usize>) -> DeviceResult<Device> {
        if let Some(BAR::Memory(addr, len, _, _)) = dev.bars[0] {
            let vaddr = crate::net::phys_to_virt(addr as usize);
            let vector = irq.map(|idx| idx + 32).unwrap_or(0);
            let iface = IxgbeInterface::init(alloc::format!("eth{}", dev.loc.bus), vector, vaddr, len as usize)?;
            Ok(Device::Net(Arc::new(iface)))
        } else {
            Err(crate::DeviceError::NotSupported)
        }
    }
}
