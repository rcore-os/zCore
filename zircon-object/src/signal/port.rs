pub use self::port_packet::*;
use crate::object::*;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::sync::Arc;
use bitflags::bitflags;
use core::sync::atomic::{AtomicBool, Ordering};
use futures::channel::oneshot::Receiver;
use kernel_hal::sync::Mutex;

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
    queue: VecDeque<QueuedPacket>,
    observers: BTreeMap<u64, Observer>,
    next_observer: u64,
    interrupt_queue: VecDeque<PortInterruptPacket>,
    interrupt_grave: BTreeSet<u64>,
    interrupt_pid: u64,
}

#[derive(Debug)]
struct QueuedPacket {
    packet: PortPacket,
    observer: Option<u64>,
}

#[derive(Debug)]
struct Observer {
    source: (KoID, HandleValue),
    key: u64,
    signals: Signal,
    options: WaitAsyncOptions,
    cancel: Option<Receiver<()>>,
}

impl Observer {
    fn handle_closed(&mut self) -> bool {
        self.cancel
            .as_mut()
            .is_some_and(|cancel| !matches!(cancel.try_recv(), Ok(None)))
    }
}

#[derive(Debug)]
struct PortInterruptPacket {
    timestamp: i64,
    key: u64,
    pid: u64,
}

impl From<PortInterruptPacket> for PacketInterrupt {
    fn from(packet: PortInterruptPacket) -> Self {
        PacketInterrupt {
            timestamp: packet.timestamp,
            _reserved0: 0,
            _reserved1: 0,
            _reserved2: 0,
        }
    }
}

impl Port {
    /// Create a new `Port`.
    pub fn new(options: u32) -> ZxResult<Arc<Self>> {
        Ok(Arc::new(Port {
            base: KObjectBase::default(),
            options: PortOptions::from_bits(options).ok_or(ZxError::INVALID_ARGS)?,
            inner: Mutex::default(),
        }))
    }

    /// Push a `packet` into the port.
    pub fn push(&self, packet: impl Into<PortPacket>) {
        let mut inner = self.inner.lock();
        inner.queue.push_back(QueuedPacket {
            packet: packet.into(),
            observer: None,
        });
        self.base.signal_set(Signal::READABLE);
    }

    /// Push a `User` type `packet` into the port.
    pub fn push_user(&self, packet: impl Into<PortPacket>) -> ZxResult<()> {
        let mut packet = packet.into();
        packet.type_ = PacketType::User;
        let mut inner = self.inner.lock();
        if inner.queue.len() >= MAX_ALLOCATED_PACKET_COUNT_PER_PORT {
            return Err(ZxError::SHOULD_WAIT);
        }
        inner.queue.push_back(QueuedPacket {
            packet,
            observer: None,
        });
        self.base.signal_set(Signal::READABLE);
        Ok(())
    }

    /// Register a one-shot signal wait, identified by its source handle and key.
    pub fn wait_async(
        self: &Arc<Self>,
        object: &Arc<dyn KernelObject>,
        source: (KoID, HandleValue),
        key: u64,
        signals: Signal,
        options: WaitAsyncOptions,
        cancel: Option<Receiver<()>>,
    ) {
        let id = {
            let mut inner = self.inner.lock();
            inner.next_observer += 1;
            let id = inner.next_observer;
            inner.observers.insert(
                id,
                Observer {
                    source,
                    key,
                    signals,
                    options,
                    cancel,
                },
            );
            id
        };
        let port = Arc::downgrade(self);
        let initial = AtomicBool::new(true);
        object.add_signal_callback(Box::new(move |observed| {
            let Some(port) = port.upgrade() else {
                return true;
            };
            port.signal_observer(id, observed, initial.swap(false, Ordering::Relaxed))
        }));
    }

    fn signal_observer(&self, id: u64, observed: Signal, initial: bool) -> bool {
        let mut inner = self.inner.lock();
        let Some(observer) = inner.observers.get_mut(&id) else {
            return true;
        };
        if observer.handle_closed() {
            inner.observers.remove(&id);
            return true;
        }
        if !observed.intersects(observer.signals)
            || (initial && observer.options.contains(WaitAsyncOptions::EDGE))
        {
            return false;
        }
        let timestamp = if observer
            .options
            .intersects(WaitAsyncOptions::TIMESTAMP | WaitAsyncOptions::BOOT_TIMESTAMP)
        {
            kernel_hal::timer::timer_now().as_nanos() as u64
        } else {
            0
        };
        let packet = PortPacketRepr {
            key: observer.key,
            status: ZxError::OK,
            data: PayloadRepr::Signal(PacketSignal {
                trigger: observer.signals,
                observed,
                count: 1,
                timestamp,
                _reserved1: 0,
            }),
        }
        .into();
        inner.queue.push_back(QueuedPacket {
            packet,
            observer: Some(id),
        });
        self.base.signal_set(Signal::READABLE);
        true
    }

    /// Cancel pending waits and queued packets matching a key and optional source.
    pub fn cancel(&self, source: Option<(KoID, HandleValue)>, key: u64) -> ZxResult {
        let mut inner = self.inner.lock();
        let mut found = false;
        inner.observers.retain(|_, observer| {
            let matched = observer.key == key && source.is_none_or(|s| s == observer.source);
            found |= matched;
            !matched
        });
        let PortInner {
            queue, observers, ..
        } = &mut *inner;
        queue.retain(|queued| {
            let remove = match queued.observer {
                Some(id) => !observers.contains_key(&id),
                None => source.is_none() && queued.packet.key == key,
            };
            found |= remove;
            !remove
        });
        if inner.queue.is_empty() && inner.interrupt_queue.is_empty() {
            self.base.signal_clear(Signal::READABLE);
        }
        if found {
            Ok(())
        } else {
            Err(ZxError::NOT_FOUND)
        }
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
        self.base.signal_set(Signal::READABLE);
        pid
    }

    /// Remove an `InterruptPacket` from the port.
    /// Return whether the packet is in the port
    pub(crate) fn remove_interrupt(&self, pid: u64) -> bool {
        let mut inner = self.inner.lock();
        inner.interrupt_grave.remove(&pid)
    }

    /// Asynchronous wait until at least one packet is available, then take out the earliest
    /// (in FIFO order) available packet.
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
            while let Some(queued) = inner.queue.pop_front() {
                if inner.queue.is_empty()
                    && (inner.interrupt_queue.is_empty() || !self.can_bind_to_interrupt())
                {
                    self.base.signal_clear(Signal::READABLE);
                }
                if let Some(id) = queued.observer {
                    let Some(mut observer) = inner.observers.remove(&id) else {
                        continue;
                    };
                    if observer.handle_closed() {
                        continue;
                    }
                }
                return queued.packet;
            }
        }
    }

    /// Get the number of packets in queue.
    #[allow(dead_code)]
    fn len(&self) -> usize {
        self.inner.lock().queue.len()
    }

    /// Check whether the port can be bound to an interrupt.
    pub fn can_bind_to_interrupt(&self) -> bool {
        self.options.contains(PortOptions::BIND_TO_INTERUPT)
    }
}

bitflags! {
    /// Options for one-shot asynchronous signal waits.
    pub struct WaitAsyncOptions: u32 {
        /// Include a monotonic timestamp in the signal packet.
        const TIMESTAMP = 1;
        /// Ignore the signal state at registration.
        const EDGE = 2;
        /// Include a boot timeline timestamp in the signal packet.
        const BOOT_TIMESTAMP = 4;
    }

    /// If you need this port to be bound to an interrupt, pass **BIND_TO_INTERRUPT** to *options*,
    /// otherwise it should be **0**.
    pub struct PortOptions: u32 {
        #[allow(clippy::identity_op)]
        /// Allow this port to be bound to an interrupt.
        const BIND_TO_INTERUPT         = 1 << 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn cancellation_distinguishes_source_handles_and_queued_packets() {
        use futures::FutureExt;
        let port = Port::new(0).unwrap();
        let object = DummyObject::new() as Arc<dyn KernelObject>;
        for source in [(1, 4), (1, 8)] {
            port.wait_async(
                &object,
                source,
                7,
                Signal::READABLE,
                WaitAsyncOptions::empty(),
                None,
            );
        }
        port.cancel(Some((1, 4)), 7).unwrap();
        object.signal_set(Signal::READABLE);
        assert_eq!(port.cancel(Some((1, 4)), 7), Err(ZxError::NOT_FOUND));
        assert_eq!(port.wait().now_or_never().unwrap().key, 7);
        assert_eq!(port.cancel(None, 7), Err(ZxError::NOT_FOUND));
        port.wait_async(
            &object,
            (1, 4),
            9,
            Signal::READABLE,
            WaitAsyncOptions::empty(),
            None,
        );
        port.cancel(None, 9).unwrap();
        assert!(port.wait().now_or_never().is_none());
    }

    #[test]
    fn observers_do_not_keep_ports_alive() {
        let object = DummyObject::new() as Arc<dyn KernelObject>;
        let port = Port::new(0).unwrap();
        let weak = Arc::downgrade(&port);
        port.wait_async(
            &object,
            (1, 4),
            7,
            Signal::READABLE,
            WaitAsyncOptions::empty(),
            None,
        );
        drop(port);
        assert!(weak.upgrade().is_none());
        object.signal_set(Signal::READABLE);
    }

    #[test]
    fn new() {
        assert!(Port::new(0).is_ok());
        assert!(Port::new(1).is_ok());
        assert_eq!(Port::new(2).unwrap_err(), ZxError::INVALID_ARGS);
    }

    #[async_std::test]
    async fn wait() {
        let port = Port::new(0).unwrap();
        let object = DummyObject::new() as Arc<dyn KernelObject>;
        object.send_signal_to_port_async(Signal::READABLE, &port, 1);

        let packet_repr2 = PortPacketRepr {
            key: 2,
            status: ZxError::OK,
            data: PayloadRepr::Signal(PacketSignal {
                trigger: Signal::WRITABLE,
                observed: Signal::WRITABLE,
                count: 1,
                timestamp: 0,
                _reserved1: 0,
            }),
        };
        async_std::task::spawn({
            let port = port.clone();
            let object = object.clone();
            let packet2 = packet_repr2.clone();
            async move {
                // Assert an irrelevant signal to test the `false` branch of the callback for `READABLE`.
                object.signal_set(Signal::USER_SIGNAL_0);
                object.signal_clear(Signal::USER_SIGNAL_0);
                object.signal_set(Signal::READABLE);
                async_std::task::sleep(Duration::from_millis(1)).await;
                port.push(packet2);
            }
        });

        let packet = port.wait().await;
        let packet_repr = PortPacketRepr {
            key: 1,
            status: ZxError::OK,
            data: PayloadRepr::Signal(PacketSignal {
                trigger: Signal::READABLE,
                observed: Signal::READABLE,
                count: 1,
                timestamp: 0,
                _reserved1: 0,
            }),
        };
        assert_eq!(PortPacketRepr::from(&packet), packet_repr);

        let packet = port.wait().await;
        assert_eq!(PortPacketRepr::from(&packet), packet_repr2);

        // Test asserting signal before `send_signal_to_port_async`.
        let port = Port::new(0).unwrap();
        let object = DummyObject::new() as Arc<dyn KernelObject>;
        object.signal_set(Signal::READABLE);
        object.send_signal_to_port_async(Signal::READABLE, &port, 1);
        let packet = port.wait().await;
        assert_eq!(PortPacketRepr::from(&packet), packet_repr);
    }
}
