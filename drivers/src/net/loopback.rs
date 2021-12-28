// smoltcp
use smoltcp::{iface::Interface, phy::Loopback, time::Instant};

use crate::net::get_sockets;
use alloc::sync::Arc;

use alloc::string::String;
use spin::Mutex;

use crate::scheme::{NetScheme, Scheme};
use crate::{DeviceError, DeviceResult};

use alloc::vec::Vec;
use smoltcp::wire::EthernetAddress;
use smoltcp::wire::IpCidr;

#[derive(Clone)]
pub struct LoopbackInterface {
    pub iface: Arc<Mutex<Interface<'static, Loopback>>>,
    pub name: String,
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
    fn send(&self, _buf: &[u8]) -> DeviceResult<usize> {
        unimplemented!()
    }
    fn poll(&self) -> DeviceResult {
        let timestamp = Instant::from_millis(0);
        let sockets = get_sockets();
        let mut sockets = sockets.lock();
        match self.iface.lock().poll(&mut sockets, timestamp) {
            Ok(_) => Ok(()),
            Err(err) => {
                debug!("poll got err {}", err);
                Err(DeviceError::IoError)
            }
        }
    }

    fn get_mac(&self) -> EthernetAddress {
        unimplemented!()
    }
    fn get_ifname(&self) -> String {
        unimplemented!()
    }
    fn get_ip_addrrs(&self) -> Vec<IpCidr> {
        unimplemented!()
    }
}
