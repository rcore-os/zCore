// smoltcp
use smoltcp::iface::{Interface, InterfaceBuilder, NeighborCache, Route, Routes};
use smoltcp::phy::{Loopback, Medium};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr, Ipv4Address};

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use alloc::sync::Arc;
use kernel_hal::drivers::{DeviceType, Driver, NetDriver, NET_DRIVERS, SOCKETS};

use alloc::string::String;
use spin::Mutex;

#[derive(Clone)]
pub struct LoopbackInterface {
    pub iface: Arc<Mutex<Interface<'static, Loopback>>>,
    pub name: String,
}

impl Driver for LoopbackInterface {
    fn try_handle_interrupt(&self, irq: Option<usize>) -> bool {
        return false;
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Net
    }

    fn get_id(&self) -> String {
        String::from("loopback")
    }

    fn as_net(&self) -> Option<&dyn NetDriver> {
        Some(self)
    }
}

impl NetDriver for LoopbackInterface {
    fn get_mac(&self) -> EthernetAddress {
        self.iface.lock().ethernet_addr()
    }

    // get ip addresses
    fn get_ip_addresses(&self) -> Vec<IpCidr> {
        unimplemented!()
    }

    fn ipv4_address(&self) -> Option<Ipv4Address> {
        self.iface.lock().ipv4_address()
    }

    fn poll(&self) {
        let timestamp = Instant::from_millis(0);
        let mut sockets = SOCKETS.lock();
        match self.iface.lock().poll(&mut sockets, timestamp) {
            Ok(_) => {}
            Err(err) => {
                debug!("poll got err {}", err);
            }
        }
    }

    fn send(&self, data: &[u8]) -> Option<usize> {
        unimplemented!()
    }

    fn get_arp(&self, ip: IpAddress) -> Option<EthernetAddress> {
        /*
        let iface = self.iface.lock();
        let cache = iface.neighbor_cache();
        cache.lookup(&ip, Instant::from_millis(0))
        */
        unimplemented!()
    }
}

pub fn init(name: String) {
    warn!("loopback");
    // 初始化 一个 协议栈
    // 从外界 接受 一些 配置 参数 如果 没有 选择 默认 的

    // 网络 设备
    // 默认 loopback
    let loopback = Loopback::new(Medium::Ethernet);

    // 为 设备 分配 网络 身份

    // 物理地址
    let mac: [u8; 6] = [0x52, 0x54, 0x98, 0x76, 0x54, 0x32];
    let ethernet_addr = EthernetAddress::from_bytes(&mac);
    // ip 地址
    let ip_addrs = [IpCidr::new(IpAddress::v4(127, 0, 0, 1), 24)];
    // let ip_addrs = [IpCidr::new(IpAddress::v4(10, 0, 2, 15), 24)];
    // 路由
    let default_gateway = Ipv4Address::new(127, 0, 0, 1);
    // let default_gateway = Ipv4Address::new(10, 0, 2, 2);
    static mut routes_storage: [Option<(IpCidr, Route)>; 1] = [None; 1];
    let mut routes = unsafe { Routes::new(&mut routes_storage[..]) };
    routes.add_default_ipv4_route(default_gateway).unwrap();
    // arp缓存
    let neighbor_cache = NeighborCache::new(BTreeMap::new());

    // 设置 主要 设置 iface
    let iface = InterfaceBuilder::new(loopback)
        .ethernet_addr(ethernet_addr)
        .ip_addrs(ip_addrs)
        .routes(routes)
        .neighbor_cache(neighbor_cache)
        .finalize();

    let loopback_iface = LoopbackInterface {
        iface: Arc::new(Mutex::new(iface)),
        name,
    };
    let driver = Arc::new(loopback_iface);
    NET_DRIVERS.write().push(driver);
}
