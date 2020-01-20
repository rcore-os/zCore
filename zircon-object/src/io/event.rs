use super::*;
use crate::object::*;
use alloc::sync::{Arc, Weak};

pub type Event = DummyObject;

pub struct EventPair {
    base: KObjectBase,
    peer: Weak<EventPair>,
}

impl_kobject!(EventPair);

impl EventPair {
    /// Create a pair of event.
    #[allow(unsafe_code)]
    pub fn create() -> (Arc<Self>, Arc<Self>) {
        let mut event0 = Arc::new(EventPair {
            base: KObjectBase::default(),
            peer: Weak::default(),
        });
        let event1 = Arc::new(EventPair {
            base: KObjectBase::default(),
            peer: Arc::downgrade(&event0),
        });
        // no other reference of `channel0`
        unsafe {
            Arc::get_mut_unchecked(&mut event0).peer = Arc::downgrade(&event1);
        }
        (event0, event1)
    }

    /// Get the peer event.
    pub fn peer(&self) -> ZxResult<Arc<Self>> {
        self.peer.upgrade().ok_or(ZxError::PEER_CLOSED)
    }
}

impl Drop for EventPair {
    fn drop(&mut self) {
        if let Some(peer) = self.peer.upgrade() {
            peer.base.signal_set(Signal::PEER_CLOSED);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_closed() {
        let (event0, event1) = EventPair::create();
        assert!(Arc::ptr_eq(&event0.peer().unwrap(), &event1));

        drop(event1);
        assert_eq!(event0.signal(), Signal::PEER_CLOSED);
        assert_eq!(event0.peer().err(), Some(ZxError::PEER_CLOSED));
    }
}
