use crate::object::*;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use spin::Mutex;

pub struct Channel {
    base: KObjectBase,
    send_queue: Arc<Mutex<VecDeque<MessagePacket>>>,
    recv_queue: Arc<Mutex<VecDeque<MessagePacket>>>,
}

impl_kobject!(Channel);

impl Channel {
    /// Create a channel and return a pair of its endpoints
    pub fn create() -> (Handle, Handle) {
        let queue0 = Arc::new(Mutex::new(VecDeque::new()));
        let queue1 = Arc::new(Mutex::new(VecDeque::new()));
        let end0 = Channel {
            base: KObjectBase::new(),
            send_queue: queue0.clone(),
            recv_queue: queue1.clone(),
        };
        let end1 = Channel {
            base: KObjectBase::new(),
            send_queue: queue1,
            recv_queue: queue0,
        };
        let handle0 = Handle::new(Arc::new(end0), Rights::DEFAULT_CHANNEL);
        let handle1 = Handle::new(Arc::new(end1), Rights::DEFAULT_CHANNEL);
        (handle0, handle1)
    }

    /// Read a packet from the channel
    pub fn read(&self) -> Option<MessagePacket> {
        self.recv_queue.lock().pop_front()
    }

    /// Write a packet to the channel
    pub fn write(&self, msg: MessagePacket) {
        self.send_queue.lock().push_back(msg)
    }
}

pub struct MessagePacket {
    pub data: Vec<u8>,
    pub handles: Vec<Handle>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn is_work() {
        use crate::error::*;
        use crate::ipc::channel::Channel;
        use crate::object::handle::Handle;
        use crate::object::KernelObject;

        let (handle0, handle1) = Channel::create();
        let ch0 = handle0.object.downcast_ref::<Channel>().unwrap();
        let ch1 = handle1.object.downcast_ref::<Channel>().unwrap();
        assert_eq!(ch1.id(), ch0.id() + 1);
    }
}
