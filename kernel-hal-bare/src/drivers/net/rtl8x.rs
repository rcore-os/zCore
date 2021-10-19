use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use smoltcp::iface::*;
use smoltcp::phy::{self, Device, DeviceCapabilities, Medium};
use smoltcp::time::Instant;
use smoltcp::wire::*;
use smoltcp::Result;

use super::super::IRQ_MANAGER;
use super::realtek::rtl8211f::RTL8211F;
use crate::drivers::provider::Provider;
use crate::PAGE_SIZE;
use kernel_hal::drivers::{DeviceType, Driver, NetDriver, DRIVERS, NET_DRIVERS, SOCKETS};

#[derive(Clone)]
pub struct RTL8xDriver(Arc<Mutex<RTL8211F<Provider>>>);

#[derive(Clone)]
pub struct RTL8xInterface {
    pub iface: Arc<Mutex<Interface<'static, RTL8xDriver>>>,
    pub driver: RTL8xDriver,
    pub name: String,
    pub irq: Option<usize>,
}

impl Driver for RTL8xInterface {
    fn try_handle_interrupt(&self, irq: Option<usize>) -> bool {
        if irq.is_some() && self.irq.is_some() && irq != self.irq {
            // not ours, skip it
            return false;
        }

        let status = self.driver.0.lock().interrupt_status();

        let handle_tx_rx = 3;
        if status == handle_tx_rx {
            let timestamp = Instant::from_millis(0);
            let mut sockets = SOCKETS.lock(); //引发死锁？

            self.driver.0.lock().int_disable();
            match self.iface.lock().poll(&mut sockets, timestamp) {
                Ok(_) => {
                    //SOCKET_ACTIVITY.notify_all();
                    error!("e1000 try_handle_interrupt SOCKET_ACTIVITY unimplemented !");
                }
                Err(err) => {
                    debug!("poll got err {}", err);
                }
            }
            self.driver.0.lock().int_enable();
            return true;
        }

        return false;
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Net
    }

    fn get_id(&self) -> String {
        String::from("e1000")
    }

    fn as_net(&self) -> Option<&dyn NetDriver> {
        Some(self)
    }
}

impl NetDriver for RTL8xInterface {
    fn get_mac(&self) -> EthernetAddress {
        self.iface.lock().ethernet_addr()
    }

    fn get_ifname(&self) -> String {
        self.name.clone()
    }

    // get ip addresses
    fn get_ip_addresses(&self) -> Vec<IpCidr> {
        Vec::from(self.iface.lock().ip_addrs())
    }

    fn ipv4_address(&self) -> Option<Ipv4Address> {
        self.iface.lock().ipv4_address()
    }

    fn poll(&self) {
        let timestamp = Instant::from_millis(0);
        let mut sockets = SOCKETS.lock();
        match self.iface.lock().poll(&mut sockets, timestamp) {
            Ok(_) => {
                //SOCKET_ACTIVITY.notify_all();
                error!("e1000 poll SOCKET_ACTIVITY unimplemented !");
            }
            Err(err) => {
                debug!("poll got err {}", err);
            }
        }
    }

    fn send(&self, data: &[u8]) -> Option<usize> {
        self.driver.0.lock().geth_send(&data);
        Some(data.len())
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

pub struct RTL8xRxToken(Vec<u8>);
pub struct RTL8xTxToken(RTL8xDriver);

impl<'a> Device<'a> for RTL8xDriver {
    type RxToken = RTL8xRxToken;
    type TxToken = RTL8xTxToken;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1536;
        caps.max_burst_size = Some(64);
        caps.medium = Medium::Ethernet;
        caps
    }

    fn receive(&mut self) -> Option<(Self::RxToken, Self::TxToken)> {
        if self.0.lock().can_recv() {
            //这里每次只接收一个网络包
            let (vec_recv, rxcount) = self.0.lock().geth_recv(1);
            Some((RTL8xRxToken(vec_recv), RTL8xTxToken(self.clone())))
        } else {
            None
        }
    }

    fn transmit(&mut self) -> Option<Self::TxToken> {
        if self.0.lock().can_send() {
            Some(RTL8xTxToken(self.clone()))
        } else {
            None
        }
    }
}

impl phy::RxToken for RTL8xRxToken {
    fn consume<R, F>(mut self, _timestamp: Instant, f: F) -> Result<R>
    where
        F: FnOnce(&mut [u8]) -> Result<R>,
    {
        f(&mut self.0)
    }
}

impl phy::TxToken for RTL8xTxToken {
    fn consume<R, F>(self, _timestamp: Instant, len: usize, f: F) -> Result<R>
    where
        F: FnOnce(&mut [u8]) -> Result<R>,
    {
        let mut buffer = [0u8; 1536];
        let result = f(&mut buffer[..len]);
        if result.is_ok() {
            (self.0).0.lock().geth_send(&buffer[..len]);
        }
        result
    }
}

pub fn init(name: String, irq: Option<usize>) {
    let mut rtl8211f = RTL8211F::<Provider>::new(&[0u8; 6]);
    let mac = rtl8211f.get_umac();
    //启动前请为D1插上网线
    rtl8211f.open();
    rtl8211f.set_rx_mode();
    rtl8211f.adjust_link();

    let net_driver = RTL8xDriver(Arc::new(Mutex::new(rtl8211f)));

    let ethernet_addr = EthernetAddress::from_bytes(&mac);
    let ip_addrs = [IpCidr::new(IpAddress::v4(192, 168, 0, 123), 24)];
    let neighbor_cache = NeighborCache::new(BTreeMap::new());
    let iface = InterfaceBuilder::new(net_driver.clone())
        .ethernet_addr(ethernet_addr)
        .neighbor_cache(neighbor_cache)
        .ip_addrs(ip_addrs)
        .finalize();

    info!("rtl8211f interface {} up with addr 192.168.0.123/24", name);
    let rtl8211f_iface = RTL8xInterface {
        iface: Arc::new(Mutex::new(iface)),
        driver: net_driver.clone(),
        name,
        irq,
    };

    let driver = Arc::new(rtl8211f_iface);
    DRIVERS.write().push(driver.clone());
    IRQ_MANAGER.write().register_opt(irq, driver.clone());
    NET_DRIVERS.write().push(driver);
}
