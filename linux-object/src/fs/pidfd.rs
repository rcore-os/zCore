//! Process file descriptor (`pidfd_open`).

use super::*;
use crate::sync::{Event, EventBus};
use alloc::sync::Arc;
use lock::Mutex;
use zircon_object::object::*;
use zircon_object::task::{Process, Status};

/// Linux `PIDFD_THREAD` (unsupported here — pid must name a process).
pub const PIDFD_THREAD: u32 = 2;

/// Anonymous fd referring to a live or zombie process (pollable on exit).
pub struct PidFd {
    base: KObjectBase,
    process: Arc<Process>,
    open_flags: Mutex<OpenFlags>,
    eventbus: Arc<Mutex<EventBus>>,
}

impl_kobject!(PidFd);

impl PidFd {
    /// Create a pidfd for `process`. `open_flags` may include `O_NONBLOCK` / `O_CLOEXEC`.
    pub fn new(process: Arc<Process>, open_flags: OpenFlags) -> Arc<Self> {
        let eventbus = EventBus::new();
        if matches!(process.status(), Status::Exited(_)) {
            eventbus.lock().set(Event::READABLE);
        }
        Arc::new(Self {
            base: KObjectBase::new(),
            process,
            open_flags: Mutex::new(open_flags),
            eventbus,
        })
    }

    /// Resolve a pidfd from the caller's fd table.
    pub fn from_file_like(file: Arc<dyn FileLike>) -> LxResult<Arc<Self>> {
        file.downcast_arc::<Self>().map_err(|_| LxError::EINVAL)
    }

    /// Target process.
    pub fn target(&self) -> &Arc<Process> {
        &self.process
    }

    pub(crate) fn exited(&self) -> bool {
        matches!(self.process.status(), Status::Exited(_))
    }
}

#[async_trait]
impl FileLike for PidFd {
    fn flags(&self) -> OpenFlags {
        *self.open_flags.lock()
    }

    fn set_flags(&self, f: OpenFlags) -> LxResult {
        let mut flags = self.open_flags.lock();
        flags.set(OpenFlags::NON_BLOCK, f.contains(OpenFlags::NON_BLOCK));
        flags.set(OpenFlags::CLOEXEC, f.contains(OpenFlags::CLOEXEC));
        Ok(())
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        Arc::new(Self {
            base: KObjectBase::new(),
            process: self.process.clone(),
            open_flags: Mutex::new(*self.open_flags.lock()),
            eventbus: self.eventbus.clone(),
        })
    }

    async fn read(&self, _buf: &mut [u8]) -> LxResult<usize> {
        Err(LxError::EINVAL)
    }

    fn write(&self, _buf: &[u8]) -> LxResult<usize> {
        Err(LxError::EINVAL)
    }

    async fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> LxResult<usize> {
        Err(LxError::EINVAL)
    }

    fn poll(&self, events: PollEvents) -> LxResult<PollStatus> {
        let exited = self.exited();
        Ok(PollStatus {
            read: exited && events.contains(PollEvents::IN),
            write: false,
            error: false,
        })
    }

    async fn async_poll(&self, events: PollEvents) -> LxResult<PollStatus> {
        if self.exited() {
            return self.poll(events);
        }
        if self.open_flags.lock().non_block() {
            return Ok(PollStatus::default());
        }
        let proc_obj: Arc<dyn KernelObject> = self.process.clone();
        proc_obj.wait_signal(Signal::PROCESS_TERMINATED).await;
        self.eventbus.lock().set(Event::READABLE);
        self.poll(events)
    }
}
