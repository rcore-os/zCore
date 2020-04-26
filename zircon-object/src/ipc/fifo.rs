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
        let rest_capacity = self.elem_count * elem_size - self.recv_queue.lock().len();
        if rest_capacity > count_size {
            *actual = rest_capacity - count_size;
        } else {
            *actual = count_size - rest_capacity;
        }
        data.truncate(*actual);
        let mut append_queue: VecDeque<u8> = data.into_iter().collect();
        self.recv_queue.lock().append(&mut append_queue);
        Ok(())
    }

    pub fn read(&self, elem_size: usize, count: usize, actual: &mut usize) -> ZxResult<Vec<u8>> {
        if elem_size != self.elem_size {
            return Err(ZxError::OUT_OF_RANGE);
        }
        let count_size = count * elem_size;
        let rest_size = self.recv_queue.lock().len();
        if rest_size == 0 {
            return Err(ZxError::SHOULD_WAIT);
        }
        if rest_size > count_size {
            *actual = count_size;
        } else {
            *actual = rest_size;
        }
        Ok(self.recv_queue.lock().drain(..*actual).collect::<Vec<u8>>())
    }
}
