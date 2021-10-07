use crate::time::TimeVal;
use alloc::{boxed::Box, collections::VecDeque, sync::Arc};
use core::any::Any;
use spin::Mutex;

use kernel_hal::dev::input;
use rcore_fs::vfs::*;

/// input device
pub struct InputEventInode {
    id: usize,
    data: Arc<Mutex<VecDeque<InputEvent>>>,
}

const MAX_QUEUE: usize = 32;

impl InputEventInode {
    /// Create a input event INode
    pub fn new(id: usize) -> Self {
        let data = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_QUEUE)));
        let data_clone = data.clone();
        input::kbd_set_callback(Box::new(move |code, value| {
            let mut queue = data_clone.lock();
            while queue.len() >= MAX_QUEUE {
                queue.pop_front();
            }
            queue.push_back(InputEvent::new(1, code, value));
        }));
        Self { id, data }
    }
}

impl INode for InputEventInode {
    #[allow(unsafe_code)]
    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        let event = self.data.lock().pop_front();
        if let Some(event) = event {
            let event: [u8; core::mem::size_of::<InputEvent>()] =
                unsafe { core::mem::transmute(event) };
            let len = event.len().min(buf.len());
            buf.copy_from_slice(&event[..len]);
            Ok(len)
        } else {
            Ok(0)
        }
    }

    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }

    fn metadata(&self) -> Result<Metadata> {
        Ok(Metadata {
            dev: 5,
            inode: 0,
            size: 0,
            blk_size: 0,
            blocks: 0,
            atime: Timespec { sec: 0, nsec: 0 },
            mtime: Timespec { sec: 0, nsec: 0 },
            ctime: Timespec { sec: 0, nsec: 0 },
            type_: FileType::CharDevice,
            mode: 0o666,
            nlinks: 1,
            uid: 0,
            gid: 0,
            rdev: make_rdev(13, 64 + self.id),
        })
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}

#[repr(C)]
/// The event structure itself
pub struct InputEvent {
    time: TimeVal,
    type_: u16,
    code: u16,
    value: i32,
}

impl InputEvent {
    /// Create a new input event
    pub fn new(type_: u16, code: u16, value: i32) -> Self {
        InputEvent {
            time: TimeVal::now(),
            type_,
            code,
            value,
        }
    }
}
