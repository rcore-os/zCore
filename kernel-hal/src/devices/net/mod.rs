pub mod e1000;
pub use e1000::*;

pub use super::*;
use alloc::string::String;
use alloc::vec::Vec;
use smoltcp::phy::DeviceCapabilities;
use smoltcp::socket::SocketSet;
use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr, Ipv4Address};

pub trait NetDriver: Driver {
    // get mac address for this device
    fn get_mac(&self) -> EthernetAddress {
        unimplemented!("not a net driver")
    }

    // get interface name for this device
    fn get_ifname(&self) -> String {
        unimplemented!("not a net driver")
    }

    // get ip addresses
    fn get_ip_addresses(&self) -> Vec<IpCidr> {
        unimplemented!("not a net driver")
    }

    // get ipv4 address
    fn ipv4_address(&self) -> Option<Ipv4Address> {
        unimplemented!("not a net driver")
    }

    // manually trigger a poll, use it after sending packets
    fn poll(&self, _socketset: &Mutex<SocketSet>) {
        unimplemented!("not a net driver")
    }

    // send an ethernet frame, only use it when necessary
    fn send(&self, _data: &[u8]) -> Option<usize> {
        unimplemented!("not a net driver")
    }

    // get mac address from ip address in arp table
    fn get_arp(&self, _ip: IpAddress) -> Option<EthernetAddress> {
        unimplemented!("not a net driver")
    }

    fn get_device_cap(&self) -> DeviceCapabilities {
        unimplemented!("not a net driver")
    }
}
use downcast_rs::impl_downcast;
impl_downcast!(sync NetDriver);
