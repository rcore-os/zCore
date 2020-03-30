use {
    crate::object::*,
    crate::util::async_complete::{self, Sender},
    alloc::collections::{BTreeMap, VecDeque},
    alloc::sync::{Arc, Weak},
    alloc::vec::Vec,
    core::convert::TryInto,
    core::sync::atomic::{AtomicU32, Ordering},
    spin::Mutex,
};

/// Bidirectional interprocess communication
pub struct Channel {
    base: KObjectBase,
    peer: Weak<Channel>,
    recv_queue: Mutex<VecDeque<T>>,
    call_reply: Mutex<BTreeMap<TxID, Sender<ZxResult<T>>>>,
    next_txid: AtomicU32,
}

type T = MessagePacket;
type TxID = u32;

impl_kobject!(Channel
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

impl Channel {
    /// Create a channel and return a pair of its endpoints
    #[allow(unsafe_code)]
    pub fn create() -> (Arc<Self>, Arc<Self>) {
        let mut channel0 = Arc::new(Channel {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            peer: Weak::default(),
            recv_queue: Default::default(),
            call_reply: Default::default(),
            next_txid: AtomicU32::new(0x8000_0000),
        });
        let channel1 = Arc::new(Channel {
            base: KObjectBase::with_signal(Signal::WRITABLE),
            peer: Arc::downgrade(&channel0),
            recv_queue: Default::default(),
            call_reply: Default::default(),
            next_txid: AtomicU32::new(0x8000_0000),
        });
        // no other reference of `channel0`
        unsafe {
            Arc::get_mut_unchecked(&mut channel0).peer = Arc::downgrade(&channel1);
        }
        (channel0, channel1)
    }

    /// Read a packet from the channel if check is ok, otherwise the msg will keep.
    pub fn check_and_read(&self, checker: impl FnOnce(&T) -> ZxResult) -> ZxResult<T> {
        let mut recv_queue = self.recv_queue.lock();
        if let Some(msg) = recv_queue.front() {
            checker(msg)?;
            let msg = recv_queue.pop_front().unwrap();
            if recv_queue.is_empty() {
                self.base.signal_clear(Signal::READABLE);
            }
            return Ok(msg);
        }
        if self.peer_closed() {
            Err(ZxError::PEER_CLOSED)
        } else {
            Err(ZxError::SHOULD_WAIT)
        }
    }

    /// Read a packet from the channel
    pub fn read(&self) -> ZxResult<T> {
        self.check_and_read(|_| Ok(()))
    }

    /// Write a packet to the channel
    pub fn write(&self, msg: T) -> ZxResult {
        let peer = self.peer.upgrade().ok_or(ZxError::PEER_CLOSED)?;
        if msg.data.len() >= 4 {
            // check first 4 bytes: whether it is a call reply?
            let txid = TxID::from_ne_bytes(msg.data[..4].try_into().unwrap());
            if let Some(sender) = peer.call_reply.lock().remove(&txid) {
                sender.push(Ok(msg));
                return Ok(());
            }
        }
        peer.push_general(msg);
        Ok(())
    }

    /// Send a message to a channel and await a reply.
    pub async fn call(self: &Arc<Self>, mut msg: T) -> ZxResult<T> {
        let peer = self.peer.upgrade().ok_or(ZxError::PEER_CLOSED)?;
        let txid = self.new_txid();
        msg.data[..4].copy_from_slice(&txid.to_ne_bytes());
        peer.push_general(msg);
        let (sender, receiver) = async_complete::create();
        self.call_reply.lock().insert(txid, sender);
        drop(peer);
        receiver.await
    }

    /// Push a message to general queue, called from peer.
    fn push_general(&self, msg: T) {
        let mut send_queue = self.recv_queue.lock();
        send_queue.push_back(msg);
        if send_queue.len() == 1 {
            self.base.signal_set(Signal::READABLE);
        }
    }

    /// Generate a new transaction ID for `call`.
    fn new_txid(&self) -> TxID {
        self.next_txid.fetch_add(1, Ordering::SeqCst)
    }

    /// Is peer channel closed?
    fn peer_closed(&self) -> bool {
        self.peer.strong_count() == 0
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        if let Some(peer) = self.peer.upgrade() {
            peer.base
                .signal_change(Signal::WRITABLE, Signal::PEER_CLOSED);
            for (_, sender) in core::mem::take(&mut *peer.call_reply.lock()).into_iter() {
                sender.push(Err(ZxError::PEER_CLOSED));
            }
        }
    }
}

#[derive(Default)]
pub struct MessagePacket {
    pub data: Vec<u8>,
    pub handles: Vec<Handle>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use core::sync::atomic::*;

    #[test]
    fn read_write() {
        let (channel0, channel1) = Channel::create();
        // write a message to each other
        channel0
            .write(MessagePacket {
                data: Vec::from("hello 1"),
                handles: Vec::new(),
            })
            .unwrap();
        channel1
            .write(MessagePacket {
                data: Vec::from("hello 0"),
                handles: Vec::new(),
            })
            .unwrap();

        // read message should success
        let recv_msg = channel1.read().unwrap();
        assert_eq!(recv_msg.data.as_slice(), b"hello 1");
        assert!(recv_msg.handles.is_empty());

        let recv_msg = channel0.read().unwrap();
        assert_eq!(recv_msg.data.as_slice(), b"hello 0");
        assert!(recv_msg.handles.is_empty());

        // read more message should fail.
        assert_eq!(channel0.read().err(), Some(ZxError::SHOULD_WAIT));
        assert_eq!(channel1.read().err(), Some(ZxError::SHOULD_WAIT));
    }

    #[test]
    fn peer_closed() {
        let (channel0, channel1) = Channel::create();
        // write a message from peer, then drop it
        channel1.write(MessagePacket::default()).unwrap();
        drop(channel1);
        // read the first message should success.
        channel0.read().unwrap();
        // read more message should fail.
        assert_eq!(channel0.read().err(), Some(ZxError::PEER_CLOSED));
        // write message should fail.
        assert_eq!(
            channel0.write(MessagePacket::default()),
            Err(ZxError::PEER_CLOSED)
        );
    }

    #[test]
    fn signal() {
        let (channel0, channel1) = Channel::create();

        // initial status is writable and not readable.
        let init_signal = channel0.base.signal();
        assert!(!init_signal.contains(Signal::READABLE));
        assert!(init_signal.contains(Signal::WRITABLE));

        // register callback for `Signal::READABLE` & `Signal::PEER_CLOSED`:
        //   set `readable` and `peer_closed`
        let readable = Arc::new(AtomicBool::new(false));
        let peer_closed = Arc::new(AtomicBool::new(false));
        channel0.add_signal_callback(Box::new({
            let readable = readable.clone();
            let peer_closed = peer_closed.clone();
            move |signal| {
                readable.store(signal.contains(Signal::READABLE), Ordering::SeqCst);
                peer_closed.store(signal.contains(Signal::PEER_CLOSED), Ordering::SeqCst);
                false
            }
        }));

        // writing to peer should trigger `Signal::READABLE`.
        channel1.write(MessagePacket::default()).unwrap();
        assert!(readable.load(Ordering::SeqCst));

        // reading all messages should cause `Signal::READABLE` be cleared.
        channel0.read().unwrap();
        assert!(!readable.load(Ordering::SeqCst));

        // peer closed should trigger `Signal::PEER_CLOSED`.
        assert!(!peer_closed.load(Ordering::SeqCst));
        drop(channel1);
        assert!(peer_closed.load(Ordering::SeqCst));
    }
}
