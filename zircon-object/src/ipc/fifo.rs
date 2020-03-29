use {
    crate::object::*,
    alloc::sync::{Arc, Weak},
};

pub struct Fifo {
    base: KObjectBase,
    peer: Weak<Fifo>,
}

impl_kobject!(Fifo
    fn peer(&self) -> ZxResult<Arc<dyn KernelObject>> {
        let peer = self.peer.upgrade().ok_or(ZxError::PEER_CLOSED)?;
        Ok(peer)
    }
    fn related_koid(&self) -> KoID {
        if let Some(peer) = self.peer.upgrade() {
            peer.id()
        } else {
            0
        }
    }
);

impl Fifo {
    #[allow(unsafe_code)]
    pub fn create(_item_count: usize, _item_size: usize) -> (Arc<Self>, Arc<Self>) {
        let mut end0 = Arc::new(Fifo {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            peer: Weak::default(),
        });
        let end1 = Arc::new(Fifo {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            peer: Arc::downgrade(&end0),
        });
        // no other reference of `end0`
        unsafe {
            Arc::get_mut_unchecked(&mut end0).peer = Arc::downgrade(&end1);
        }
        (end0, end1)
    }
}
