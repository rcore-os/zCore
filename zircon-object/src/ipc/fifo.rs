use {
    crate::object::*,
    alloc::collections::VecDeque,
    alloc::sync::{Arc, Weak},
    alloc::vec::Vec,
    spin::Mutex,
};

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
    #[allow(unsafe_code)]
    pub fn create(_elem_count: usize, _elem_size: usize) -> (Arc<Self>, Arc<Self>) {
        let mut end0 = Arc::new(Fifo {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            peer: Weak::default(),
            elem_count: _elem_count,
            elem_size: _elem_size,
            recv_queue: Mutex::new(VecDeque::with_capacity(_elem_count * _elem_size)),
        });
        let end1 = Arc::new(Fifo {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            peer: Arc::downgrade(&end0),
            elem_count: _elem_count,
            elem_size: _elem_size,
            recv_queue: Mutex::new(VecDeque::with_capacity(_elem_count * _elem_size)),
        });
        // no other reference of `end0`
        unsafe {
            Arc::get_mut_unchecked(&mut end0).peer = Arc::downgrade(&end1);
        }
        (end0, end1)
    }

    pub fn write(
        &self,
        elem_size: usize,
        mut data: Vec<u8>,
        count: usize,
        actual: &mut usize,
    ) -> ZxResult {
        if elem_size != self.elem_size {
            return Err(ZxError::OUT_OF_RANGE);
        }
        let count_size = count * elem_size;
        let peer = self.peer.upgrade().ok_or(ZxError::PEER_CLOSED)?;
        let rest_capacity = peer.elem_count * elem_size - peer.recv_queue.lock().len();
        // error!("write rest = {}", rest_capacity);
        if rest_capacity == 0 {
            return Err(ZxError::SHOULD_WAIT);
        }
        if rest_capacity > count_size {
            *actual = count_size;
        } else {
            *actual = rest_capacity;
        }
        data.truncate(*actual);
        let mut append_queue: VecDeque<u8> = data.into_iter().collect();
        // error!("append len = {}", append_queue.len());
        peer.recv_queue.lock().append(&mut append_queue);
        // error!("after append len = {}", self.recv_queue.lock().len());
        if rest_capacity == peer.elem_count * elem_size {
            peer.base.signal_set(Signal::READABLE);
        }
        if rest_capacity == *actual {
            self.base.signal_clear(Signal::WRITABLE);
        }
        // error!(
        //     "after write rest = {}",
        //     self.elem_count * elem_size - self.recv_queue.lock().len()
        // );
        Ok(())
    }

    pub fn read(&self, elem_size: usize, count: usize, actual: &mut usize) -> ZxResult<Vec<u8>> {
        if elem_size != self.elem_size {
            return Err(ZxError::OUT_OF_RANGE);
        }
        let count_size = count * elem_size;
        let peer = self.peer.upgrade().ok_or(ZxError::PEER_CLOSED)?;
        let used_capacity = self.recv_queue.lock().len();
        //error!("write uesd = {}", used_capacity);
        if used_capacity == 0 {
            return Err(ZxError::SHOULD_WAIT);
        }
        if used_capacity > count_size {
            *actual = count_size;
        } else {
            *actual = used_capacity;
        }
        if used_capacity == self.elem_count * elem_size {
            peer.base.signal_set(Signal::WRITABLE);
        }
        if used_capacity == *actual {
            self.base.signal_clear(Signal::READABLE);
        }
        let vec = self.recv_queue.lock().drain(..*actual).collect::<Vec<u8>>();
        // error!("write after uesd = {}", self.recv_queue.lock().len());
        // error!("ret len = {}", vec.len());
        Ok(vec)
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
