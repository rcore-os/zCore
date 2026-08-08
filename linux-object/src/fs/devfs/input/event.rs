use alloc::{boxed::Box, collections::VecDeque, sync::Arc};
use core::task::{Context, Poll};
use core::{any::Any, future::Future, mem::size_of, pin::Pin};

use lock::Mutex;

use kernel_hal::drivers::prelude::{CapabilityType, InputCapability, InputEvent, InputEventType};
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
            // evdev timestamps must be CLOCK_MONOTONIC (libinput sets that via
            // EVIOCSCLOCKID and times its filters against it); the wall clock
            // would desync libinput's button-debounce/tap/scroll timers.
            time: TimeVal::now_monotonic(),
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

/// Maps a `kernel_hal::user` pointer-operation result onto `FsError`.
/// `linux-object` cannot add a `From<kernel_hal::user::Error>` impl for
/// `FsError` (orphan rule: neither type is local to this crate), so every
/// checked-pointer call site below routes through this instead of a bare `?`.
/// Mirrors `fs/stdio.rs`'s private `user_copy`, which cannot be reused across
/// modules.
fn user_copy<T>(r: core::result::Result<T, kernel_hal::user::Error>) -> Result<T> {
    r.map_err(|_| FsError::InvalidParam)
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

    /// Map a Linux `EV_*` event-type code to the driver capability bitmap that
    /// `EVIOCGBIT(ev)` should report. `ev == 0` asks for the set of supported
    /// event types themselves.
    fn capability_for_ev(&self, ev: u16) -> InputCapability {
        let cap_type = match ev {
            0x00 => CapabilityType::Event,   // EVIOCGBIT(0): supported event types
            0x01 => CapabilityType::Key,     // EV_KEY
            0x02 => CapabilityType::RelAxis, // EV_REL
            0x03 => CapabilityType::AbsAxis, // EV_ABS
            0x04 => CapabilityType::Misc,    // EV_MSC
            0x05 => CapabilityType::Switch,  // EV_SW
            0x11 => CapabilityType::Led,     // EV_LED
            0x12 => CapabilityType::Sound,   // EV_SND
            0x15 => CapabilityType::FeedBack, // EV_FF
            _ => return InputCapability::empty(),
        };
        self.input.capability(cap_type)
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
        /// Parks on the input EventListener until readable. Must unsubscribe
        /// on Ready/`Drop`: poll/epoll re-scan (and Ready-after-subscribe) used
        /// to leave one-shot wakers that later `trigger` into freed tasks —
        /// UAF that surfaces as KERNEL PAGE FAULT after a long desktop idle.
        #[must_use = "future does nothing unless polled/`await`-ed"]
        struct EventFuture<'a> {
            dev: &'a EventDev,
            sub_id: Option<u64>,
        }

        impl Drop for EventFuture<'_> {
            fn drop(&mut self) {
                if let Some(id) = self.sub_id.take() {
                    self.dev.input.unsubscribe(id);
                }
            }
        }

        impl<'a> Future for EventFuture<'a> {
            type Output = Result<PollStatus>;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
                let this = self.as_mut().get_mut();
                // Fast path: data already available.
                if this.dev.can_read() {
                    if let Some(id) = this.sub_id.take() {
                        this.dev.input.unsubscribe(id);
                    }
                    return Poll::Ready(this.dev.poll());
                }
                // Register the waker BEFORE the second can_read() check to
                // eliminate the TOCTOU race: if an event arrives between the
                // first check (false) and subscribe(), it would not fire any
                // waker and the task would sleep indefinitely until the next
                // event.  By registering first, any event that arrives during
                // or after subscribe() will call the waker and reschedule the
                // task.
                if this.sub_id.is_none() {
                    let waker = cx.waker().clone();
                    this.sub_id = this
                        .dev
                        .input
                        .subscribe(Box::new(move |_| waker.wake_by_ref()), true);
                }
                // Re-check after registering the waker in case an event
                // arrived in the window between the first check and subscribe().
                if this.dev.can_read() {
                    if let Some(id) = this.sub_id.take() {
                        this.dev.input.unsubscribe(id);
                    }
                    return Poll::Ready(this.dev.poll());
                }
                Poll::Pending
            }
        }

        Box::pin(EventFuture {
            dev: self,
            sub_id: None,
        })
    }

    /// Implement the `EVIOC*` ioctls (Linux `<linux/input.h>`) that
    /// `evdev`/`libinput` issue while probing a device. The request encodes a
    /// direction, size, type (`'E'`) and number; we decode the number and the
    /// userspace buffer size from it.
    fn io_control(&self, cmd: u32, data: usize) -> Result<usize> {
        let size = (((cmd >> 16) & 0x3fff) as usize).min(256);
        let typ = (cmd >> 8) & 0xff;
        let nr = (cmd & 0xff) as usize;
        // Only the input ioctl group ('E').
        if typ != 'E' as u32 {
            return Err(FsError::NotSupported);
        }
        // EVIOCSCLOCKID / EVIOCGRAB / EVIOCREVOKE pass their argument by value
        // and never dereference the pointer; in particular seatd issues
        // EVIOCREVOKE with a NULL argument (`data == 0`) when it revokes a
        // device on session (de)activation. Handle them before the null-pointer
        // guard below — that guard is only for the ioctls that read/write
        // through `data`. We do not enforce grab/revoke, so accept as no-ops.
        // (Rejecting EVIOCREVOKE with EINVAL made seatd treat the open as failed
        // and never hand the device fd to libinput → "no input devices".)
        if matches!(nr, 0xa0 | 0x90 | 0x91) {
            return Ok(0);
        }
        if data == 0 {
            return Err(FsError::InvalidParam);
        }
        match nr {
            // EVIOCGVERSION -> EV_VERSION (0x010001).
            0x01 => {
                let mut ptr = kernel_hal::user::UserOutPtr::<i32>::from(data);
                user_copy(ptr.write(0x01_0001))?;
                Ok(core::mem::size_of::<i32>())
            }
            // EVIOCGID -> struct input_id { bustype, vendor, product, version }.
            // Report a virtual bus; vendor/product/version are not meaningful.
            0x02 => {
                let mut ptr = kernel_hal::user::UserOutPtr::<[u16; 4]>::from(data);
                user_copy(ptr.write([0x06, 0, 0, 0]))?;
                Ok(8)
            }
            // EVIOCGREP -> repeat [delay_ms, period_ms].
            0x03 => {
                let mut ptr = kernel_hal::user::UserOutPtr::<[u32; 2]>::from(data);
                user_copy(ptr.write([250, 33]))?;
                Ok(8)
            }
            // EVIOCGNAME(len) -> device name (NUL-terminated).
            0x06 => {
                let name = self.input.name().as_bytes();
                let n = (name.len() + 1).min(size);
                if n == 0 {
                    return Ok(0);
                }
                let mut buf = [0u8; 256];
                let body = n - 1;
                buf[..body].copy_from_slice(&name[..body]);
                // buf[body] is already 0 from initialization -- the NUL terminator.
                let mut ptr = kernel_hal::user::UserOutPtr::<u8>::from(data);
                user_copy(ptr.write_array(&buf[..n]))?;
                Ok(n)
            }
            // EVIOCGPHYS / EVIOCGUNIQ: physical location / unique id strings.
            // We have neither, but libevdev (which libinput uses) treats any
            // ioctl error here OTHER than ENOENT as fatal and aborts the whole
            // device setup — so returning ENOTTY made libinput reject every
            // device. Return an empty (NUL-terminated) string instead.
            0x07 | 0x08 => {
                let mut ptr = kernel_hal::user::UserOutPtr::<u8>::from(data);
                user_copy(ptr.write(0))?;
                Ok(1)
            }
            // EVIOCGPROP / EVIOCGKEY / EVIOCGLED / EVIOCGSND / EVIOCGSW: report
            // an all-zero state (no properties, nothing currently pressed/lit).
            0x09 | 0x18 | 0x19 | 0x1a | 0x1b => {
                let zeros = [0u8; 256];
                let mut ptr = kernel_hal::user::UserOutPtr::<u8>::from(data);
                user_copy(ptr.write_array(&zeros[..size]))?;
                Ok(size)
            }
            // EVIOCGBIT(ev, len): supported event types / codes bitmap.
            0x20..=0x3f => {
                let bytes = self.capability_for_ev((nr - 0x20) as u16).to_le_bytes();
                let n = size.min(bytes.len());
                let mut ptr = kernel_hal::user::UserOutPtr::<u8>::from(data);
                user_copy(ptr.write_array(&bytes[..n]))?;
                Ok(n)
            }
            // EVIOCGABS(abs): struct input_absinfo — zeroed (no absolute axes).
            0x40..=0x7f => {
                let n = size.min(24);
                let zeros = [0u8; 256];
                let mut ptr = kernel_hal::user::UserOutPtr::<u8>::from(data);
                user_copy(ptr.write_array(&zeros[..n]))?;
                Ok(0)
            }
            _ => Err(FsError::NotSupported),
        }
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
