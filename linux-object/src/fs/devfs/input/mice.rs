use alloc::{boxed::Box, collections::VecDeque, sync::Arc, vec::Vec};
use core::task::{Context, Poll};
use core::{any::Any, future::Future, pin::Pin};

use lock::Mutex;

use kernel_hal::drivers::prelude::input::{Mouse, MouseFlags, MouseState};
use kernel_hal::drivers::scheme::{EventScheme, InputScheme};
use rcore_fs::vfs::*;
use rcore_fs_devfs::DevFS;

const MAX_MOUSE_DEVICES: usize = 30;
const PACKET_SIZE: usize = 3;
const BUF_CAPACITY: usize = 32;

const MOUSE_DEV_MINOR_BASE: usize = 0x20;

struct MiceDevInner {
    packet_offset: usize,
    last_buttons: MouseFlags,
    buf: VecDeque<MouseState>,
}

/// mice device
pub struct MiceDev {
    id: usize,
    inode_id: usize,
    mice: Vec<Arc<Mouse>>,
    inner: Arc<Mutex<MiceDevInner>>,
}

impl MiceDevInner {
    fn read_at(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.buf.is_empty() {
            return Err(FsError::Again);
        }
        let mut read = 0;
        for i in 0..buf.len().min(PACKET_SIZE) {
            if let Some(p) = self.buf.front() {
                let data = p.as_ps2_buf();
                buf[i] = data[self.packet_offset];
                read += 1;
                self.packet_offset += 1;
                if self.packet_offset == PACKET_SIZE {
                    self.packet_offset = 0;
                    self.buf.pop_front();
                }
            } else {
                break;
            }
        }
        Ok(read)
    }

    fn handle_mouse_packet(&mut self, p: &MouseState) {
        if p.dx == 0 && p.dy == 0 && p.dz == 0 && p.buttons == self.last_buttons {
            return;
        }

        self.last_buttons = p.buttons;
        while self.buf.len() >= BUF_CAPACITY {
            self.packet_offset = 0;
            self.buf.pop_front();
        }
        self.buf.push_back(*p);
    }
}

impl MiceDev {
    /// Create a list of "mouseX" and "mice" INode from input devices.
    pub fn from_input_devices(inputs: &[Arc<dyn InputScheme>]) -> Vec<(Option<usize>, MiceDev)> {
        let mut mice = Vec::with_capacity(inputs.len());
        for i in inputs {
            if mice.len() < MAX_MOUSE_DEVICES && Mouse::compatible_with(i) {
                mice.push(Mouse::new(i.clone()));
            }
        }
        let mut ret = mice
            .iter()
            .enumerate()
            .map(|(i, m)| (Some(i), Self::new(m.clone(), i)))
            .collect::<Vec<_>>();
        if !mice.is_empty() {
            ret.push((None, Self::new_many(mice, MAX_MOUSE_DEVICES + 1)));
        }
        ret
    }

    /// Create a "mouseX" INode from one mouse device.
    pub fn new(mouse: Arc<Mouse>, id: usize) -> Self {
        Self::new_many(vec![mouse], id)
    }

    /// Create a "mice" INode from multiple mice.
    pub fn new_many(mice: Vec<Arc<Mouse>>, id: usize) -> Self {
        let inner = Arc::new(Mutex::new(MiceDevInner {
            packet_offset: 0,
            last_buttons: MouseFlags::empty(),
            buf: VecDeque::with_capacity(BUF_CAPACITY),
        }));
        for m in &mice {
            let cloned = inner.clone();
            m.subscribe(
                Box::new(move |p| cloned.lock().handle_mouse_packet(p)),
                false,
            );
        }
        Self {
            id,
            mice,
            inner,
            inode_id: DevFS::new_inode_id(),
        }
    }

    fn can_read(&self) -> bool {
        !self.inner.lock().buf.is_empty()
    }
}

impl INode for MiceDev {
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
        struct MiceFuture<'a> {
            dev: &'a MiceDev,
        }

        impl<'a> Future for MiceFuture<'a> {
            type Output = Result<PollStatus>;

            fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
                if self.dev.can_read() {
                    return Poll::Ready(self.dev.poll());
                }
                for m in &self.dev.mice {
                    let waker = cx.waker().clone();
                    m.subscribe(Box::new(move |_| waker.wake_by_ref()), true);
                }
                Poll::Pending
            }
        }

        Box::pin(MiceFuture { dev: self })
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
            rdev: make_rdev(0xd, MOUSE_DEV_MINOR_BASE + self.id),
        })
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}
