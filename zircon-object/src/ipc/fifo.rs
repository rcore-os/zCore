use {
    crate::object::*,
    alloc::sync::{Arc, Weak},
};

pub struct Fifo {
    base: KObjectBase,
    peer: Weak<Fifo>,
    item_count : usize,
    item_size : usize,
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

zx_status_t WriteFromUser(size_t elem_size, user_in_ptr<const uint8_t> src, size_t count,
    size_t* actual);

impl Fifo {
    #[allow(unsafe_code)]
    pub fn create(_item_count: usize, _item_size: usize) -> (Arc<Self>, Arc<Self>) {
        let mut end0 = Arc::new(Fifo {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            peer: Weak::default(),
            item_count: _item_count,
            item_size:　_item_size,
            recv_queue: Mutex::new(VecDeque::with_capacity(item_count * item_size)),
        });
        let end1 = Arc::new(Fifo {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            peer: Arc::downgrade(&end0),
            item_count: _item_count,
            item_size:　_item_size,
            recv_queue: Mutex::new(VecDeque::with_capacity(item_count * item_size)),
        });
        // no other reference of `end0`
        unsafe {
            Arc::get_mut_unchecked(&mut end0).peer = Arc::downgrade(&end1);
        }
        (end0, end1)
    }

    pub fn write(elem_size : usize, data : Vec<u8>, count : usize, mut &actual : usize) {
        if elem_size != self.elem_size {
           return Err(ZxError::OUT_OF_RANGE);
        }
        let count_size = count * elem_size;
        let rest_capacity = item_count * item_size - recv_queue.len();
        if rest_capacity > count_size {
            *actual = rest_capacity - count_size;
        } else {
            *actual = count_size - rest_capacity;
        }
        data.truncate(*actual);
        let mut append_queue : VecDeque<u8> = data.into_iter().collect();
        recv_queue.append(&mut append_queue);
        OK()
    }

    pub fn read(elem_size : usize, data : UserOutPtr<u8>, count : usize, mut &actual : usize) {
        if elem_size != self.elem_size {
           return Err(ZxError::OUT_OF_RANGE);
        }
        let count_size = count * elem_size;
        let rest_size = recv_queue.len();
        if rest_size > count_size {
            *actual = count_size;
        } else {
            *actual = rest_size;
        }
        let item_vec: Vec<u8> = recv_queue.drain(..*actual).collect();
        data.write_array(item_vec.as_slice())?
    }
}
