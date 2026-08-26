use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use lock::Mutex;

use smoltcp::iface::*;
use smoltcp::phy::{self, Checksum, Device, DeviceCapabilities, Medium};
// use smoltcp::socket::SocketSet;
use smoltcp::time::Instant;
use smoltcp::wire::*;
use smoltcp::Result;

use super::realtek::rtl8211f::{self, RTL8211F};
use super::{timer_now_as_micros, ProviderImpl, PAGE_SIZE};

use crate::net::get_sockets;
use crate::scheme::{NetScheme, RouteInfo, Scheme};
use crate::{DeviceError, DeviceResult};

#[derive(Clone)]
pub struct RTLxDriver(Arc<Mutex<RTL8211F<ProviderImpl>>>);

#[derive(Clone)]
pub struct RTLxInterface {
    pub iface: Arc<Mutex<Interface<'static, RTLxDriver>>>,
    pub driver: RTLxDriver,
    pub routes: Arc<Mutex<Vec<RouteInfo>>>,
    pub name: String,
    pub irq: usize,
}

impl Scheme for RTLxInterface {
    fn name(&self) -> &str {
        "rtl8211f"
    }

    fn handle_irq(&self, irq: usize) {
        if irq != self.irq {
            // not ours, skip it
            return;
        }

        let status = self.driver.0.lock().interrupt_status();

        let handle_tx_rx = 3;
        if status == handle_tx_rx {
            let timestamp = Instant::from_micros(timer_now_as_micros() as i64);
            let sockets = get_sockets();
            let mut sockets = sockets.lock();

            self.driver.0.lock().int_disable();
            match self.iface.lock().poll(&mut sockets, timestamp) {
                Ok(b) => {
                    debug!("nic poll, is changed ?: {}", b);
                }
                Err(err) => {
                    error!("poll got err {}", err);
                }
            }
            self.driver.0.lock().int_enable();
            //return true;
        }
    }
}

impl NetScheme for RTLxInterface {
    fn get_mac(&self) -> EthernetAddress {
        self.iface.lock().ethernet_addr()
    }

    fn get_ifname(&self) -> String {
        self.name.clone()
    }

    fn get_ip_address(&self) -> Vec<IpCidr> {
        Vec::from(self.iface.lock().ip_addrs())
    }

    fn set_ipv4_address(&self, cidr: Ipv4Cidr) -> DeviceResult {
        let mut iface = self.iface.lock();
        iface.update_ip_addrs(|addrs| {
            let mut set_primary = false;
            for slot in addrs.iter_mut() {
                if let IpCidr::Ipv4(_) = slot {
                    if !set_primary {
                        *slot = IpCidr::Ipv4(cidr);
                        set_primary = true;
                    } else {
                        *slot = IpCidr::Ipv4(Ipv4Cidr::new(Ipv4Address::UNSPECIFIED, 0));
                    }
                }
            }
            if !set_primary {
                if let Some(slot) = addrs.iter_mut().next() {
                    *slot = IpCidr::Ipv4(cidr);
                }
            }
        });
        Ok(())
    }

    fn add_ip_address(&self, cidr: IpCidr) -> DeviceResult {
        let mut iface = self.iface.lock();
        iface.update_ip_addrs(|addrs| {
            if addrs.contains(&cidr) {
                return;
            }
            for slot in addrs.iter_mut() {
                if (slot.address().is_unspecified() && slot.prefix_len() == 0)
                    || (slot.address() == IpAddress::v4(240, 0, 0, 0) && slot.prefix_len() == 32)
                {
                    *slot = cidr;
                    return;
                }
            }
            if let Some(slot) = addrs.iter_mut().last() {
                *slot = cidr;
            }
        });
        Ok(())
    }

    fn remove_ip_address(&self, cidr: IpCidr) -> DeviceResult {
        let mut iface = self.iface.lock();
        iface.update_ip_addrs(|addrs| {
            for slot in addrs.iter_mut() {
                if *slot == cidr {
                    *slot = IpCidr::new(IpAddress::v4(0, 0, 0, 0), 0);
                    return;
                }
            }
        });
        Ok(())
    }

    fn add_route(&self, cidr: IpCidr, gateway: Option<IpAddress>) -> DeviceResult {
        let mut iface = self.iface.lock();
        match gateway {
            Some(IpAddress::Ipv4(gw)) => {
                if cidr.prefix_len() == 0 {
                    iface
                        .routes_mut()
                        .add_default_ipv4_route(gw)
                        .map_err(|_| DeviceError::IoError)?;
                }
                let mut routes = self.routes.lock();
                routes.retain(|r| !(matches!(r.dst, IpCidr::Ipv4(_)) && r.dst.prefix_len() == 0));
                routes.push(RouteInfo {
                    dst: cidr,
                    gateway: Some(IpAddress::Ipv4(gw)),
                });
            }
            Some(IpAddress::Ipv6(gw)) => {
                if cidr.prefix_len() == 0 {
                    iface
                        .routes_mut()
                        .add_default_ipv6_route(gw)
                        .map_err(|_| DeviceError::IoError)?;
                }
                let mut routes = self.routes.lock();
                routes.retain(|r| !(matches!(r.dst, IpCidr::Ipv6(_)) && r.dst.prefix_len() == 0));
                routes.push(RouteInfo {
                    dst: cidr,
                    gateway: Some(IpAddress::Ipv6(gw)),
                });
            }
            None => {
                self.routes.lock().push(RouteInfo { dst: cidr, gateway });
            }
            _ => {}
        }
        Ok(())
    }

    fn del_route(&self, cidr: IpCidr, _gateway: Option<IpAddress>) -> DeviceResult {
        let mut iface = self.iface.lock();
        if cidr.prefix_len() == 0 {
            match cidr {
                IpCidr::Ipv4(_) => {
                    let _ = iface.routes_mut().remove_default_ipv4_route();
                }
                IpCidr::Ipv6(_) => {
                    let _ = iface.routes_mut().remove_default_ipv6_route();
                }
                _ => {}
            }
        }
        self.routes.lock().retain(|r| r.dst != cidr);
        Ok(())
    }

    fn get_routes(&self) -> Vec<RouteInfo> {
        let iface = self.iface.lock();
        let mut res = Vec::new();

        res.extend(self.routes.lock().clone());

        for cidr in iface.ip_addrs() {
            match cidr {
                IpCidr::Ipv4(v4) if v4.prefix_len() > 0 => {
                    res.push(RouteInfo {
                        dst: IpCidr::Ipv4(v4.network()),
                        gateway: None,
                    });
                }
                IpCidr::Ipv6(v6) if v6.prefix_len() > 0 => {
                    res.push(RouteInfo {
                        dst: IpCidr::Ipv6(v6.network()),
                        gateway: None,
                    });
                }
                _ => {}
            }
        }
        res
    }

    fn poll(&self) -> DeviceResult {
        let timestamp = Instant::from_micros(timer_now_as_micros() as i64);
        // Disable interrupts while holding the SOCKETS and iface locks.
        // On real hardware the NIC fires a hardware interrupt as soon as a
        // frame lands in the DMA ring.  If that interrupt is delivered while
        // this thread already holds SOCKETS, handle_irq() will try to acquire
        // the same lock and spin forever, dead-locking the system.
        // The kernel-sync Mutex already keeps interrupts off for the duration
        // of the locked critical section (push_off/pop_off), so the NIC IRQ
        // cannot reenter while SOCKETS is held. Manual intr_off/on here would
        // desync the noff accounting and panic ("pop_off" / "RefCell already
        // borrowed") under SMP, so we rely on the Mutex alone.
        let sockets = get_sockets();
        let mut sockets = sockets.lock();
        let result = self.iface.lock().poll(&mut sockets, timestamp);
        // Release the SOCKETS guard promptly so interrupts (disabled by the
        // lock) are re-enabled as soon as the critical section ends.
        drop(sockets);
        match result {
            Ok(b) => {
                debug!("nic poll, is changed ?: {}", b);
                Ok(())
            }
            Err(err) => {
                error!("poll got err {}", err);
                Err(DeviceError::IoError)
            }
        }
    }

    fn recv(&self, buf: &mut [u8]) -> DeviceResult<usize> {
        if self.driver.0.lock().can_recv() {
            let (vec_recv, _rxcount) = self.driver.0.lock().geth_recv(1);
            // `copy_from_slice` panics unless the lengths match; the caller's
            // buffer is MTU-sized while `vec_recv` is the actual frame, so copy
            // only the received bytes and return that count (cf. e1000e::recv).
            let n = vec_recv.len().min(buf.len());
            buf[..n].copy_from_slice(&vec_recv[..n]);
            Ok(n)
        } else {
            Err(DeviceError::NotReady)
        }
    }

    fn send(&self, data: &[u8]) -> DeviceResult<usize> {
        // Hold the lock across can_send() and geth_send() so another CPU cannot
        // consume the slot in between (TOCTOU) and have geth_send() post into an
        // in-flight descriptor. Propagate the error instead of unwrap()-panicking
        // on an over-size frame.
        let mut driver = self.driver.0.lock();
        if !driver.can_send() {
            return Err(DeviceError::NotReady);
        }
        driver.geth_send(data).map_err(|_| DeviceError::IoError)?;
        Ok(data.len())
    }
    fn get_mtu(&self) -> usize {
        1500
    }
}

pub struct RTLxRxToken(Vec<u8>);
pub struct RTLxTxToken(RTLxDriver);

impl<'a> Device<'a> for RTLxDriver {
    type RxToken = RTLxRxToken;
    type TxToken = RTLxTxToken;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1514;
        caps.max_burst_size = Some(64);
        caps.medium = Medium::Ethernet;
        caps
    }

    fn receive(&mut self) -> Option<(Self::RxToken, Self::TxToken)> {
        if self.0.lock().can_recv() {
            //这里每次只接收一个网络包
            let (vec_recv, _rxcount) = self.0.lock().geth_recv(1);
            Some((RTLxRxToken(vec_recv), RTLxTxToken(self.clone())))
        } else {
            None
        }
    }

    fn transmit(&mut self) -> Option<Self::TxToken> {
        if self.0.lock().can_send() {
            Some(RTLxTxToken(self.clone()))
        } else {
            None
        }
    }
}

impl phy::RxToken for RTLxRxToken {
    fn consume<R, F>(mut self, _timestamp: Instant, f: F) -> Result<R>
    where
        F: FnOnce(&mut [u8]) -> Result<R>,
    {
        // Dispatch to global packet tapping (AF_PACKET sockets)
        super::net_defer_packet(&self.0);
        f(&mut self.0)
    }
}

impl phy::TxToken for RTLxTxToken {
    fn consume<R, F>(self, _timestamp: Instant, len: usize, f: F) -> Result<R>
    where
        F: FnOnce(&mut [u8]) -> Result<R>,
    {
        let mut buffer = [0u8; 1536];
        let result = f(&mut buffer[..len]);
        if result.is_ok() {
            // Re-check ownership under the SAME lock as the send: transmit()
            // gated on can_send() under a different lock acquisition, so on SMP
            // another CPU can consume the slot in between. Drop the frame instead
            // of unwrap()-panicking if the ring is full or the frame is over-size.
            let mut driver = (self.0).0.lock();
            if driver.can_send() {
                let _ = driver.geth_send(&buffer[..len]);
            }
        }
        result
    }
}

pub fn rtlx_init<F: Fn(usize, usize) -> Option<usize>>(
    irq: usize,
    mapper: F,
) -> DeviceResult<RTLxInterface> {
    mapper(rtl8211f::PINCTRL_GPIO_BASE as usize, PAGE_SIZE * 2);
    mapper(rtl8211f::SYS_CFG_BASE as usize, PAGE_SIZE * 2);

    let mut rtl8211f = RTL8211F::<ProviderImpl>::new(&[0u8; 6]);
    let mac = rtl8211f.get_umac();
    //启动前请为D1插上网线
    warn!("Please plug in the Ethernet cable");

    rtl8211f.open().unwrap();
    rtl8211f.set_rx_mode();
    rtl8211f.adjust_link().unwrap();

    let net_driver = RTLxDriver(Arc::new(Mutex::new(rtl8211f)));

    let ethernet_addr = EthernetAddress::from_bytes(&mac);

    let mut eui64 = [0u8; 8];
    eui64[0] = mac[0] ^ 2;
    eui64[1] = mac[1];
    eui64[2] = mac[2];
    eui64[3] = 0xff;
    eui64[4] = 0xfe;
    eui64[5] = mac[3];
    eui64[6] = mac[4];
    eui64[7] = mac[5];
    let link_local = Ipv6Address::new(
        0xfe80,
        0,
        0,
        0,
        (eui64[0] as u16) << 8 | eui64[1] as u16,
        (eui64[2] as u16) << 8 | eui64[3] as u16,
        (eui64[4] as u16) << 8 | eui64[5] as u16,
        (eui64[6] as u16) << 8 | eui64[7] as u16,
    );

    let ip_addrs = vec![
        IpCidr::new(IpAddress::v4(192, 168, 0, 123), 24),
        IpCidr::Ipv6(Ipv6Cidr::new(link_local, 64)),
        IpCidr::new(IpAddress::v4(0, 0, 0, 0), 0),
        IpCidr::new(IpAddress::v4(0, 0, 0, 0), 0),
    ];
    let default_gateway = Ipv4Address::new(192, 168, 0, 1);
    static mut ROUTES_STORAGE: [Option<(IpCidr, Route)>; 4] = [None; 4];
    let mut routes = unsafe { Routes::new(&mut ROUTES_STORAGE[..]) };
    routes.add_default_ipv4_route(default_gateway).unwrap();
    let neighbor_cache = NeighborCache::new(BTreeMap::new());
    let iface = InterfaceBuilder::new(net_driver.clone())
        .ethernet_addr(ethernet_addr)
        .neighbor_cache(neighbor_cache)
        .ip_addrs(ip_addrs)
        .routes(routes)
        .finalize();

    info!("rtl8211f interface up with addr 192.168.0.123/24");
    info!("rtl8211f interface up with route 192.168.0.1/24");
    let rtl8211f_iface = RTLxInterface {
        iface: Arc::new(Mutex::new(iface)),
        driver: net_driver,
        routes: Arc::new(Mutex::new(vec![RouteInfo {
            dst: IpCidr::new(IpAddress::v4(0, 0, 0, 0), 0),
            gateway: Some(IpAddress::Ipv4(default_gateway)),
        }])),
        name: String::from("rtl8211f"),
        irq,
    };

    Ok(rtl8211f_iface)
}

//TODO: Global SocketSet
// lazy_static::lazy_static! {
//     pub static ref SOCKETS: Mutex<SocketSet<'static>> =
//         Mutex::new(SocketSet::new(vec![]));
// }
