pub use self::port_packet::*;
use super::*;
use crate::object::*;
use alloc::collections::vec_deque::VecDeque;
use alloc::sync::Arc;
use spin::Mutex;

#[path = "port_packet.rs"]
mod port_packet;

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

#[derive(Default, Debug)]
struct PortInner {
    queue: VecDeque<PortPacket>,
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
    pub fn push(&self, packet: impl Into<PortPacket>) {
        let mut inner = self.inner.lock();
        inner.queue.push_back(packet.into());
        drop(inner);
        self.base.signal_set(Signal::READABLE);
    }

    /// Asynchronous wait until at least one packet is available, then take out all packets.
    pub async fn wait(self: &Arc<Self>) -> PortPacket {
        let object = self.clone() as Arc<dyn KernelObject>;
        loop {
            object.wait_signal(Signal::READABLE).await;
            let mut inner = self.inner.lock();
            if let Some(packet) = inner.queue.pop_front() {
                if inner.queue.is_empty() {
                    self.base.signal_clear(Signal::READABLE);
                }
                return packet;
            }
        }
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
    async fn wait() {
        let port = Port::new();
        let object = DummyObject::new() as Arc<dyn KernelObject>;
        object.send_signal_to_port_async(Signal::READABLE, &port, 1);

        let packet2 = PortPacketRepr {
            key: 2,
            status: ZxError::OK,
            data: PayloadRepr::Signal(PacketSignal {
                trigger: Signal::WRITABLE,
                observed: Signal::WRITABLE,
                count: 1,
                timestamp: 0,
            }),
        };
        async_std::task::spawn({
            let port = port.clone();
            let object = object.clone();
            let packet2 = packet2.clone();
            async move {
                object.signal_set(Signal::READABLE);
                async_std::task::sleep(Duration::from_millis(1)).await;
                port.push(packet2);
            }
        });

        let packet = port.wait().await;
        assert_eq!(
            PortPacketRepr::from(&packet),
            PortPacketRepr {
                key: 1,
                status: ZxError::OK,
                data: PayloadRepr::Signal(PacketSignal {
                    trigger: Signal::READABLE,
                    observed: Signal::READABLE,
                    count: 1,
                    timestamp: 0,
                }),
            }
        );

        let packet = port.wait().await;
        assert_eq!(PortPacketRepr::from(&packet), packet2);
    }
}
