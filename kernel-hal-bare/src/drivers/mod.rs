pub mod device_tree;
pub mod irq;
pub mod serial;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::RwLock;

pub use serial::SerialDriver;

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
    // fn as_net(&self) -> Option<&dyn NetDriver> {
    //     None
    // }

    // fn as_block(&self) -> Option<&dyn BlockDriver> {
    //     None
    // }

    // fn as_rtc(&self) -> Option<&dyn RtcDriver> {
    //     None
    // }
}

lazy_static! {
    // NOTE: RwLock only write when initializing drivers
    pub static ref DRIVERS: RwLock<Vec<Arc<dyn Driver>>> = RwLock::new(Vec::new());
    // pub static ref NET_DRIVERS: RwLock<Vec<Arc<dyn NetDriver>>> = RwLock::new(Vec::new());
    // pub static ref BLK_DRIVERS: RwLock<Vec<Arc<dyn BlockDriver>>> = RwLock::new(Vec::new());
    // pub static ref RTC_DRIVERS: RwLock<Vec<Arc<dyn RtcDriver>>> = RwLock::new(Vec::new());
    pub static ref SERIAL_DRIVERS: RwLock<Vec<Arc<dyn SerialDriver>>> = RwLock::new(Vec::new());
    pub static ref IRQ_MANAGER: RwLock<irq::IrqManager> = RwLock::new(irq::IrqManager::new(true));
}
