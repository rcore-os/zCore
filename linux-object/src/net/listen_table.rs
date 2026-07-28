use crate::error::{LxError, LxResult};
use alloc::{boxed::Box, vec::Vec};
use lock::Mutex;
use smoltcp::wire::IpEndpoint;

const PORT_NUM: usize = 65536;

pub struct ListenTableEntry {
    pub listen_endpoint: IpEndpoint,
}

pub struct ListenTable {
    tcp: Box<[Mutex<Option<Box<ListenTableEntry>>>]>,
}

impl Default for ListenTable {
    fn default() -> Self {
        Self::new()
    }
}

impl ListenTable {
    pub fn new() -> Self {
        let mut vec = Vec::with_capacity(PORT_NUM);
        for _ in 0..PORT_NUM {
            vec.push(Mutex::new(None));
        }
        Self {
            tcp: vec.into_boxed_slice(),
        }
    }

    pub fn can_listen(&self, port: u16) -> bool {
        self.tcp[port as usize].lock().is_none()
    }

    pub fn listen(&self, listen_endpoint: IpEndpoint) -> LxResult<()> {
        let port = listen_endpoint.port;
        if port == 0 {
            return Err(LxError::EINVAL);
        }
        let mut entry = self.tcp[port as usize].lock();
        if entry.is_none() {
            *entry = Some(Box::new(ListenTableEntry { listen_endpoint }));
            Ok(())
        } else {
            Err(LxError::EADDRINUSE)
        }
    }

    pub fn unlisten(&self, port: u16) {
        *self.tcp[port as usize].lock() = None;
    }
}

lazy_static::lazy_static! {
    pub static ref LISTEN_TABLE: ListenTable = ListenTable::new();
}
