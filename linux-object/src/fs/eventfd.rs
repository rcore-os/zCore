use super::*;
use crate::sync::{Event, EventBus};
use alloc::sync::Arc;
use core::convert::TryInto;
use core::sync::atomic::{AtomicU64, Ordering};
use lock::Mutex;
use zircon_object::object::*;

/// eventfd implementation
pub struct EventFd {
    base: KObjectBase,
    counter: Arc<AtomicU64>,
    eventbus: Arc<Mutex<EventBus>>,
    flags: OpenFlags,
}

impl_kobject!(EventFd);

impl EventFd {
    /// create an eventfd
    pub fn new(initval: u32, flags: OpenFlags) -> Arc<Self> {
        Arc::new(EventFd {
            base: KObjectBase::new(),
            counter: Arc::new(AtomicU64::new(initval as u64)),
            eventbus: EventBus::new(),
            flags,
        })
    }
}

#[async_trait]
impl FileLike for EventFd {
    fn flags(&self) -> OpenFlags {
        self.flags
    }

    fn set_flags(&self, _f: OpenFlags) -> LxResult {
        Ok(())
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        Arc::new(Self {
            base: KObjectBase::new(),
            counter: self.counter.clone(),
            eventbus: self.eventbus.clone(),
            flags: self.flags,
        })
    }

    async fn read(&self, buf: &mut [u8]) -> LxResult<usize> {
        if buf.len() < 8 {
            return Err(LxError::EINVAL);
        }
        loop {
            let counter = self.counter.load(Ordering::SeqCst);
            if counter > 0 {
                let res = if self.flags.bits() & 1 != 0 {
                    // EFD_SEMAPHORE
                    if self
                        .counter
                        .compare_exchange(counter, counter - 1, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        1
                    } else {
                        continue;
                    }
                } else {
                    if self
                        .counter
                        .compare_exchange(counter, 0, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        counter
                    } else {
                        continue;
                    }
                };
                buf[..8].copy_from_slice(&res.to_ne_bytes());
                if self.counter.load(Ordering::SeqCst) == 0 {
                    self.eventbus.lock().clear(Event::READABLE);
                }
                return Ok(8);
            }
            if self.flags.contains(OpenFlags::NON_BLOCK) {
                return Err(LxError::EAGAIN);
            }
            self.async_poll(PollEvents::IN).await?;
        }
    }

    fn write(&self, buf: &[u8]) -> LxResult<usize> {
        if buf.len() < 8 {
            return Err(LxError::EINVAL);
        }
        let val = u64::from_ne_bytes(buf[..8].try_into().unwrap());
        if val == u64::MAX {
            return Err(LxError::EINVAL);
        }
        loop {
            let counter = self.counter.load(Ordering::SeqCst);
            if u64::MAX - counter > val {
                if self
                    .counter
                    .compare_exchange(counter, counter + val, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    self.eventbus.lock().set(Event::READABLE);
                    return Ok(8);
                }
            } else {
                if self.flags.contains(OpenFlags::NON_BLOCK) {
                    return Err(LxError::EAGAIN);
                }
                // TODO: wait for writeable? EventFd is almost always writeable unless overflow
                return Err(LxError::EAGAIN);
            }
        }
    }

    async fn read_at(&self, _offset: u64, buf: &mut [u8]) -> LxResult<usize> {
        self.read(buf).await
    }

    fn poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        let counter = self.counter.load(Ordering::SeqCst);
        Ok(PollStatus {
            read: counter > 0,
            write: counter < u64::MAX - 1,
            error: false,
        })
    }

    async fn async_poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        loop {
            let status = self.poll(_events)?;
            let want_read = _events.contains(PollEvents::IN);
            let want_write = _events.contains(PollEvents::OUT);
            let ready = (want_read && status.read)
                || (want_write && status.write)
                || (!want_read && !want_write);
            if ready {
                return Ok(status);
            }
            let bus = self.eventbus.clone();
            crate::sync::wait_for_event(bus, Event::READABLE | Event::WRITABLE).await;
        }
    }

    fn subscribe_readiness(
        &self,
        events: PollEvents,
        waker: &core::task::Waker,
    ) -> Option<crate::sync::ReadinessSub> {
        let mask = super::poll_events_to_bus_mask(events);
        Some(crate::sync::subscribe_readiness_on(
            &self.eventbus,
            mask,
            waker,
        ))
    }
}
