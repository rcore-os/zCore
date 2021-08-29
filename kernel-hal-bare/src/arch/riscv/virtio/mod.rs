use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use rcore_fs::dev::{self, BlockDevice, DevError};
use spin::RwLock;

//pub use block::BlockDriver;

/// Block device
pub mod virtio;

/// Device tree
pub mod device_tree;

#[derive(Debug, Eq, PartialEq)]
pub enum DeviceType {
    Net,
    Gpu,
    Input,
    Block,
    Rtc,
    Serial,
    Intc,
}

pub trait Driver: Send + Sync {
    // if interrupt belongs to this driver, handle it and return true
    // return false otherwise
    // irq number is provided when available
    // driver should skip handling when irq number is mismatched
    fn try_handle_interrupt(&self, irq: Option<usize>) -> bool;

    // return the correspondent device type, see DeviceType
    fn device_type(&self) -> DeviceType;

    // get unique identifier for this device
    // should be different for each instance
    fn get_id(&self) -> String;

    // trait casting
    /*
    fn as_net(&self) -> Option<&dyn NetDriver> {
        None
    }
    */

    fn as_block(&self) -> Option<&dyn BlockDriver> {
        None
    }

    /*
    fn as_rtc(&self) -> Option<&dyn RtcDriver> {
        None
    }
    */
}

/////////
pub trait BlockDriver: Driver {
    fn read_block(&self, _block_id: usize, _buf: &mut [u8]) -> bool {
        unimplemented!("not a block driver")
    }

    fn write_block(&self, _block_id: usize, _buf: &[u8]) -> bool {
        unimplemented!("not a block driver")
    }
}

pub trait GpuDriver: Driver {
    fn resolution(&self) -> (u32, u32) {
        unimplemented!("not a gpu driver")
    }

    fn setup_framebuffer(&self) -> (usize, usize) {
        unimplemented!("not a gpu driver")
    }

    fn flush(&self) -> virtio_drivers::Result {
        unimplemented!("not a gpu driver")
    }
}

pub trait InputDriver: Driver {
    fn mouse_xy(&self) -> (i32, i32) {
        unimplemented!("not a input driver")
    }
}
/////////

lazy_static! {
    // NOTE: RwLock only write when initializing drivers
    pub static ref DRIVERS: RwLock<Vec<Arc<dyn Driver>>> = RwLock::new(Vec::new());
    pub static ref BLK_DRIVERS: RwLock<Vec<Arc<dyn BlockDriver>>> = RwLock::new(Vec::new());
    pub static ref INPUT_DRIVERS: RwLock<Vec<Arc<dyn InputDriver>>> = RwLock::new(Vec::new());
    pub static ref GPU_DRIVERS: RwLock<Vec<Arc<dyn GpuDriver>>> = RwLock::new(Vec::new());
    //pub static ref IRQ_MANAGER: RwLock<irq::IrqManager> = RwLock::new(irq::IrqManager::new(true));
}

pub struct BlockDriverWrapper(pub Arc<dyn BlockDriver>);

impl BlockDevice for BlockDriverWrapper {
    const BLOCK_SIZE_LOG2: u8 = 9; // 512
    fn read_at(&self, block_id: usize, buf: &mut [u8]) -> dev::Result<()> {
        match self.0.read_block(block_id, buf) {
            true => Ok(()),
            false => Err(DevError),
        }
    }

    fn write_at(&self, block_id: usize, buf: &[u8]) -> dev::Result<()> {
        match self.0.write_block(block_id, buf) {
            true => Ok(()),
            false => Err(DevError),
        }
    }

    fn sync(&self) -> dev::Result<()> {
        Ok(())
    }
}

lazy_static! {
    // Write only once at boot
    pub static ref CMDLINE: RwLock<String> = RwLock::new(String::new());
}
