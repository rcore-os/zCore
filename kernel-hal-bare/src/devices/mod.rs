pub mod bus;
pub mod net;
pub use net::*;


use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

use lazy_static::lazy_static;

use spin::RwLock;

use pci::Location;

// use crate::irq::*;
use kernel_hal::NetDriver;
use kernel_hal::Driver;

lazy_static! {
    pub static ref DRIVERS: RwLock<Vec<Arc<dyn Driver>>> = RwLock::new(Vec::new());
    pub static ref NET_DRIVERS: RwLock<Vec<Arc<dyn NetDriver>>> = RwLock::new(Vec::new());
    // pub static ref IRQ_MANAGER: RwLock<IrqManager> = RwLock::new(IrqManager::new(true));
    pub static ref PCI_DRIVERS: RwLock<BTreeMap<Location, Arc<dyn Driver>>> =RwLock::new(BTreeMap::new());
}


pub fn devices_init() {
    bus::pci::init();
}

// #[export_name = "hal_driver"]
// pub fn get_driver() -> Vec<Arc<dyn Driver>> {
//     DRIVERS.read().clone()
// }

#[export_name = "hal_get_driver"]
pub fn get_net_driver() -> Vec<Arc<dyn NetDriver>> {
    NET_DRIVERS.read().clone()
}