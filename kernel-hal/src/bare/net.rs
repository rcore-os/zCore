// May need move to drivers
use smoltcp::{
    iface::{InterfaceBuilder, NeighborCache, Route, Routes},
    phy::{Loopback, Medium},
    wire::{EthernetAddress, IpAddress, IpCidr, Ipv4Address},
};

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

// use zcore_drivers::net::get_sockets;
use alloc::sync::Arc;

use alloc::string::String;
use lock::Mutex;

use crate::drivers::add_device;
use crate::drivers::all_net;
use zcore_drivers::net::LoopbackInterface;
use zcore_drivers::scheme::NetScheme;
use zcore_drivers::Device;

pub fn init() {
    let name = String::from("loopback");
    warn!("name : {}", name);
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
    // qemu
    // let ip_addrs = [IpCidr::new(IpAddress::v4(10, 0, 2, 15), 24)];
    // 路由
    let default_gateway = Ipv4Address::new(127, 0, 0, 1);
    // qemu route
    // let default_gateway = Ipv4Address::new(10, 0, 2, 2);
    static mut ROUTES_STORAGE: [Option<(IpCidr, Route)>; 1] = [None; 1];
    let mut routes = unsafe { Routes::new(&mut ROUTES_STORAGE[..]) };
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
    // loopback_iface
    let dev = Device::Net(Arc::new(loopback_iface));
    add_device(dev);
}

pub fn get_net_device() -> Vec<Arc<dyn NetScheme>> {
    all_net().as_vec().clone()
}
