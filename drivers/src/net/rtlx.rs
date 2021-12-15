use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use smoltcp::iface::*;
use smoltcp::phy::{self, Device, DeviceCapabilities, Medium};
use smoltcp::socket::SocketSet;
use smoltcp::time::Instant;
use smoltcp::wire::*;
use smoltcp::Result;

use super::realtek::rtl8211f;
use super::realtek::rtl8211f::RTL8211F;
use super::ProviderImpl;
use super::PAGE_SIZE;
//use kernel_hal::drivers::{Driver, DeviceType, NetDriver, DRIVERS, NET_DRIVERS, SOCKETS};

use crate::scheme::{NetScheme, Scheme};
use crate::{DeviceError, DeviceResult};

#[derive(Clone)]
pub struct RTLxDriver(Arc<Mutex<RTL8211F<ProviderImpl>>>);

#[derive(Clone)]
pub struct RTLxInterface {
    pub iface: Arc<Mutex<Interface<'static, RTLxDriver>>>,
    pub driver: RTLxDriver,
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
            let timestamp = Instant::from_millis(0);
            let mut sockets = SOCKETS.lock(); //引发死锁？

            self.driver.0.lock().int_disable();
            match self.iface.lock().poll(&mut sockets, timestamp) {
                Ok(_) => {
                    //SOCKET_ACTIVITY.notify_all();
                    debug!("try_handle_interrupt SOCKET_ACTIVITY unimplemented");
                }
                Err(err) => {
                    error!("poll got err {}", err);
                }
            }
            self.driver.0.lock().int_enable();
            //return true;
            return;
        }

        return;
    }
}

impl NetScheme for RTLxInterface {
    fn get_mac(&self) -> EthernetAddress {
        self.iface.lock().ethernet_addr()
    }

    fn get_ifname(&self) -> String {
        self.name.clone()
    }

    fn get_ip_addrrs(&self) -> Vec<IpCidr> {
        Vec::from(self.iface.lock().ip_addrs())
    }

    fn poll(&self) -> DeviceResult {
        let timestamp = Instant::from_millis(0);
        let mut sockets = SOCKETS.lock();
        match self.iface.lock().poll(&mut sockets, timestamp) {
            Ok(_) => {
                //SOCKET_ACTIVITY.notify_all();
                error!("poll, SOCKET_ACTIVITY unimplemented");
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
            let (vec_recv, rxcount) = self.driver.0.lock().geth_recv(1);
            buf.copy_from_slice(&vec_recv);
            Ok(rxcount as usize)
        } else {
            Err(DeviceError::NotReady)
        }
    }

    fn send(&self, data: &[u8]) -> DeviceResult<usize> {
        if self.driver.0.lock().can_send() {
            self.driver.0.lock().geth_send(&data);
            Ok(data.len())
        } else {
            Err(DeviceError::NotReady)
        }
    }
}

pub struct RTLxRxToken(Vec<u8>);
pub struct RTLxTxToken(RTLxDriver);

impl<'a> Device<'a> for RTLxDriver {
    type RxToken = RTLxRxToken;
    type TxToken = RTLxTxToken;

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
            (self.0).0.lock().geth_send(&buffer[..len]);
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

    rtl8211f.open();
    rtl8211f.set_rx_mode();
    rtl8211f.adjust_link();

    let net_driver = RTLxDriver(Arc::new(Mutex::new(rtl8211f)));

    let ethernet_addr = EthernetAddress::from_bytes(&mac);
    let ip_addrs = [IpCidr::new(IpAddress::v4(192, 168, 0, 123), 24)];
    let neighbor_cache = NeighborCache::new(BTreeMap::new());
    let iface = InterfaceBuilder::new(net_driver.clone())
        .ethernet_addr(ethernet_addr)
        .neighbor_cache(neighbor_cache)
        .ip_addrs(ip_addrs)
        .finalize();

    info!("rtl8211f interface up with addr 192.168.0.123/24");
    let rtl8211f_iface = RTLxInterface {
        iface: Arc::new(Mutex::new(iface)),
        driver: net_driver.clone(),
        name: String::from("rtl8211f"),
        irq,
    };

    Ok(rtl8211f_iface)
}

//TODO: Global SocketSet
lazy_static::lazy_static! {
    pub static ref SOCKETS: Mutex<SocketSet<'static>> =
        Mutex::new(SocketSet::new(vec![]));
}
