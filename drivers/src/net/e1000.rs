//! Intel PRO/1000 Network Adapter i.e. e1000 network driver
//! Datasheet: https://www.intel.ca/content/dam/doc/datasheet/82574l-gbe-controller-datasheet.pdf

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use smoltcp::iface::*;
use smoltcp::phy::{self, DeviceCapabilities};
use smoltcp::time::Instant;
use smoltcp::wire::*;
use smoltcp::Result;

use super::{timer_now_as_micros, ProviderImpl};
use crate::net::get_sockets;
use crate::scheme::{NetScheme, Scheme};
use crate::{DeviceError, DeviceResult};
use isomorphic_drivers::net::ethernet::intel::e1000::E1000;
use isomorphic_drivers::net::ethernet::structs::EthernetAddress as DriverEthernetAddress;
use lock::Mutex;

#[derive(Clone)]
pub struct E1000Driver(Arc<Mutex<E1000<ProviderImpl>>>);

#[derive(Clone)]
pub struct E1000Interface {
    iface: Arc<Mutex<Interface<'static, E1000Driver>>>,
    driver: E1000Driver,
    name: String,
    irq: usize,
}

impl Scheme for E1000Interface {
    fn name(&self) -> &str {
        "e1000"
    }

    fn handle_irq(&self, irq: usize) {
        if irq != self.irq {
            // not ours, skip it
            return;
        }

        let data = self.driver.0.lock().handle_interrupt();

        if data {
            let timestamp = Instant::from_micros(timer_now_as_micros() as i64);
            let sockets = get_sockets();
            let mut sockets = sockets.lock();
            match self.iface.lock().poll(&mut sockets, timestamp) {
                Ok(p) => {
                    //SOCKET_ACTIVITY.notify_all();
                    info!("e1000 try_handle_interrupt poll: {:?}", p);
                }
                Err(err) => {
                    warn!("poll got err {}", err);
                }
            }
        }
    }
}

impl NetScheme for E1000Interface {
    fn get_mac(&self) -> EthernetAddress {
        self.iface.lock().ethernet_addr()
    }

    fn get_ifname(&self) -> String {
        self.name.clone()
    }

    // get ip addresses
    fn get_ip_address(&self) -> Vec<IpCidr> {
        Vec::from(self.iface.lock().ip_addrs())
    }

    fn poll(&self) -> DeviceResult {
        let timestamp = Instant::from_micros(timer_now_as_micros() as i64);
        let sockets = get_sockets();
        let mut sockets = sockets.lock();
        match self.iface.lock().poll(&mut sockets, timestamp) {
            Ok(p) => {
                //SOCKET_ACTIVITY.notify_all();
                info!("e1000 NetScheme poll: {:?}", p);
                Ok(())
            }
            Err(err) => {
                warn!("poll got err {}", err);
                Err(DeviceError::IoError)
            }
        }
    }

    fn recv(&self, buf: &mut [u8]) -> DeviceResult<usize> {
        if let Some(vec_recv) = self.driver.0.lock().receive() {
            buf.copy_from_slice(&vec_recv);
            Ok(vec_recv.len())
        } else {
            Err(DeviceError::NotReady)
        }
    }

    fn send(&self, data: &[u8]) -> DeviceResult<usize> {
        if self.driver.0.lock().can_send() {
            let mut driver = self.driver.0.lock();
            driver.send(data);
            Ok(data.len())
        } else {
            Err(DeviceError::NotReady)
        }
    }
}

pub struct E1000RxToken(Vec<u8>);
pub struct E1000TxToken(E1000Driver);

impl phy::Device<'_> for E1000Driver {
    type RxToken = E1000RxToken;
    type TxToken = E1000TxToken;

    fn receive(&mut self) -> Option<(Self::RxToken, Self::TxToken)> {
        self.0
            .lock()
            .receive()
            .map(|vec_recv| (E1000RxToken(vec_recv), E1000TxToken(self.clone())))
    }

    fn transmit(&mut self) -> Option<Self::TxToken> {
        if self.0.lock().can_send() {
            Some(E1000TxToken(self.clone()))
        } else {
            None
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1536;
        caps.max_burst_size = Some(64);
        caps
    }
}

impl phy::RxToken for E1000RxToken {
    fn consume<R, F>(mut self, _timestamp: Instant, f: F) -> Result<R>
    where
        F: FnOnce(&mut [u8]) -> Result<R>,
    {
        f(&mut self.0)
    }
}

impl phy::TxToken for E1000TxToken {
    fn consume<R, F>(self, _timestamp: Instant, len: usize, f: F) -> Result<R>
    where
        F: FnOnce(&mut [u8]) -> Result<R>,
    {
        let mut buffer = [0u8; 1536];
        let result = f(&mut buffer[..len]);

        let mut driver = (self.0).0.lock();
        driver.send(&buffer[..len]);

        result
    }
}

// JudgeDuck-OS/kern/e1000.c
pub fn init(
    name: String,
    irq: usize,
    header: usize,
    size: usize,
    index: usize,
) -> DeviceResult<E1000Interface> {
    info!("Probing e1000 {}", name);

    // randomly generated
    let mac: [u8; 6] = [0x54, 0x51, 0x9F, 0x71, 0xC0, index as u8];

    let e1000 = E1000::new(header, size, DriverEthernetAddress::from_bytes(&mac));

    let net_driver = E1000Driver(Arc::new(Mutex::new(e1000)));

    let ethernet_addr = EthernetAddress::from_bytes(&mac);
    let ip_addrs = [IpCidr::new(IpAddress::v4(10, 0, 2, (15 + index) as u8), 24)];
    let default_v4_gw = Ipv4Address::new(10, 0, 2, 2); //Qemu user network gateway: 10.0.2.2
    static mut ROUTES_STORAGE: [Option<(IpCidr, Route)>; 1] = [None; 1];
    let mut routes = unsafe { Routes::new(&mut ROUTES_STORAGE[..]) };
    routes.add_default_ipv4_route(default_v4_gw).unwrap();

    let neighbor_cache = NeighborCache::new(BTreeMap::new());
    let iface = InterfaceBuilder::new(net_driver.clone())
        .ethernet_addr(ethernet_addr)
        .neighbor_cache(neighbor_cache)
        .ip_addrs(ip_addrs)
        .routes(routes)
        .finalize();

    info!(
        "e1000 interface {} up with addr 10.0.2.{}/24",
        name,
        15 + index
    );
    let e1000_iface = E1000Interface {
        iface: Arc::new(Mutex::new(iface)),
        driver: net_driver,
        name,
        irq,
    };

    Ok(e1000_iface)
}
