mod bus;
pub use bus::*;

mod net;
pub use net::*;

// alloc
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
// smoltcp
use smoltcp::socket::SocketSet;
// spin
use spin::Mutex;
// downcast
use downcast_rs::DowncastSync;

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

pub trait Driver: DowncastSync + Send + Sync {
    // if interrupt belongs to this driver, handle it and return true
    // return false otherwise
    // irq number is provided when available
    // driver should skip handling when irq number is mismatched
    fn try_handle_interrupt(&self, irq: Option<usize>, socketset: &Mutex<SocketSet>) -> bool;

    // return the correspondent device type, see DeviceType
    fn device_type(&self) -> DeviceType;

    // get unique identifier for this device
    // should be different for each instance
    fn get_id(&self) -> String;

    // trait casting
    fn as_net(&self) -> Option<&dyn NetDriver> {
        None
    }
}

// function

#[linkage = "weak"]
#[export_name = "hal_driver"]
pub fn get_driver() -> Vec<Arc<dyn Driver>> {
    unimplemented!()
}

#[linkage = "weak"]
#[export_name = "hal_get_driver"]
pub fn get_net_driver() -> Vec<Arc<dyn NetDriver>> {
    unimplemented!()
}
