use {
    crate::object::*,
    alloc::sync::{Arc, Weak},
    alloc::{collections::VecDeque, vec::Vec},
    spin::Mutex,
};

/// Bidirectional streaming IPC transport.
///
/// # SYNOPSIS
///
/// Sockets are a bidirectional stream transport.
/// Unlike channels, sockets only move data (not handles).
pub struct Socket {
    base: KObjectBase,
    peer: Weak<Socket>,
    inner: Mutex<SocketInner>,
}

#[derive(Default)]
struct SocketInner {
    read_disabled: bool,
    read_threshold: usize,
    write_threshold: usize, // only for core-test
    data: VecDeque<u8>,
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
    /// Create a socket.
    #[allow(unsafe_code)]
    pub fn create() -> (Arc<Self>, Arc<Self>) {
        let mut end0 = Arc::new(Socket {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            peer: Weak::default(),
            inner: Default::default(),
        });
        let end1 = Arc::new(Socket {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            peer: Arc::downgrade(&end0),
            inner: Default::default(),
        });
        // no other reference of `end0`
        unsafe {
            Arc::get_mut_unchecked(&mut end0).peer = Arc::downgrade(&end1);
        }
        (end0, end1)
    }

    /// Read data from the socket.
    pub fn read(&self, size: usize, peek: bool) -> ZxResult<Vec<u8>> {
        let mut inner = self.inner.lock();
        if inner.data.is_empty() {
            let _peer = self.peer.upgrade().ok_or(ZxError::PEER_CLOSED)?;
            if inner.read_disabled {
                return Err(ZxError::BAD_STATE);
            }
            return Err(ZxError::SHOULD_WAIT);
        }
        let size = size.min(inner.data.len());
        let data = if peek {
            let (slice0, slice1) = inner.data.as_slices();
            if size <= slice0.len() {
                Vec::from(&slice0[..size])
            } else {
                let mut v = Vec::from(slice0);
                v.extend(&slice1[..size - slice0.len()]);
                v
            }
        } else {
            inner.data.drain(..size).collect()
        };
        let mut clear = Signal::empty();
        if inner.read_threshold > 0 && inner.data.len() < inner.read_threshold {
            clear |= Signal::SOCKET_READ_THRESHOLD;
        }
        if inner.data.is_empty() {
            clear |= Signal::READABLE;
        }
        self.base.signal_clear(clear);
        Ok(data)
    }

    /// Write data to the socket.
    pub fn write(&self, buffer: &[u8]) -> ZxResult<usize> {
        let peer = self.peer.upgrade().ok_or(ZxError::PEER_CLOSED)?;
        if self.signal().contains(Signal::SOCKET_WRITE_DISABLED) {
            return Err(ZxError::BAD_STATE);
        }
        if buffer.is_empty() {
            return Ok(0);
        }
        let buffer_len = buffer.len();
        let mut peer_inner = peer.inner.lock();
        let mut set = Signal::empty();
        if peer_inner.data.is_empty() {
            set |= Signal::READABLE;
        }
        peer_inner.data.extend(buffer);
        if peer_inner.read_threshold > 0 && peer_inner.data.len() >= peer_inner.read_threshold {
            set |= Signal::SOCKET_READ_THRESHOLD;
        }
        peer.base.signal_set(set);
        Ok(buffer_len)
    }

    /// Get information of the socket.
    pub fn get_info(&self) -> SocketInfo {
        let self_size = self.inner.lock().data.len();
        let peer_size = match self.peer.upgrade() {
            Some(peer) => peer.inner.lock().data.len(),
            None => 0,
        };
        SocketInfo {
            options: 0,
            padding1: 0,
            rx_buf_max: u64::MAX,
            rx_buf_size: self_size as _,
            rx_buf_available: self_size as _,
            tx_buf_max: u64::MAX,
            tx_buf_size: peer_size as _,
        }
    }

    /// Prevent reading or writing.
    pub fn shutdown(&self, read: bool, write: bool) -> ZxResult {
        self.shutdown_self(read, write)?;
        if let Some(peer) = self.peer.upgrade() {
            peer.shutdown_self(!read, !write)?;
        }
        Ok(())
    }

    fn shutdown_self(&self, read: bool, write: bool) -> ZxResult {
        let mut set = Signal::empty();
        let mut clear = Signal::empty();
        let mut inner = self.inner.lock();
        if read {
            inner.read_disabled = true;
            set |= Signal::SOCKET_PEER_WRITE_DISABLED;
        }
        if write {
            clear |= Signal::WRITABLE;
            set |= Signal::SOCKET_WRITE_DISABLED;
        }
        self.base.signal_change(clear, set);
        Ok(())
    }

    pub fn set_read_threshold(&self, threshold: usize) -> ZxResult {
        let mut inner = self.inner.lock();
        inner.read_threshold = threshold;
        if threshold == 0 {
            self.base.signal_clear(Signal::SOCKET_READ_THRESHOLD);
        } else if inner.data.len() >= threshold {
            self.base.signal_set(Signal::SOCKET_READ_THRESHOLD);
        } else {
            self.base.signal_clear(Signal::SOCKET_READ_THRESHOLD);
        }
        Ok(())
    }

    pub fn set_write_threshold(&self, threshold: usize) -> ZxResult {
        let peer = self.peer.upgrade().ok_or(ZxError::PEER_CLOSED)?;
        peer.inner.lock().write_threshold = threshold;
        Ok(())
    }

    pub fn get_rx_tx_threshold(&self) -> (usize, usize) {
        let inner = self.inner.lock();
        (inner.read_threshold, inner.write_threshold)
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
