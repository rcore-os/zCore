// smoltcp
use alloc::collections::VecDeque;
use smoltcp::{
    iface::Interface,
    phy::{self, DeviceCapabilities, Medium},
    time::Instant,
    Result,
};

use crate::net::get_sockets;
use alloc::sync::Arc;

use alloc::string::String;
use lock::Mutex;

use crate::scheme::{NetScheme, NetStats, RouteInfo, Scheme};
use crate::{DeviceError, DeviceResult};

use alloc::vec::Vec;
use smoltcp::wire::EthernetAddress;
use smoltcp::wire::{IpAddress, IpCidr, Ipv4Cidr};

pub struct LoopbackDevice {
    queue: VecDeque<Vec<u8>>,
    medium: Medium,
    stats: Arc<Mutex<NetStats>>,
}

impl LoopbackDevice {
    pub fn new(medium: Medium, stats: Arc<Mutex<NetStats>>) -> Self {
        Self {
            queue: VecDeque::new(),
            medium,
            stats,
        }
    }
}

impl<'a> phy::Device<'a> for LoopbackDevice {
    type RxToken = LoopbackRxToken;
    type TxToken = LoopbackTxToken<'a>;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 65535;
        caps.medium = self.medium;
        caps
    }

    fn receive(&'a mut self) -> Option<(Self::RxToken, Self::TxToken)> {
        let stats = self.stats.clone();
        self.queue.pop_front().map(move |buffer| {
            let rx = LoopbackRxToken {
                buffer,
                stats: stats.clone(),
            };
            let tx = LoopbackTxToken {
                queue: &mut self.queue,
                stats,
            };
            (rx, tx)
        })
    }

    fn transmit(&'a mut self) -> Option<Self::TxToken> {
        Some(LoopbackTxToken {
            queue: &mut self.queue,
            stats: self.stats.clone(),
        })
    }
}

pub struct LoopbackRxToken {
    buffer: Vec<u8>,
    stats: Arc<Mutex<NetStats>>,
}

impl phy::RxToken for LoopbackRxToken {
    fn consume<R, F>(mut self, _timestamp: Instant, f: F) -> Result<R>
    where
        F: FnOnce(&mut [u8]) -> Result<R>,
    {
        let mut stats = self.stats.lock();
        stats.rx_packets += 1;
        stats.rx_bytes += self.buffer.len() as u64;
        drop(stats);

        f(&mut self.buffer)
    }
}

pub static mut LOOPBACK_TX_CALLBACK: Option<fn(&[u8])> = None;

pub fn register_loopback_tx_callback(cb: fn(&[u8])) {
    unsafe {
        LOOPBACK_TX_CALLBACK = Some(cb);
    }
}

/// Upper bound on frames buffered in the loopback queue. `poll()` uses
/// `try_lock` and silently skips on contention, so without a cap a sender that
/// outruns the drain would grow the queue without bound and exhaust the kernel
/// heap. When full we drop the oldest frame (backpressure); TCP retransmits.
const LOOPBACK_QUEUE_MAX: usize = 256;

pub struct LoopbackTxToken<'a> {
    queue: &'a mut VecDeque<Vec<u8>>,
    stats: Arc<Mutex<NetStats>>,
}

impl<'a> phy::TxToken for LoopbackTxToken<'a> {
    fn consume<R, F>(self, _timestamp: Instant, len: usize, f: F) -> Result<R>
    where
        F: FnOnce(&mut [u8]) -> Result<R>,
    {
        const MAX_TX_COPY: usize = 65536;
        let len = len.min(MAX_TX_COPY);
        let mut buffer = alloc::vec![0u8; len];
        let result = f(&mut buffer);

        let mut stats = self.stats.lock();
        stats.tx_packets += 1;
        stats.tx_bytes += len as u64;
        drop(stats);

        unsafe {
            if let Some(cb) = LOOPBACK_TX_CALLBACK {
                cb(&buffer);
            }
        }

        if self.queue.len() >= LOOPBACK_QUEUE_MAX {
            self.queue.pop_front();
        }
        self.queue.push_back(buffer);
        result
    }
}

#[derive(Clone)]
pub struct LoopbackInterface {
    pub iface: Arc<Mutex<Interface<'static, LoopbackDevice>>>,
    pub name: String,
    pub stats: Arc<Mutex<NetStats>>,
    pub routes: Arc<Mutex<Vec<RouteInfo>>>,
    pub ip_addrs: Arc<Mutex<Vec<IpCidr>>>,
}

impl Scheme for LoopbackInterface {
    fn name(&self) -> &str {
        "loopback"
    }

    fn handle_irq(&self, _cause: usize) {}
}

impl NetScheme for LoopbackInterface {
    fn recv(&self, _buf: &mut [u8]) -> DeviceResult<usize> {
        unimplemented!()
    }
    fn send(&self, buf: &[u8]) -> DeviceResult<usize> {
        let mut iface = self.iface.lock();
        let queue = &mut iface.device_mut().queue;
        if queue.len() >= LOOPBACK_QUEUE_MAX {
            queue.pop_front();
        }
        queue.push_back(buf.to_vec());
        Ok(buf.len())
    }
    fn poll(&self) -> DeviceResult {
        let timestamp = Instant::from_micros(crate::net::timer_now_as_micros() as i64);
        let Some(mut iface) = self.iface.try_lock() else {
            return Ok(());
        };
        let sockets = get_sockets();
        let Some(mut sockets) = sockets.try_lock() else {
            return Ok(());
        };
        match iface.poll(&mut sockets, timestamp) {
            Ok(_) => Ok(()),
            Err(err) => {
                debug!("poll got err {}", err);
                Err(DeviceError::IoError)
            }
        }
    }

    fn get_mac(&self) -> EthernetAddress {
        EthernetAddress::default()
    }

    fn get_ifname(&self) -> String {
        self.name.clone()
    }

    fn get_ip_address(&self) -> Vec<IpCidr> {
        self.ip_addrs.lock().clone()
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
        *self.ip_addrs.lock() = iface.ip_addrs().to_vec();
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
        *self.ip_addrs.lock() = iface.ip_addrs().to_vec();
        Ok(())
    }

    fn add_route(&self, cidr: IpCidr, gateway: Option<smoltcp::wire::IpAddress>) -> DeviceResult {
        self.routes.lock().push(RouteInfo { dst: cidr, gateway });
        Ok(())
    }

    fn del_route(&self, cidr: IpCidr, _gateway: Option<smoltcp::wire::IpAddress>) -> DeviceResult {
        self.routes.lock().retain(|r| r.dst != cidr);
        Ok(())
    }

    fn get_routes(&self) -> Vec<RouteInfo> {
        let iface = self.iface.lock();
        let mut res = Vec::new();

        // 1. Add tracked routes
        res.extend(self.routes.lock().clone());

        // 2. Add direct routes
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

    fn get_stats(&self) -> NetStats {
        self.stats.lock().clone()
    }
    fn get_mtu(&self) -> usize {
        65535
    }
}
