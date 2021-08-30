use crate::{frame_dealloc, hal_frame_alloc_contiguous, phys_to_virt, virt_to_phys, PAGE_SIZE};
use device_tree::util::SliceRead;
use device_tree::Node;
use log::*;
use virtio_drivers::{VirtIOBlk, VirtIOGpu, VirtIOHeader, VirtIOInput};

use super::super::PHYSICAL_MEMORY_OFFSET;

pub fn virtio_probe(node: &Node) {
    let reg = match node.prop_raw("reg") {
        Some(reg) => reg,
        _ => return,
    };
    let paddr = reg.as_slice().read_be_u64(0).unwrap();
    //let vaddr = phys_to_virt(paddr as usize);
    let size = reg.as_slice().read_be_u64(8).unwrap();
    // assuming one page
    assert_eq!(size as usize, PAGE_SIZE);

    /* 一一映射
    let vaddr = paddr;
    unsafe{
        PageTableImpl::active().map_if_not_exists(vaddr as usize, paddr as usize);
    }
    */
    let vaddr = paddr + PHYSICAL_MEMORY_OFFSET as u64;

    debug!("virtio_probe, paddr:{:#x}, vaddr:{:#x}", paddr, vaddr);

    let header = unsafe { &mut *(vaddr as *mut VirtIOHeader) };
    if !header.verify() {
        // only support legacy device
        return;
    }
    info!(
        "Detected virtio device with vendor id: {:#X}",
        header.vendor_id()
    );
    info!("Device tree node {:?}", node);
    match header.device_type() {
        //DeviceType::Network => virtio_net::init(header),
        virtio_drivers::DeviceType::Block => virtio_blk_init(header),
        virtio_drivers::DeviceType::Input => virtio_input_init(header),
        virtio_drivers::DeviceType::GPU => virtio_gpu_init(header),
        t => warn!("Unrecognized virtio device: {:?}", t),
    }
}

/// virtio_mmio
/////////
/// virtio_blk
use alloc::string::String;
use alloc::sync::Arc;

use alloc::format;

use super::{
    BlockDriver, DeviceType, Driver, GpuDriver, InputDriver, BLK_DRIVERS, DRIVERS, GPU_DRIVERS,
    INPUT_DRIVERS,
};

//use crate::{sync::SpinNoIrqLock as Mutex};
use spin::Mutex;

struct VirtIOBlkDriver(Mutex<VirtIOBlk<'static>>);
struct VirtIOGpuDriver(Mutex<VirtIOGpu<'static>>);
struct VirtIOInputDriver(Mutex<VirtIOInput<'static>>);

impl Driver for VirtIOBlkDriver {
    fn try_handle_interrupt(&self, _irq: Option<usize>) -> bool {
        self.0.lock().ack_interrupt()
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }

    fn get_id(&self) -> String {
        format!("virtio_block")
    }

    fn as_block(&self) -> Option<&dyn BlockDriver> {
        None
    }
}

impl BlockDriver for VirtIOBlkDriver {
    fn read_block(&self, block_id: usize, buf: &mut [u8]) -> bool {
        self.0.lock().read_block(block_id, buf).is_ok()
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) -> bool {
        self.0.lock().write_block(block_id, buf).is_ok()
    }
}

impl Driver for VirtIOGpuDriver {
    fn try_handle_interrupt(&self, _irq: Option<usize>) -> bool {
        self.0.lock().ack_interrupt()
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Gpu
    }

    fn get_id(&self) -> String {
        format!("virtio_gpu")
    }

    fn as_block(&self) -> Option<&dyn BlockDriver> {
        None
    }
}

impl GpuDriver for VirtIOGpuDriver {
    fn resolution(&self) -> (u32, u32) {
        self.0.lock().resolution()
    }

    fn setup_framebuffer(&self) -> (usize, usize) {
        let mut gpu = self.0.lock();
        let framebuffer = gpu.setup_framebuffer().expect("failed to get fb");
        let vaddr = framebuffer.as_ptr() as usize;
        let size = framebuffer.len();
        return (vaddr, size);
    }

    fn flush(&self) -> virtio_drivers::Result {
        self.0.lock().flush()
    }
}

impl Driver for VirtIOInputDriver {
    fn try_handle_interrupt(&self, _irq: Option<usize>) -> bool {
        self.0.lock().ack_interrupt().unwrap_or(false)
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Input
    }

    fn get_id(&self) -> String {
        format!("virtio_input")
    }

    fn as_block(&self) -> Option<&dyn BlockDriver> {
        None
    }
}

impl InputDriver for VirtIOInputDriver {
    fn mouse_xy(&self) -> (i32, i32) {
        self.0.lock().mouse_xy()
    }
}

pub fn virtio_blk_init(header: &'static mut VirtIOHeader) {
    let blk = VirtIOBlk::new(header).expect("failed to init blk driver");
    let driver = Arc::new(VirtIOBlkDriver(Mutex::new(blk)));
    DRIVERS.write().push(driver.clone());
    //IRQ_MANAGER.write().register_all(driver.clone());
    BLK_DRIVERS.write().push(driver);
}

static mut input_event_buf: [u64; 32] = [0u64; 32];

pub fn virtio_input_init(header: &'static mut VirtIOHeader) {
    let input = unsafe {
        VirtIOInput::new(header, &mut input_event_buf).expect("failed to init input driver")
    };
    let driver = Arc::new(VirtIOInputDriver(Mutex::new(input)));
    DRIVERS.write().push(driver.clone());
    INPUT_DRIVERS.write().push(driver);
}

pub fn virtio_gpu_init(header: &'static mut VirtIOHeader) {
    let gpu = VirtIOGpu::new(header).expect("failed to init gpu driver");
    let driver = Arc::new(VirtIOGpuDriver(Mutex::new(gpu)));
    DRIVERS.write().push(driver.clone());
    GPU_DRIVERS.write().push(driver);
}

/////////
/// virtio dma alloc/dealloc

#[no_mangle]
extern "C" fn virtio_dma_alloc(pages: usize) -> PhysAddr {
    let paddr = unsafe { hal_frame_alloc_contiguous(pages, 0).unwrap() };
    trace!("alloc DMA: paddr={:#x}, pages={}", paddr, pages);
    paddr
}

#[no_mangle]
extern "C" fn virtio_dma_dealloc(paddr: PhysAddr, pages: usize) -> i32 {
    for i in 0..pages {
        unsafe {
            frame_dealloc(&(paddr + i * PAGE_SIZE));
        }
    }
    trace!("dealloc DMA: paddr={:#x}, pages={}", paddr, pages);
    0
}

#[no_mangle]
extern "C" fn virtio_phys_to_virt(paddr: PhysAddr) -> VirtAddr {
    phys_to_virt(paddr)
}

#[no_mangle]
extern "C" fn virtio_virt_to_phys(vaddr: VirtAddr) -> PhysAddr {
    virt_to_phys(vaddr)
}

type VirtAddr = usize;
type PhysAddr = usize;
