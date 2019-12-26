use crate::object::*;
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::any::Any;
use spin::Mutex;

pub type Channel = Channel_<MessagePacket>;

pub struct Channel_<T> {
    base: KObjectBase,
    send_queue: Weak<Mutex<VecDeque<T>>>,
    recv_queue: Arc<Mutex<VecDeque<T>>>,
}

impl_kobject!(Channel);

impl<T> Channel_<T> {
    /// Create a channel and return a pair of its endpoints
    pub fn create() -> (Arc<Self>, Arc<Self>) {
        let queue0 = Arc::new(Mutex::new(VecDeque::new()));
        let queue1 = Arc::new(Mutex::new(VecDeque::new()));
        let channel0 = Arc::new(Channel_ {
            base: KObjectBase::new(),
            send_queue: Arc::downgrade(&queue0),
            recv_queue: queue1.clone(),
        });
        let channel1 = Arc::new(Channel_ {
            base: KObjectBase::new(),
            send_queue: Arc::downgrade(&queue1),
            recv_queue: queue0,
        });
        (channel0, channel1)
    }

    /// Read a packet from the channel
    pub fn read(&self) -> ZxResult<T> {
        if let Some(msg) = self.recv_queue.lock().pop_front() {
            return Ok(msg);
        }
        if self.peer_closed() {
            return Err(ZxError::PEER_CLOSED);
        } else {
            return Err(ZxError::SHOULD_WAIT);
        }
    }

    /// Write a packet to the channel
    pub fn write(&self, msg: T) -> ZxResult<()> {
        self.send_queue
            .upgrade()
            .ok_or(ZxError::PEER_CLOSED)?
            .lock()
            .push_back(msg);
        Ok(())
    }

    fn peer_closed(&self) -> bool {
        self.send_queue.strong_count() == 0
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
}
