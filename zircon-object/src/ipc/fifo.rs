use {
    crate::object::*,
    alloc::collections::VecDeque,
    alloc::sync::{Arc, Weak},
    spin::Mutex,
};

/// First-In First-Out inter-process queue.
///
/// FIFOs are intended to be the control plane for shared memory transports.
/// Their read and write operations are more efficient than sockets or channels,
/// but there are severe restrictions on the size of elements and buffers.
pub struct Fifo {
    base: KObjectBase,
    peer: Weak<Fifo>,
    elem_count: usize,
    elem_size: usize,
    recv_queue: Mutex<VecDeque<u8>>,
}

impl_kobject!(Fifo
    fn peer(&self) -> ZxResult<Arc<dyn KernelObject>> {
        let peer = self.peer.upgrade().ok_or(ZxError::PEER_CLOSED)?;
        Ok(peer)
    }
    fn related_koid(&self) -> KoID {
        self.peer.upgrade().map(|p| p.id()).unwrap_or(0)
    }
);

impl Fifo {
    /// Create a FIFO.
    #[allow(unsafe_code)]
    pub fn create(elem_count: usize, elem_size: usize) -> (Arc<Self>, Arc<Self>) {
        let mut end0 = Arc::new(Fifo {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            peer: Weak::default(),
            elem_count,
            elem_size,
            recv_queue: Mutex::new(VecDeque::with_capacity(elem_count * elem_size)),
        });
        let end1 = Arc::new(Fifo {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            peer: Arc::downgrade(&end0),
            elem_count,
            elem_size,
            recv_queue: Mutex::new(VecDeque::with_capacity(elem_count * elem_size)),
        });
        // no other reference of `end0`
        unsafe {
            Arc::get_mut_unchecked(&mut end0).peer = Arc::downgrade(&end1);
        }
        (end0, end1)
    }

    /// Write data to the FIFO.
    ///
    /// This attempts to write up to `count` elements (`count * elem_size` bytes)
    /// from `data` to the fifo.
    ///
    /// Fewer elements may be written than requested if there is insufficient room
    /// in the fifo to contain all of them.
    ///
    /// The number of elements actually written is returned.
    pub fn write(&self, elem_size: usize, data: &[u8], count: usize) -> ZxResult<usize> {
        if elem_size != self.elem_size {
            return Err(ZxError::OUT_OF_RANGE);
        }
        let count_size = count * elem_size;
        assert_eq!(data.len(), count_size);

        let peer = self.peer.upgrade().ok_or(ZxError::PEER_CLOSED)?;
        let mut recv_queue = peer.recv_queue.lock();
        let rest_capacity = self.capacity() - recv_queue.len();
        if rest_capacity == 0 {
            return Err(ZxError::SHOULD_WAIT);
        }
        if recv_queue.is_empty() {
            peer.base.signal_set(Signal::READABLE);
        }
        let write_len = count_size.min(rest_capacity);
        recv_queue.extend(&data[..write_len]);
        if recv_queue.len() == self.capacity() {
            self.base.signal_clear(Signal::WRITABLE);
        }
        Ok(write_len / elem_size)
    }

    /// Read data from the FIFO.
    ///
    /// This attempts to read up to `count` elements from the fifo into `data`.
    ///
    /// Fewer elements may be read than requested if there are insufficient elements
    /// in the fifo to fulfill the entire request.
    /// The number of elements actually read is returned.
    ///
    /// The `elem_size` must match the element size that was passed into `Fifo::create()`.
    ///
    /// `data` must have a size of `count * elem_size` bytes.
    pub fn read(&self, elem_size: usize, count: usize, data: &mut [u8]) -> ZxResult<usize> {
        if elem_size != self.elem_size {
            return Err(ZxError::OUT_OF_RANGE);
        }
        let count_size = count * elem_size;
        assert_eq!(data.len(), count_size);

        let peer = self.peer.upgrade();
        let mut recv_queue = self.recv_queue.lock();
        if recv_queue.is_empty() {
            if peer.is_none() {
                return Err(ZxError::PEER_CLOSED);
            }
            return Err(ZxError::SHOULD_WAIT);
        }
        let read_size = count_size.min(recv_queue.len());
        if recv_queue.len() == self.capacity() && peer.is_some() {
            peer.unwrap().base.signal_set(Signal::WRITABLE);
        }
        for (i, x) in recv_queue.drain(..read_size).enumerate() {
            data[i] = x;
        }
        if recv_queue.is_empty() {
            self.base.signal_clear(Signal::READABLE);
        }
        Ok(read_size / elem_size)
    }

    /// Get capacity in bytes.
    fn capacity(&self) -> usize {
        self.elem_size * self.elem_count
    }
}

impl Drop for Fifo {
    fn drop(&mut self) {
        if let Some(peer) = self.peer.upgrade() {
            peer.base
                .signal_change(Signal::WRITABLE, Signal::PEER_CLOSED);
        }
    }
}
