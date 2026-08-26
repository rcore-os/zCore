use super::Scheme;
use crate::{DeviceError, DeviceResult};
use alloc::string::String;
use alloc::vec::Vec;
use smoltcp::wire::{EthernetAddress, IpCidr, Ipv4Cidr};

#[derive(Debug, Clone, Default)]
pub struct NetStats {
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
    /// RX errors / drops (driver-specific; shown in /proc/net/dev).
    pub rx_errors: u64,
    pub rx_dropped: u64,
    pub tx_errors: u64,
    pub tx_dropped: u64,
}

#[derive(Debug, Clone)]
pub struct RouteInfo {
    pub dst: IpCidr,
    pub gateway: Option<smoltcp::wire::IpAddress>,
}

pub trait NetScheme: Scheme {
    fn recv(&self, buf: &mut [u8]) -> DeviceResult<usize>;
    fn send(&self, buf: &[u8]) -> DeviceResult<usize>;
    fn can_recv(&self) -> bool {
        true
    }
    fn can_send(&self) -> bool {
        true
    }
    fn get_mac(&self) -> EthernetAddress;
    fn get_ifname(&self) -> String;
    fn get_ip_address(&self) -> Vec<IpCidr>;
    fn set_ipv4_address(&self, _cidr: Ipv4Cidr) -> DeviceResult {
        Err(DeviceError::NotSupported)
    }
    fn add_ip_address(&self, _cidr: IpCidr) -> DeviceResult {
        Err(DeviceError::NotSupported)
    }
    fn remove_ip_address(&self, _cidr: IpCidr) -> DeviceResult {
        Err(DeviceError::NotSupported)
    }
    fn add_route(&self, _cidr: IpCidr, _gateway: Option<smoltcp::wire::IpAddress>) -> DeviceResult {
        Err(DeviceError::NotSupported)
    }
    fn del_route(&self, _cidr: IpCidr, _gateway: Option<smoltcp::wire::IpAddress>) -> DeviceResult {
        Err(DeviceError::NotSupported)
    }
    fn get_routes(&self) -> Vec<RouteInfo> {
        Vec::new()
    }
    fn get_stats(&self) -> NetStats {
        NetStats::default()
    }
    fn get_mtu(&self) -> usize {
        1500
    }
    fn get_arp_content(&self) -> String {
        String::new()
    }
    /// Import a resolved IPv4/IPv6 neighbor into smoltcp's cache (for TCP/UDP egress).
    fn seed_neighbor(
        &self,
        _protocol: smoltcp::wire::IpAddress,
        _hardware: EthernetAddress,
    ) -> DeviceResult {
        Ok(())
    }
    fn poll(&self) -> DeviceResult;
    /// SIOCSIFFLAGS / admin up — drivers may re-probe link (default no-op).
    fn refresh_link(&self) -> DeviceResult {
        Ok(())
    }
    /// Physical carrier up (for SIOCGIFFLAGS IFF_LOWER_UP / RUNNING).
    fn link_carrier_up(&self) -> bool {
        true
    }
}
