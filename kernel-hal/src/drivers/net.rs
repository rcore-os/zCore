use super::Driver;
use alloc::string::String;
use alloc::vec::Vec;
// use core::any::Any;
use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr, Ipv4Address};

/*
 * 定义trait
 * 定义struct并实现该trait
 * 该struct的函数用#[linkage = "weak"]
 *
 * 在底层再实现一遍该trait，并替代以上的struct的函数
 */

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
    fn poll(&self) {
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
}

use downcast_rs::impl_downcast;
impl_downcast!(sync NetDriver);

// little hack, see https://users.rust-lang.org/t/how-to-downcast-from-a-trait-any-to-a-struct/11219/3
// pub trait AsAny : Sync + Send {
//     fn as_any(&self) -> &dyn Any;
// }

// impl<T: Any + Send + Sync> AsAny for T {
//     fn as_any(&self) -> &dyn Any {
//         self
//     }
// }
