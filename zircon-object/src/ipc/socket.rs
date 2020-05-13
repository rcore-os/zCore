use {
    crate::object::*,
    alloc::sync::{Arc, Weak},
    alloc::vec::Vec,
    core::cmp::min,
    core::iter::FromIterator,
    spin::Mutex,
    bitflags::bitflags,
};

pub struct Socket {
    base: KObjectBase,
    peer: Weak<Socket>,
    lock: Arc<Mutex<u8>>,
    inner: Mutex<SocketInner>,
}

#[derive(Default)]
struct SocketInner {
    read_disabled: bool,
    read_threshold: usize,
    write_threshold: usize, // only for core-test
    data: Vec<u8>,
}

impl_kobject!(Socket
    fn peer(&self) -> ZxResult<Arc<dyn KernelObject>> {
        let peer = self.peer.upgrade().ok_or(ZxError::PEER_CLOSED)?;
        Ok(peer)
    }
    fn related_koid(&self) -> KoID {
        self.peer.upgrade().map(|p| p.id()).unwrap_or(0)
    }
);

// Only support stream mode
// The size of data is unlimited
impl Socket {
    #[allow(unsafe_code)]
    pub fn create() -> (Arc<Self>, Arc<Self>) {
        let lock = Arc::new(Mutex::new(0 as u8));
        let mut end0 = Arc::new(Socket {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            peer: Weak::default(),
            lock: lock.clone(),
            inner: Default::default(),
        });
        let end1 = Arc::new(Socket {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            peer: Arc::downgrade(&end0),
            lock: lock,
            inner: Default::default(),
        });
        // no other reference of `end0`
        unsafe {
            Arc::get_mut_unchecked(&mut end0).peer = Arc::downgrade(&end1);
        }
        (end0, end1)
    }

    pub fn read(&self, size: usize, options: SocketOptions) -> ZxResult<Vec<u8>> {
        let _ = self.lock.lock();
        let mut inner = self.inner.lock();
        if inner.data.is_empty() {
            let peer = self.peer.upgrade();
            if peer.is_none() {
                return Err(ZxError::PEER_CLOSED);
            }
            if inner.read_disabled {
                return Err(ZxError::BAD_STATE);
            }
            return Err(ZxError::SHOULD_WAIT);
        }
        let size = min(size, inner.data.len());
        let data = if options.contains(SocketOptions::PEEK) {
            Vec::from_iter(inner.data[0..size].iter().cloned())
        } else {
            inner.data.drain(..size).collect::<Vec<_>>()
        };
        let mut clear = Signal::empty();
        if inner.read_threshold > 0 && inner.data.len() < inner.read_threshold {
            clear |= Signal::SOCKET_READ_THRESHOLD;
        }
        error!("inner size {} {} {:?}", inner.data.len(), inner.data.is_empty(), self.signal());
        if inner.data.is_empty() {
            clear |= Signal::READABLE;
        }
        self.signal_change(clear, Signal::empty());
        Ok(data)
        
    }

    pub fn write(&self, mut buffer: Vec<u8>) -> ZxResult<usize> {
        let _ = self.lock.lock();
        let peer = self.peer.upgrade();
        if peer.is_none() {
            return Err(ZxError::PEER_CLOSED);
        }
        let peer = peer.unwrap();
        if self.signal().contains(Signal::SOCKET_WRITE_DISABLED) {
            return Err(ZxError::BAD_STATE);
        }
        if buffer.len() == 0 {
            return Ok(0);
        }
        let buffer_len = buffer.len();
        let mut peer_inner = peer.inner.lock();
        let mut set = Signal::empty();
        if peer_inner.data.is_empty() {
            set |= Signal::READABLE;
        }
        peer_inner.data.append(&mut buffer);
        if peer_inner.read_threshold > 0 && peer_inner.data.len() >= peer_inner.read_threshold {
            set |= Signal::SOCKET_READ_THRESHOLD;
        }
        peer.signal_change(Signal::empty(), set);
        Ok(buffer_len)
    }

    pub fn get_info(&self) -> SocketInfo {
        let _ = self.lock.lock();
        let self_size = self.inner.lock().data.len();
        let peer_size = match self.peer.upgrade() {
            Some(peer) => peer.inner.lock().data.len(),
            None => 0,
        };
        return SocketInfo {
            options: 0,
            padding1: 0,
            rx_buf_max: u64::MAX,
            rx_buf_size: self_size as _,
            rx_buf_available: self_size as _,
            tx_buf_max: u64::MAX,
            tx_buf_size: peer_size as _,
        }
    }
    
    pub fn shutdown(&self, options: SocketOptions) -> ZxResult {
        let _ = self.lock.lock();
        self.shutdown_self(options)?;
        if let Some(peer) = self.peer.upgrade() {
            peer.shutdown_self(options ^ SocketOptions::SHUTDOWN_READ ^ SocketOptions::SHUTDOWN_WRITE)?;
        }
        Ok(())
    }

    pub fn shutdown_self(&self, options: SocketOptions) -> ZxResult {
        let mut set = Signal::empty();
        let mut clear = Signal::empty();
        let mut inner = self.inner.lock();
        if options.contains(SocketOptions::SHUTDOWN_READ) {
            inner.read_disabled = true;
            set |= Signal::SOCKET_PEER_WRITE_DISABLED;
        }
        if options.contains(SocketOptions::SHUTDOWN_WRITE) {
            clear |= Signal::WRITABLE;
            set |= Signal::SOCKET_WRITE_DISABLED;
        }
        self.signal_change(clear, set);
        Ok(())
    }

    pub fn set_read_threshold(&self, threshold: usize) -> ZxResult {
        let _ = self.lock.lock();
        let mut inner = self.inner.lock();
        inner.read_threshold = threshold;
        if threshold == 0 {
            self.signal_change(Signal::SOCKET_READ_THRESHOLD, Signal::empty());
        } else {
            if inner.data.len() >= threshold {
                self.signal_change(Signal::empty(), Signal::SOCKET_READ_THRESHOLD);
            } else {
                self.signal_change(Signal::SOCKET_READ_THRESHOLD, Signal::empty());
            }
        }
        Ok(())
    }

    pub fn set_write_threshold(&self, threshold: usize) -> ZxResult {
        let _ = self.lock.lock();
        let peer = self.peer.upgrade();
        if peer.is_none() {
            return Err(ZxError::PEER_CLOSED);
        }
        peer.unwrap().inner.lock().write_threshold = threshold;
        Ok(())
    }

    pub fn get_rx_tx_threshold(&self) -> (usize, usize) {
        let inner = self.inner.lock();
        return (inner.read_threshold, inner.write_threshold)
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        if let Some(peer) = self.peer.upgrade() {
            peer.signal_change(Signal::WRITABLE, Signal::PEER_CLOSED);
        }
    }
}

#[repr(C)]
#[derive(Default)]
pub struct SocketInfo {
    options: u32,
    padding1: u32,
    rx_buf_max: u64,
    rx_buf_size: u64,
    rx_buf_available: u64,
    tx_buf_max: u64,
    tx_buf_size: u64,
}

bitflags! {
    #[derive(Default)]
    pub struct SocketOptions: u32 {
        #[allow(clippy::identity_op)]
        const SHUTDOWN_WRITE = 1 << 0;
        const SHUTDOWN_READ = 1 << 1;
        const DATAGRAM = 1 << 0;
        const PEEK = 1 << 3;
    }
}
