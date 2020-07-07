pub use self::port_packet::*;
use super::*;
use crate::object::*;
use alloc::collections::{BTreeSet, VecDeque};
use alloc::sync::Arc;
use bitflags::bitflags;
use spin::Mutex;

#[path = "port_packet.rs"]
mod port_packet;

const MAX_ALLOCATED_PACKET_COUNT: usize = 16 * 1024;
const MAX_ALLOCATED_PACKET_COUNT_PER_PORT: usize = MAX_ALLOCATED_PACKET_COUNT / 8;

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
    options: PortOptions,
    inner: Mutex<PortInner>,
}

impl_kobject!(Port);

#[derive(Default, Debug)]
struct PortInner {
    queue: VecDeque<PortPacket>,
    interrupt_queue: VecDeque<PortInterruptPacket>,
    interrupt_grave: BTreeSet<u64>,
    interrupt_pid: u64,
}

#[derive(Default, Debug)]
struct PortInterruptPacket {
    timestamp: i64,
    key: u64,
    pid: u64,
}

impl From<PortInterruptPacket> for PacketInterrupt {
    fn from(packet: PortInterruptPacket) -> Self {
        PacketInterrupt {
            timestamp: packet.timestamp,
            reserved0: 0,
            reserved1: 0,
            reserved2: 0,
        }
    }
}

impl Port {
    /// Create a new `Port`.
    pub fn new(options: u32) -> Arc<Self> {
        Arc::new(Port {
            base: KObjectBase::default(),
            options: PortOptions::from_bits_truncate(options),
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

    /// Push an `User` type `packet` into the port.
    pub fn push_user(&self, packet: impl Into<PortPacket>) -> ZxResult<()> {
        let mut packet = packet.into();
        packet.type_ = PacketType::User;
        if self.inner.lock().queue.len() > MAX_ALLOCATED_PACKET_COUNT_PER_PORT {
            return Err(ZxError::SHOULD_WAIT);
        }
        self.push(packet);
        Ok(())
    }

    /// Push an `InterruptPacket` into the port.
    pub(crate) fn push_interrupt(&self, timestamp: i64, key: u64) -> u64 {
        let mut inner = self.inner.lock();
        inner.interrupt_pid += 1;
        let pid = inner.interrupt_pid;
        inner.interrupt_queue.push_back(PortInterruptPacket {
            timestamp,
            key,
            pid,
        });
        inner.interrupt_grave.insert(pid);
        drop(inner);
        self.base.signal_set(Signal::READABLE);
        pid
    }

    /// Remove an `InterruptPacket` from the port.
    /// Return whether the packet is in the port
    pub(crate) fn remove_interrupt(&self, pid: u64) -> bool {
        let mut inner = self.inner.lock();
        inner.interrupt_grave.remove(&pid)
    }

    /// Asynchronous wait until at least one packet is available, then take out all packets.
    pub async fn wait(self: &Arc<Self>) -> PortPacket {
        let object = self.clone() as Arc<dyn KernelObject>;
        loop {
            object.wait_signal(Signal::READABLE).await;
            let mut inner = self.inner.lock();
            if self.can_bind_to_interrupt() {
                while let Some(packet) = inner.interrupt_queue.pop_front() {
                    let in_queue = inner.interrupt_grave.remove(&packet.pid);
                    if inner.queue.is_empty() && inner.interrupt_queue.is_empty() {
                        self.base.signal_clear(Signal::READABLE);
                    }
                    if !in_queue {
                        continue;
                    }
                    return PortPacketRepr {
                        key: packet.key,
                        status: ZxError::OK,
                        data: PayloadRepr::Interrupt(packet.into()),
                    }
                    .into();
                }
            }
            if let Some(packet) = inner.queue.pop_front() {
                if inner.queue.is_empty()
                    && (inner.interrupt_queue.is_empty() || !self.can_bind_to_interrupt())
                {
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

    pub fn can_bind_to_interrupt(&self) -> bool {
        self.options.contains(PortOptions::BIND_TO_INTERUPT)
    }
}

bitflags! {
    pub struct PortOptions: u32 {
        #[allow(clippy::identity_op)]
        const BIND_TO_INTERUPT         = 1 << 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[async_std::test]
    async fn wait() {
        let port = Port::new(0);
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
