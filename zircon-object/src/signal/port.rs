use super::*;
use crate::object::*;
use alloc::collections::vec_deque::VecDeque;
use alloc::sync::Arc;
use spin::Mutex;

/// Signaling and mailbox primitive
///
/// ## SYNOPSIS
///
/// Ports allow threads to wait for packets to be delivered from various
/// events. These events include explicit queueing on the port,
/// asynchronous waits on other handles bound to the port, and
/// asynchronous message delivery from IPC transports.
pub struct Port {
    base: KObjectBase,
    inner: Mutex<PortInner>,
}

impl_kobject!(Port);

#[derive(Default)]
struct PortInner {
    queue: VecDeque<PortPacket>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PortPacket {
    pub key: u64,
    pub _type: PortPacketType,
    pub status: ZxError,
    pub data: [u8; 32],
}

#[repr(C)]
#[derive(Debug, Eq, PartialEq)]
pub struct PortPacketSignal {
    pub trigger: Signal,
    pub observed: Signal,
    pub count: u64,
    pub timestamp: u64,
    pub reserved1: u64,
}

// reference: zircon/system/public/zircon/syscalls/port.h ZX_PKT_TYPE_*
#[repr(u32)]
#[derive(Debug, Eq, PartialEq)]
pub enum PortPacketType {
    User = 0u32,
    SignalOne = 1u32,
    SignalRep = 2u32,
    GuestBell = 3u32,
    GuestMem = 4u32,
    GuestIo = 5u32,
    GuestVcpu = 6u32,
    Interrupt = 7u32,
    Exception = 8u32, // TODO should be Exception(n) = 0x8 | ((0xFF & n) << 8)
    PageRequest = 9u32,
}

impl Port {
    /// Create a new `Port`.
    pub fn new() -> Arc<Self> {
        Arc::new(Port {
            base: KObjectBase::default(),
            inner: Mutex::default(),
        })
    }

    /// Push a `packet` into the port.
    pub fn push(&self, packet: PortPacket) {
        let mut inner = self.inner.lock();
        inner.queue.push_back(packet);
        drop(inner);
        self.base.signal_set(Signal::READABLE);
    }

    /// Asynchronous wait until at least one packet is available, then take out all packets.
    pub async fn wait_async(self: &Arc<Self>) -> PortPacket {
        (self.clone() as Arc<dyn KernelObject>)
            .wait_signal_async(Signal::READABLE)
            .await;
        let mut inner = self.inner.lock();
        self.base.signal_clear(Signal::READABLE);
        inner.queue.pop_front().unwrap()
    }

    /// Get the number of packets in queue.
    #[allow(dead_code)]
    fn len(&self) -> usize {
        self.inner.lock().queue.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[async_std::test]
    async fn wait_async() {
        let port = Port::new();
        let object = DummyObject::new() as Arc<dyn KernelObject>;
        object.send_signal_to_port_async(Signal::READABLE, &port, 1);

        async_std::task::spawn({
            let port = port.clone();
            let object = object.clone();
            async move {
                object.signal_set(Signal::READABLE);
                async_std::task::sleep(Duration::from_millis(1)).await;

                port.push(PortPacket {
                    key: 2,
                    status: ZxError::OK,
                    data: PortPacketPayload::Signal(Signal::WRITABLE),
                });
            }
        });

        let packets = port.wait_async().await;
        assert_eq!(
            packets,
            [PortPacket {
                key: 1,
                status: ZxError::OK,
                data: PortPacketPayload::Signal(Signal::READABLE),
            }]
        );

        let packets = port.wait_async().await;
        assert_eq!(
            packets,
            [PortPacket {
                key: 2,
                status: ZxError::OK,
                data: PortPacketPayload::Signal(Signal::WRITABLE),
            }]
        );
    }
}
