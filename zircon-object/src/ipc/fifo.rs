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

    pub fn check_and_write()
}
