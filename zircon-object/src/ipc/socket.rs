use {
    crate::object::*,
    alloc::sync::{Arc, Weak},
};

pub struct Socket {
    base: KObjectBase,
    peer: Weak<Socket>,
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

impl Socket {
    #[allow(unsafe_code)]
    pub fn create() -> (Arc<Self>, Arc<Self>) {
        let mut end0 = Arc::new(Socket {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            peer: Weak::default(),
        });
        let end1 = Arc::new(Socket {
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
