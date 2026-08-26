// May need move to drivers
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

    // ip 地址
    let ip_addrs = vec![
        IpCidr::new(IpAddress::v4(127, 0, 0, 1), 8),
        IpCidr::new(IpAddress::v6(0, 0, 0, 0, 0, 0, 0, 1), 128),
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

pub fn get_net_device() -> Vec<Arc<dyn NetScheme>> {
    let mut devices = all_net().as_vec().clone();
    // Real NICs first; loopback last (matches Linux ifindex 1 = first Ethernet).
    devices.sort_by_key(|d| if d.get_ifname() == "loopback" { 1 } else { 0 });
    devices
}

// ---------------------------------------------------------------------------
// Network RX waker registry
// ---------------------------------------------------------------------------
// TCP read futures register a Waker here before sleeping.
// E1000eInterface::poll() calls wake_net_rx_waiters() after iface.poll()
// so any task waiting for RX data is woken immediately instead of after 5 ms.

use core::sync::atomic::{AtomicU64, Ordering};
use core::task::Waker;
use lazy_static::lazy_static;

lazy_static! {
    static ref NET_RX_WAKERS: Mutex<Vec<Waker>> = Mutex::new(Vec::new());
}

/// Prevent unbounded registry growth if futures are cancelled or repeatedly re-register.
const MAX_NET_RX_WAKERS: usize = 1024;

fn register_waker_once(wakers: &mut Vec<Waker>, waker: &Waker) {
    if wakers.iter().any(|w| w.will_wake(waker)) {
        return;
    }
    if wakers.len() >= MAX_NET_RX_WAKERS {
        wakers.remove(0);
    }
    wakers.push(waker.clone());
}

/// Register the current task's Waker to be notified when RX data arrives.
pub fn register_net_rx_waker(waker: Waker) {
    register_waker_once(&mut NET_RX_WAKERS.lock(), &waker);
}

/// After an IRQ-driven wake: keep the waker for the next sleep cycle.
pub fn retain_net_rx_waker(waker: &Waker) {
    NET_RX_WAKERS.lock().retain(|w| w.will_wake(waker));
}

/// Drop a wait's registration once the wait future is Ready/`Drop`ed.
pub fn clear_net_rx_waker(waker: &Waker) {
    NET_RX_WAKERS.lock().retain(|w| !w.will_wake(waker));
}

/// [diag] Rate-limit interval (ns) for net-waiter wakeups — coalesce the smoltcp
/// housekeeping busy-spin to ≤1 kHz without dropping any wake (TX/connect/DHCP
/// still delivered within the interval; waiters keep their fallback timers).
const NET_WAKE_MIN_INTERVAL_NS: u64 = 1_000_000;
static LAST_NET_WAKE_NS: AtomicU64 = AtomicU64::new(0);

/// Wake tasks registered for TCP/UDP RX (and any other net progress).
/// Rate-limited (see [`NET_WAKE_MIN_INTERVAL_NS`]); coalesced wakers stay
/// registered for the next allowed wake or their fallback timer.
pub fn wake_net_rx_waiters() {
    let now = crate::timer::timer_now().as_nanos() as u64;
    let last = LAST_NET_WAKE_NS.load(Ordering::Relaxed);
    if now.wrapping_sub(last) < NET_WAKE_MIN_INTERVAL_NS {
        return;
    }
    LAST_NET_WAKE_NS.store(now, Ordering::Relaxed);
    let wakers: Vec<Waker> = core::mem::take(&mut *NET_RX_WAKERS.lock());
    for w in wakers {
        w.wake();
    }
}

/// Shared waker slot for a parked net-RX timeout timer.
///
/// `timer_set` has no cancel API: Drop / Ready `take()` the waker so a late
/// tick is a no-op (no AtomicBool TOCTOU — that path corrupted heap and showed
/// up as null fn-ptr #PF in `timer_tick`).
fn arm_net_timeout_timer(
    slot: &mut Option<crate::timer_waker::TimerWakerSlot>,
    deadline: core::time::Duration,
    cx: &mut core::task::Context<'_>,
) {
    crate::timer_waker::ensure_timer_waker(slot, deadline, cx);
}

fn kill_timer(token: &mut Option<crate::timer_waker::TimerWakerSlot>) {
    crate::timer_waker::kill_timer_waker(token);
}

/// Future that resolves when either:
///   (a) `wake_net_rx_waiters()` is called (NIC received data), or
///   (b) the timeout expires.
///
/// On first poll it registers the waker in NET_RX_WAKERS **and** installs
/// a cancellable fallback timer, so progress is guaranteed even if a wake is
/// missed, without UAF if the NIC path wins the race.
pub struct NetRxOrTimeoutFuture {
    registered: bool,
    deadline: core::time::Duration,
    timer: Option<crate::timer_waker::TimerWakerSlot>,
    /// Clone of the waker parked in [`NET_RX_WAKERS`], for Drop cleanup.
    net_waker: Option<Waker>,
}

impl NetRxOrTimeoutFuture {
    pub fn new(timeout_ms: u64) -> Self {
        Self {
            registered: false,
            deadline: crate::timer::timer_now() + core::time::Duration::from_millis(timeout_ms),
            timer: None,
            net_waker: None,
        }
    }
}

impl Drop for NetRxOrTimeoutFuture {
    fn drop(&mut self) {
        kill_timer(&mut self.timer);
        if let Some(waker) = self.net_waker.take() {
            clear_net_rx_waker(&waker);
        }
    }
}

impl core::future::Future for NetRxOrTimeoutFuture {
    type Output = ();

    fn poll(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()> {
        // Second poll: woken by net data or timer → done.
        if self.registered {
            kill_timer(&mut self.timer);
            if let Some(waker) = self.net_waker.take() {
                clear_net_rx_waker(&waker);
            } else {
                clear_net_rx_waker(cx.waker());
            }
            return core::task::Poll::Ready(());
        }
        if crate::timer::timer_now() >= self.deadline {
            kill_timer(&mut self.timer);
            return core::task::Poll::Ready(());
        }
        // Register waker for immediate NIC notification.
        register_waker_once(&mut NET_RX_WAKERS.lock(), cx.waker());
        self.net_waker = Some(cx.waker().clone());
        // Fallback timer so we don't hang if the NIC wake is missed.
        let dl = self.deadline;
        arm_net_timeout_timer(&mut self.timer, dl, cx);
        self.registered = true;
        core::task::Poll::Pending
    }
}
