// May need move to drivers
use core::task::Waker;

use smoltcp::{
    iface::{InterfaceBuilder, Route, Routes},
    phy::Medium,
    wire::{IpAddress, IpCidr},
};

use alloc::vec;
use alloc::vec::Vec;

// use zcore_drivers::net::get_sockets;
use alloc::sync::Arc;

use alloc::string::String;
use lock::Mutex;

use crate::drivers::add_device;
use crate::drivers::all_net;
use zcore_drivers::net::{LoopbackDevice, LoopbackInterface};
use zcore_drivers::scheme::{NetScheme, NetStats};
use zcore_drivers::Device;

pub fn init() {
    let name = String::from("loopback");
    warn!("name : {}", name);
    // 初始化 一个 协议栈
    // 从外界 接受 一些 配置 参数 如果 没有 选择 默认 的

    let stats = Arc::new(Mutex::new(NetStats::default()));

    // 网络 设备
    // 默认 loopback
    let loopback = LoopbackDevice::new(Medium::Ip, stats.clone());

    // 为 设备 分配 网络 身份

    // ip 地址: 127.0.0.1/8 matches the whole loopback subnet
    let ip_addrs = vec![
        IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8),
        IpCidr::new(IpAddress::v6(0, 0, 0, 0, 0, 0, 0, 1), 128),
        IpCidr::new(IpAddress::v4(0, 0, 0, 0), 0),
        IpCidr::new(IpAddress::v4(0, 0, 0, 0), 0),
    ];

    // Loopback does not require any default route/gateway
    static mut ROUTES_STORAGE: [Option<(IpCidr, Route)>; 4] = [None; 4];
    let routes = unsafe { Routes::new(&mut ROUTES_STORAGE[..]) };

    let ip_addrs_clone = ip_addrs.clone();
    // 设置 主要 设置 iface
    let iface = InterfaceBuilder::new(loopback)
        .ip_addrs(ip_addrs)
        .routes(routes)
        .finalize();

    let loopback_iface = LoopbackInterface {
        iface: Arc::new(Mutex::new(iface)),
        name,
        stats,
        routes: Arc::new(Mutex::new(vec![])),
        ip_addrs: Arc::new(Mutex::new(ip_addrs_clone)),
    };
    // loopback_iface
    let dev = Device::Net(Arc::new(loopback_iface));
    add_device(dev);
}

/// libOS host build: RX wakers are no-ops (real NIC path is bare-metal only).
pub fn register_net_rx_waker(_waker: Waker) {}

/// libOS host build: RX wakers are no-ops.
pub fn retain_net_rx_waker(_waker: &Waker) {}

/// libOS host build: RX wakers are no-ops.
pub fn clear_net_rx_waker(_waker: &Waker) {}

/// libOS host build: RX wakers are no-ops.
pub fn wake_net_rx_waiters() {}

pub fn get_net_device() -> Vec<Arc<dyn NetScheme>> {
    let mut devices = all_net().as_vec().clone();
    devices.sort_by_key(|d| if d.get_ifname() == "loopback" { 1 } else { 0 });
    devices
}

pub struct NetRxOrTimeoutFuture {
    deadline: core::time::Duration,
}

impl NetRxOrTimeoutFuture {
    pub fn new(timeout_ms: u64) -> Self {
        Self {
            deadline: crate::timer::timer_now() + core::time::Duration::from_millis(timeout_ms),
        }
    }
}

impl core::future::Future for NetRxOrTimeoutFuture {
    type Output = ();

    fn poll(
        self: core::pin::Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()> {
        if crate::timer::timer_now() >= self.deadline {
            core::task::Poll::Ready(())
        } else {
            core::task::Poll::Pending
        }
    }
}
