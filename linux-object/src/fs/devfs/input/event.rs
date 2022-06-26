use alloc::{boxed::Box, collections::VecDeque, sync::Arc};
use core::task::{Context, Poll};
use core::{any::Any, future::Future, mem::size_of, pin::Pin};

use lock::Mutex;

use kernel_hal::drivers::prelude::{InputEvent, InputEventType};
use kernel_hal::drivers::scheme::InputScheme;
use rcore_fs::vfs::*;
use rcore_fs_devfs::DevFS;

use crate::time::TimeVal;

const BUF_CAPACITY: usize = 64;

const EVENT_DEV_MINOR_BASE: usize = 0x40;

/// The event structure itself
#[repr(C)]
struct TimedInputEvent {
    time: TimeVal,
    event_type: InputEventType,
    code: u16,
    value: i32,
}

struct EventDevInner {
    buf: VecDeque<TimedInputEvent>,
}

/// Event char device, giving access to raw input device events.
pub struct EventDev {
    id: usize,
    inode_id: usize,
    input: Arc<dyn InputScheme>,
    inner: Arc<Mutex<EventDevInner>>,
}

impl TimedInputEvent {
    pub fn from(e: &InputEvent) -> Self {
        TimedInputEvent {
            time: TimeVal::now(),
            event_type: e.event_type,
            code: e.code,
            value: e.value,
        }
    }

    #[allow(unsafe_code)]
    pub fn as_buf(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self as *const _ as _, size_of::<TimedInputEvent>()) }
    }
}

impl EventDevInner {
    fn read_at(&mut self, buf: &mut [u8]) -> Result<usize> {
        let event_size = size_of::<TimedInputEvent>();
        if buf.len() < event_size {
            return Err(FsError::InvalidParam);
        }
        if self.buf.is_empty() {
            return Err(FsError::Again);
        }
        let mut read = 0;
        while read + event_size <= buf.len() {
            if let Some(e) = self.buf.pop_front() {
                buf[read..read + event_size].copy_from_slice(e.as_buf());
                read += event_size;
            } else {
                break;
            }
        }
        Ok(read)
    }

    fn handle_input_event(&mut self, e: &InputEvent) {
        while self.buf.len() >= BUF_CAPACITY {
            self.buf.pop_front();
        }
        self.buf.push_back(TimedInputEvent::from(e));
    }
}

impl EventDev {
    /// Create a input event INode
    pub fn new(input: Arc<dyn InputScheme>, id: usize) -> Self {
        let inner = Arc::new(Mutex::new(EventDevInner {
            buf: VecDeque::with_capacity(BUF_CAPACITY),
        }));
        let cloned = inner.clone();
        input.subscribe(
            Box::new(move |e| cloned.lock().handle_input_event(e)),
            false,
        );
        Self {
            id,
            input,
            inner,
            inode_id: DevFS::new_inode_id(),
        }
    }

    fn can_read(&self) -> bool {
        !self.inner.lock().buf.is_empty()
    }
}

impl INode for EventDev {
    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        self.inner.lock().read_at(buf)
    }

    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: self.can_read(),
            write: false,
            error: false,
        })
    }

    fn async_poll<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<PollStatus>> + Send + Sync + 'a>> {
        #[must_use = "future does nothing unless polled/`await`-ed"]
        struct EventFuture<'a> {
            dev: &'a EventDev,
        }

        impl<'a> Future for EventFuture<'a> {
            type Output = Result<PollStatus>;

            fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
                if self.dev.can_read() {
                    return Poll::Ready(self.dev.poll());
                }
                let waker = cx.waker().clone();
                self.dev
                    .input
                    .subscribe(Box::new(move |_| waker.wake_by_ref()), true);
                Poll::Pending
            }
        }

        Box::pin(EventFuture { dev: self })
    }

    fn metadata(&self) -> Result<Metadata> {
        Ok(Metadata {
            dev: 1,
            inode: self.inode_id,
            size: 0,
            blk_size: 0,
            blocks: 0,
            atime: Timespec { sec: 0, nsec: 0 },
            mtime: Timespec { sec: 0, nsec: 0 },
            ctime: Timespec { sec: 0, nsec: 0 },
            type_: FileType::CharDevice,
            mode: 0o660,
            nlinks: 1,
            uid: 0,
            gid: 0,
            rdev: make_rdev(0xd, EVENT_DEV_MINOR_BASE + self.id),
        })
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}
