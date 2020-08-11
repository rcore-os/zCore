use super::*;
use crate::object::*;
use alloc::sync::{Arc, Weak};

/// Mutually signalable pair of events for concurrent programming
///
/// ## SYNOPSIS
///
/// Event Pairs are linked pairs of user-signalable objects. The 8 signal
/// bits reserved for userspace (`ZX_USER_SIGNAL_0` through
/// `ZX_USER_SIGNAL_7`) may be set or cleared on the local or opposing
/// endpoint of an Event Pair.
pub struct EventPair {
    base: KObjectBase,
    _counter: CountHelper,
    peer: Weak<EventPair>,
}

impl_kobject!(EventPair
    fn allowed_signals(&self) -> Signal {
        Signal::USER_ALL | Signal::SIGNALED
    }
    fn peer(&self) -> ZxResult<Arc<dyn KernelObject>> {
        let peer = self.peer.upgrade().ok_or(ZxError::PEER_CLOSED)?;
        Ok(peer)
    }
    fn related_koid(&self) -> KoID {
        self.peer.upgrade().map(|p| p.id()).unwrap_or(0)
    }
);
define_count_helper!(EventPair);

impl EventPair {
    /// Create a pair of event.
    #[allow(unsafe_code)]
    pub fn create() -> (Arc<Self>, Arc<Self>) {
        let mut event0 = Arc::new(EventPair {
            base: KObjectBase::default(),
            _counter: CountHelper::new(),
            peer: Weak::default(),
        });
        let event1 = Arc::new(EventPair {
            base: KObjectBase::default(),
            _counter: CountHelper::new(),
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
    fn test_allowed_signals() {
        let (event0, event1) = EventPair::create();
        assert!(Signal::verify_user_signal(
            event0.allowed_signals(),
            (Signal::USER_SIGNAL_5 | Signal::SIGNALED).bits().into()
        )
        .is_ok());
        assert_eq!(event0.allowed_signals(), event1.allowed_signals());

        event0.peer().unwrap();
    }

    #[test]
    fn peer_closed() {
        let (event0, event1) = EventPair::create();
        assert!(Arc::ptr_eq(&event0.peer().unwrap(), &event1));
        assert_eq!(event0.related_koid(), event1.id());

        drop(event1);
        assert_eq!(event0.signal(), Signal::PEER_CLOSED);
        assert_eq!(event0.peer().err(), Some(ZxError::PEER_CLOSED));
        assert_eq!(event0.related_koid(), 0);
    }
}
