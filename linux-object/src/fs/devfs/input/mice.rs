use alloc::{boxed::Box, collections::VecDeque, sync::Arc};
use core::any::Any;
use spin::Mutex;

use kernel_hal::dev::input;
use rcore_fs::vfs::*;

/// mice device
pub struct InputMiceInode {
    data: Arc<Mutex<VecDeque<[u8; 3]>>>,
}
const MAX_QUEUE: usize = 32;

impl InputMiceInode {
    /// Create a mice INode
    pub fn new() -> Self {
        let data = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_QUEUE)));
        let data_clone = data.clone();
        input::mouse_set_callback(Box::new(move |data| {
            let mut queue = data_clone.lock();
            while queue.len() >= MAX_QUEUE {
                queue.pop_front();
            }
            queue.push_back(data);
        }));
        Self { data }
    }
}

impl Default for InputMiceInode {
    fn default() -> Self {
        Self::new()
    }
}

impl INode for InputMiceInode {
    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        let data = self.data.lock().pop_front();
        if let Some(data) = data {
            let len = buf.len().min(3);
            buf[..len].copy_from_slice(&data[..len]);
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
            rdev: make_rdev(13, 63),
        })
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}
