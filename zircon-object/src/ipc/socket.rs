use {
    crate::object::*,
    alloc::collections::VecDeque,
    alloc::sync::{Arc, Weak},
    alloc::vec::Vec,
    bitflags::bitflags,
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
    flags: SocketFlags, // constant value
    inner: Mutex<SocketInner>,
}

#[derive(Default)]
struct SocketInner {
    control_msg: Vec<u8>,
    data: VecDeque<u8>,
    datagram_len: VecDeque<usize>,
    read_threshold: usize,
    write_threshold: usize,
    read_disabled: bool,
}

const SOCKET_SIZE: usize = 128 * 2048;

impl_kobject!(Socket
    fn peer(&self) -> ZxResult<Arc<dyn KernelObject>> {
        let peer = self.peer.upgrade().ok_or(ZxError::PEER_CLOSED)?;
        Ok(peer)
    }
    fn related_koid(&self) -> KoID {
        self.peer.upgrade().map(|p| p.id()).unwrap_or(0)
    }
);

bitflags! {
    /// Signals that waitable kernel objects expose to applications.
    #[derive(Default)]
    pub struct SocketFlags: u32 {
        #[allow(clippy::identity_op)]
        // These options can be passed to socket_shutdown().
        const SHUTDOWN_WRITE                = 1;
        const SHUTDOWN_READ                 = 1 << 1;
        const SHUTDOWN_MASK                 = Self::SHUTDOWN_WRITE.bits | Self::SHUTDOWN_READ.bits;

        // These can be passed to socket_create().
        // const STREAM                     = 0; // Don't use contains
        const DATAGRAM                      = 1;
        const HAS_CONTROL                   = 1 << 1;
        const HAS_ACCEPT                    = 1 << 2;
        const CREATE_MASK                   = Self::DATAGRAM.bits | Self::HAS_CONTROL.bits | Self::HAS_ACCEPT.bits;

        // These can be passed to socket_read() and socket_write().
        const SOCKET_CONTROL                = 1 << 2;

        // These can be passed to socket_read().
        const SOCKET_PEEK                   = 1 << 3;
    }
}

impl Socket {
    /// Create a socket.
    #[allow(unsafe_code)]
    pub fn create(flags: u32) -> ZxResult<(Arc<Self>, Arc<Self>)> {
        let flags = SocketFlags::from_bits(flags).ok_or(ZxError::INVALID_ARGS)?;
        if !(flags - SocketFlags::CREATE_MASK).is_empty() {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut starting_signals: Signal = Signal::WRITABLE;
        if flags.contains(SocketFlags::HAS_ACCEPT) {
            starting_signals |= Signal::SOCKET_SHARE;
        }
        if flags.contains(SocketFlags::HAS_CONTROL) {
            starting_signals |= Signal::SOCKET_CONTROL_WRITABLE;
        }
        let mut end0 = Arc::new(Socket {
            base: KObjectBase::with_signal(starting_signals),
            peer: Weak::default(),
            flags,
            inner: Default::default(),
        });
        let end1 = Arc::new(Socket {
            base: KObjectBase::with_signal(starting_signals),
            peer: Arc::downgrade(&end0),
            flags,
            inner: Default::default(),
        });
        // no other reference of `end0`
        unsafe {
            Arc::get_mut_unchecked(&mut end0).peer = Arc::downgrade(&end1);
        }
        Ok((end0, end1))
    }

    /// Write data to the socket.
    pub fn write(&self, options: SocketFlags, data: &[u8]) -> ZxResult<usize> {
        if self.base.signal().contains(Signal::SOCKET_WRITE_DISABLED) {
            return Err(ZxError::BAD_STATE);
        }
        let peer = self.peer.upgrade().ok_or(ZxError::PEER_CLOSED)?;
        if options.contains(SocketFlags::SOCKET_CONTROL) {
            if !self.flags.contains(SocketFlags::HAS_CONTROL) {
                return Err(ZxError::BAD_STATE);
            }
            if data.is_empty() {
                return Err(ZxError::INVALID_ARGS);
            }
            if data.len() > 1024 {
                return Err(ZxError::OUT_OF_RANGE);
            }
            let actual_count = peer.write_control(data)?;
            self.base.signal_clear(Signal::SOCKET_CONTROL_WRITABLE);
            Ok(actual_count)
        } else {
            if self.base.signal().contains(Signal::SOCKET_WRITE_DISABLED) {
                return Err(ZxError::BAD_STATE);
            }
            let actual_count = peer.write_data(data)?;
            if actual_count > 0 {
                let mut clear = Signal::empty();
                let peer_inner = peer.inner.lock();
                let inner = self.inner.lock();
                let peer_rest_size = SOCKET_SIZE - peer_inner.data.len();
                if peer_rest_size == 0 {
                    clear |= Signal::WRITABLE;
                }
                if inner.write_threshold > 0 && peer_rest_size < inner.write_threshold {
                    clear |= Signal::SOCKET_WRITE_THRESHOLD;
                }
                self.base.signal_clear(clear);
            }
            Ok(actual_count)
        }
    }

    fn write_control(&self, data: &[u8]) -> ZxResult<usize> {
        let mut inner = self.inner.lock();
        if !inner.control_msg.is_empty() {
            return Err(ZxError::SHOULD_WAIT);
        }
        let actual_count = data.len();
        inner.control_msg.extend_from_slice(data);
        self.base.signal_set(Signal::SOCKET_CONTROL_READABLE);
        Ok(actual_count)
    }

    fn write_data(&self, data: &[u8]) -> ZxResult<usize> {
        let data_len = self.inner.lock().data.len();
        let was_empty = data_len == 0;
        let rest_size = SOCKET_SIZE - data_len;
        if rest_size == 0 {
            return Err(ZxError::SHOULD_WAIT);
        }
        let actual_count = if self.flags.contains(SocketFlags::DATAGRAM) {
            if data.len() > SOCKET_SIZE {
                return Err(ZxError::OUT_OF_RANGE);
            }
            if data.is_empty() {
                return Err(ZxError::INVALID_ARGS);
            }
            self.write_datagram(&data)?
        } else {
            self.write_stream(&data[..data.len().min(rest_size)])?
        };
        if actual_count > 0 {
            let mut set = Signal::empty();
            if was_empty {
                set |= Signal::READABLE;
            }
            let inner = self.inner.lock();
            if inner.read_threshold > 0 && inner.data.len() >= inner.read_threshold {
                set |= Signal::SOCKET_READ_THRESHOLD;
            }
            self.base.signal_set(set);
        }
        Ok(actual_count)
    }

    fn write_datagram(&self, data: &[u8]) -> ZxResult<usize> {
        let mut inner = self.inner.lock();
        let actual_count = data.len();
        inner.data.extend(&data[..]);
        inner.datagram_len.push_back(actual_count);
        Ok(actual_count)
    }

    fn write_stream(&self, data: &[u8]) -> ZxResult<usize> {
        let actual_count = data.len();
        let mut inner = self.inner.lock();
        inner.data.extend(&data[..]);
        Ok(actual_count)
    }

    /// Read data from the socket.
    pub fn read(&self, options: SocketFlags, data: &mut [u8]) -> ZxResult<usize> {
        if options.contains(SocketFlags::SOCKET_CONTROL) {
            if !self.flags.contains(SocketFlags::HAS_CONTROL) {
                return Err(ZxError::BAD_STATE);
            }
            self.read_control(options, data)
        } else {
            self.read_data(options, data)
        }
    }

    fn read_control(&self, options: SocketFlags, data: &mut [u8]) -> ZxResult<usize> {
        let mut inner = self.inner.lock();
        if inner.control_msg.is_empty() {
            return Err(ZxError::SHOULD_WAIT);
        }
        let read_size = data.len().min(inner.control_msg.len());
        if options.contains(SocketFlags::SOCKET_PEEK) {
            for (i, x) in inner.control_msg.iter().take(read_size).enumerate() {
                data[i] = *x;
            }
        } else {
            for (i, x) in inner.control_msg.drain(..read_size).enumerate() {
                data[i] = x;
            }
            self.base.signal_clear(Signal::SOCKET_CONTROL_READABLE);
            if let Some(peer) = self.peer.upgrade() {
                peer.base.signal_set(Signal::SOCKET_CONTROL_WRITABLE);
            }
        }
        Ok(read_size)
    }

    fn read_data(&self, options: SocketFlags, data: &mut [u8]) -> ZxResult<usize> {
        let data_len = self.inner.lock().data.len();
        if data_len == 0 {
            let _peer = self.peer.upgrade().ok_or(ZxError::PEER_CLOSED)?;
            let inner = self.inner.lock();
            if inner.read_disabled {
                return Err(ZxError::BAD_STATE);
            }
            return Err(ZxError::SHOULD_WAIT);
        }
        let was_full = data_len == SOCKET_SIZE;
        let peek = options.contains(SocketFlags::SOCKET_PEEK);
        let actual_count = if self.flags.contains(SocketFlags::DATAGRAM) {
            self.read_datagram(options, data, peek)?
        } else {
            self.read_stream(options, data, peek)?
        };
        if !peek && actual_count > 0 {
            let inner = self.inner.lock();
            let mut clear = Signal::empty();
            if inner.read_threshold > 0 && inner.data.len() < inner.read_threshold {
                clear |= Signal::SOCKET_READ_THRESHOLD;
            }
            if inner.data.is_empty() {
                clear |= Signal::READABLE;
            }
            self.base.signal_clear(clear);
            if let Ok(peer) = self.peer.upgrade().ok_or(ZxError::PEER_CLOSED) {
                let mut set = Signal::empty();
                let peer_inner = peer.inner.lock();
                if peer_inner.write_threshold > 0
                    && SOCKET_SIZE - inner.data.len() >= peer_inner.write_threshold
                {
                    set |= Signal::SOCKET_WRITE_THRESHOLD;
                }
                if was_full {
                    set |= Signal::WRITABLE;
                }
                peer.base.signal_set(set);
            }
        }
        Ok(actual_count)
    }

    fn read_datagram(&self, _options: SocketFlags, data: &mut [u8], peek: bool) -> ZxResult<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        let mut inner = self.inner.lock();
        let datagram_len = if peek {
            *inner.datagram_len.get(0).unwrap()
        } else {
            inner.datagram_len.pop_front().unwrap()
        };
        let read_size = data.len().min(datagram_len);
        if peek {
            for (i, x) in inner.data.iter().take(read_size).enumerate() {
                data[i] = *x;
            }
        } else {
            for (i, x) in inner.data.drain(..datagram_len).take(read_size).enumerate() {
                data[i] = x;
            }
        };
        Ok(read_size)
    }

    fn read_stream(&self, _options: SocketFlags, data: &mut [u8], peek: bool) -> ZxResult<usize> {
        let mut inner = self.inner.lock();
        let read_size = data.len().min(inner.data.len());
        if peek {
            for (i, x) in inner.data.iter().take(read_size).enumerate() {
                data[i] = *x;
            }
        } else {
            for (i, x) in inner.data.drain(..read_size).enumerate() {
                data[i] = x;
            }
        };
        Ok(read_size)
    }

    /// Get information of the socket.
    pub fn get_info(&self) -> SocketInfo {
        let inner = self.inner.lock();
        let self_size = inner.data.len();
        let peer_size = match self.peer.upgrade() {
            Some(peer) => peer.inner.lock().data.len(),
            None => 0,
        };
        let rx_buf_available = if self.flags.contains(SocketFlags::DATAGRAM) {
            *inner.datagram_len.get(0).unwrap_or(&0)
        } else {
            self_size
        };
        SocketInfo {
            options: self.flags.bits(),
            padding1: 0,
            rx_buf_max: SOCKET_SIZE as _,
            rx_buf_size: self_size as _,
            rx_buf_available: rx_buf_available as _,
            tx_buf_max: SOCKET_SIZE as _,
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
        if threshold > SOCKET_SIZE {
            return Err(ZxError::INVALID_ARGS);
        }
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
        if threshold > SOCKET_SIZE {
            return Err(ZxError::INVALID_ARGS);
        }
        self.inner.lock().write_threshold = threshold;
        if threshold == 0 {
            self.base.signal_clear(Signal::SOCKET_WRITE_THRESHOLD);
        } else if SOCKET_SIZE - peer.inner.lock().data.len() >= threshold {
            self.base.signal_set(Signal::SOCKET_WRITE_THRESHOLD);
        } else {
            self.base.signal_clear(Signal::SOCKET_WRITE_THRESHOLD);
        }
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
