use alloc::{sync::Arc, vec::Vec};

use rcore_fs::vfs::*;
use rcore_fs_devfs::{special::*, DevFS};
use rcore_fs_mountfs::MountFS;
use rcore_fs_ramfs::RamFS;

pub use self::file::*;
pub use self::pseudo::*;
pub use self::random::*;
pub use self::stdio::*;
use crate::error::*;

use core::convert::TryFrom;
use downcast_rs::impl_downcast;
use zircon_object::object::*;

mod device;
mod file;
mod ioctl;
mod pseudo;
mod random;
mod stdio;

/// Generic file interface
///
/// - Normal file, Directory
/// - Socket
/// - Epoll instance
pub trait FileLike: KernelObject {
    fn read(&self, buf: &mut [u8]) -> LxResult<usize>;
    fn write(&self, buf: &[u8]) -> LxResult<usize>;
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> LxResult<usize>;
    fn write_at(&self, offset: usize, buf: &[u8]) -> LxResult<usize>;
    fn poll(&self) -> LxResult<PollStatus>;
    fn ioctl(&self, request: usize, arg1: usize, arg2: usize, arg3: usize) -> LxResult<()>;
    fn fcntl(&self, cmd: usize, arg: usize) -> LxResult<()>;
}

impl_downcast!(sync FileLike);

#[derive(Debug, Copy, Clone, Ord, PartialOrd, Eq, PartialEq)]
pub struct FileDesc(isize);

impl FileDesc {
    /// Pathname is interpreted relative to the current working directory(CWD)
    pub const CWD: Self = FileDesc(-100);
}

impl From<usize> for FileDesc {
    fn from(x: usize) -> Self {
        FileDesc(x as isize)
    }
}

impl TryFrom<&str> for FileDesc {
    type Error = SysError;
    fn try_from(name: &str) -> LxResult<Self> {
        let x: isize = name.parse().map_err(|_| SysError::EINVAL)?;
        Ok(FileDesc(x))
    }
}

impl Into<usize> for FileDesc {
    fn into(self) -> usize {
        self.0 as _
    }
}

impl Into<HandleValue> for FileDesc {
    fn into(self) -> HandleValue {
        self.0 as _
    }
}

pub fn create_root_fs() -> Arc<dyn INode> {
    // use RamFS as rootfs
    let rootfs = MountFS::new(RamFS::new());
    let root = rootfs.root_inode();

    // create DevFS
    let devfs = DevFS::new();
    devfs
        .add("null", Arc::new(NullINode::default()))
        .expect("failed to mknod /dev/null");
    devfs
        .add("zero", Arc::new(ZeroINode::default()))
        .expect("failed to mknod /dev/zero");
    devfs
        .add("random", Arc::new(RandomINode::new(false)))
        .expect("failed to mknod /dev/random");
    devfs
        .add("urandom", Arc::new(RandomINode::new(true)))
        .expect("failed to mknod /dev/urandom");

    // mount DevFS at /dev
    let dev = root.find(true, "dev").unwrap_or_else(|_| {
        root.create("dev", FileType::Dir, 0o666)
            .expect("failed to mkdir /dev")
    });
    dev.mount(devfs).expect("failed to mount DevFS");

    // mount RamFS at /tmp
    let ramfs = RamFS::new();
    let tmp = root.find(true, "tmp").unwrap_or_else(|_| {
        root.create("tmp", FileType::Dir, 0o666)
            .expect("failed to mkdir /tmp")
    });
    tmp.mount(ramfs).expect("failed to mount RamFS");

    root
}

pub trait INodeExt {
    fn read_as_vec(&self) -> Result<Vec<u8>>;
}

impl INodeExt for dyn INode {
    #[allow(unsafe_code)]
    fn read_as_vec(&self) -> Result<Vec<u8>> {
        let size = self.metadata()?.size;
        let mut buf = Vec::with_capacity(size);
        unsafe {
            buf.set_len(size);
        }
        self.read_at(0, buf.as_mut_slice())?;
        Ok(buf)
    }
}
