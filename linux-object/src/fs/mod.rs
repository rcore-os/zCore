//! Linux file objects

mod devfs;
mod file;
mod ioctl;
mod pipe;
mod pseudo;
mod stdio;

pub mod rcore_fs_wrapper;

use alloc::{boxed::Box, string::ToString, sync::Arc, vec::Vec};
use core::convert::TryFrom;

use async_trait::async_trait;
use downcast_rs::impl_downcast;

use kernel_hal::drivers;
use rcore_fs::vfs::{FileSystem, FileType, INode, PollStatus, Result};
use rcore_fs_devfs::special::{NullINode, ZeroINode};
use rcore_fs_devfs::DevFS;
use rcore_fs_mountfs::MountFS;
use rcore_fs_ramfs::RamFS;
use zircon_object::{object::KernelObject, vm::VmObject};

use self::{devfs::RandomINode, pseudo::Pseudo};
use crate::error::{LxError, LxResult};
use crate::process::LinuxProcess;

pub use file::{File, OpenFlags, SeekFrom};
pub use pipe::Pipe;
pub use rcore_fs::vfs;
pub use stdio::{STDIN, STDOUT};

#[async_trait]
/// Generic file interface
///
/// - Normal file, Directory
/// - Socket
/// - Epoll instance
pub trait FileLike: KernelObject {
    /// Returns open flags.
    fn flags(&self) -> OpenFlags;
    /// Set open flags.
    fn set_flags(&self, f: OpenFlags) -> LxResult;
    /// Duplicate the file.
    fn dup(&self) -> Arc<dyn FileLike>;
    /// read to buffer
    async fn read(&self, buf: &mut [u8]) -> LxResult<usize>;
    /// write from buffer
    fn write(&self, buf: &[u8]) -> LxResult<usize>;
    /// read to buffer at given offset
    async fn read_at(&self, offset: u64, buf: &mut [u8]) -> LxResult<usize>;
    /// write from buffer at given offset
    fn write_at(&self, offset: u64, buf: &[u8]) -> LxResult<usize>;
    /// wait for some event on a file descriptor
    fn poll(&self) -> LxResult<PollStatus>;
    /// wait for some event on a file descriptor use async
    async fn async_poll(&self) -> LxResult<PollStatus>;
    /// manipulates the underlying device parameters of special files
    fn ioctl(&self, request: usize, arg1: usize, arg2: usize, arg3: usize) -> LxResult<usize>;
    /// Returns the [`VmObject`] representing the file with given `offset` and `len`.
    fn get_vmo(&self, offset: usize, len: usize) -> LxResult<Arc<VmObject>>;
}

impl_downcast!(sync FileLike);

/// file descriptor wrapper
#[derive(Debug, Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash)]
pub struct FileDesc(i32);

impl FileDesc {
    /// Pathname is interpreted relative to the current working directory(CWD)
    pub const CWD: Self = FileDesc(-100);
}

impl From<usize> for FileDesc {
    fn from(x: usize) -> Self {
        FileDesc(x as i32)
    }
}

impl From<i32> for FileDesc {
    fn from(x: i32) -> Self {
        FileDesc(x)
    }
}

impl TryFrom<&str> for FileDesc {
    type Error = LxError;
    fn try_from(name: &str) -> LxResult<Self> {
        let x: i32 = name.parse().map_err(|_| LxError::EINVAL)?;
        Ok(FileDesc(x))
    }
}

impl From<FileDesc> for usize {
    fn from(f: FileDesc) -> Self {
        f.0 as _
    }
}

impl From<FileDesc> for i32 {
    fn from(f: FileDesc) -> Self {
        f.0
    }
}

/// create root filesystem, mount DevFS and RamFS
pub fn create_root_fs(rootfs: Arc<dyn FileSystem>) -> Arc<dyn INode> {
    let rootfs = MountFS::new(rootfs);
    let root = rootfs.mountpoint_root_inode();

    // create DevFS
    let devfs = DevFS::new();
    let devfs_root = devfs.root();
    devfs_root
        .add("null", Arc::new(NullINode::new()))
        .expect("failed to mknod /dev/null");
    devfs_root
        .add("zero", Arc::new(ZeroINode::new()))
        .expect("failed to mknod /dev/zero");
    devfs_root
        .add("random", Arc::new(RandomINode::new(false)))
        .expect("failed to mknod /dev/random");
    devfs_root
        .add("urandom", Arc::new(RandomINode::new(true)))
        .expect("failed to mknod /dev/urandom");

    if let Some(display) = drivers::all_display().first() {
        use self::devfs::{EventDev, FbDev, MiceDev};

        // Add framebuffer device at `/dev/fb0`
        if let Err(e) = devfs_root.add("fb0", Arc::new(FbDev::new(display.clone()))) {
            warn!("failed to mknod /dev/fb0: {:?}", e);
        }

        let input_dev = devfs_root
            .add_dir("input")
            .expect("failed to mkdir /dev/input");

        // Add mouse devices at `/dev/input/mouseX` and `/dev/input/mice`
        for (id, m) in MiceDev::from_input_devices(&drivers::all_input().as_vec()) {
            let fname = id.map_or("mice".to_string(), |id| format!("mouse{}", id));
            if let Err(e) = input_dev.add(&fname, Arc::new(m)) {
                warn!("failed to mknod /dev/input/{}: {:?}", &fname, e);
            }
        }

        // Add input event devices at `/dev/input/eventX`
        for (id, i) in drivers::all_input().as_vec().iter().enumerate() {
            let fname = format!("event{}", id);
            if let Err(e) = input_dev.add(&fname, Arc::new(EventDev::new(i.clone(), id))) {
                warn!("failed to mknod /dev/input/{}: {:?}", &fname, e);
            }
        }
    }

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

/// extension for INode
pub trait INodeExt {
    /// similar to read, but return a u8 vector
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

impl LinuxProcess {
    /// Lookup INode from the process.
    ///
    /// - If `path` is relative, then it is interpreted relative to the directory
    ///   referred to by the file descriptor `dirfd`.
    ///
    /// - If the `dirfd` is the special value `AT_FDCWD`, then the directory is
    ///   current working directory of the process.
    ///
    /// - If `path` is absolute, then `dirfd` is ignored.
    ///
    /// - If `follow` is true, then dereference `path` if it is a symbolic link.
    pub fn lookup_inode_at(
        &self,
        dirfd: FileDesc,
        path: &str,
        follow: bool,
    ) -> LxResult<Arc<dyn INode>> {
        debug!(
            "lookup_inode_at: dirfd: {:?}, cwd: {:?}, path: {:?}, follow: {:?}",
            dirfd,
            self.current_working_directory(),
            path,
            follow
        );
        // hard code special path
        if path == "/proc/self/exe" {
            return Ok(Arc::new(Pseudo::new(
                &self.execute_path(),
                FileType::SymLink,
            )));
        }
        let (fd_dir_path, fd_name) = split_path(path);
        if fd_dir_path == "/proc/self/fd" {
            let fd = FileDesc::try_from(fd_name)?;
            let file = self.get_file(fd)?;
            return Ok(Arc::new(Pseudo::new(file.path(), FileType::SymLink)));
        }

        let follow_max_depth = if follow { FOLLOW_MAX_DEPTH } else { 0 };
        if dirfd == FileDesc::CWD {
            Ok(self
                .root_inode()
                .lookup(&self.current_working_directory())?
                .lookup_follow(path, follow_max_depth)?)
        } else {
            let file = self.get_file(dirfd)?;
            Ok(file.lookup_follow(path, follow_max_depth)?)
        }
    }

    /// Lookup INode from the process.
    ///
    /// see `lookup_inode_at`
    pub fn lookup_inode(&self, path: &str) -> LxResult<Arc<dyn INode>> {
        self.lookup_inode_at(FileDesc::CWD, path, true)
    }
}

/// Split a `path` str to `(base_path, file_name)`
pub fn split_path(path: &str) -> (&str, &str) {
    let mut split = path.trim_end_matches('/').rsplitn(2, '/');
    let file_name = split.next().unwrap();
    let mut dir_path = split.next().unwrap_or(".");
    if dir_path.is_empty() {
        dir_path = "/";
    }
    (dir_path, file_name)
}

/// the max depth for following a link
const FOLLOW_MAX_DEPTH: usize = 1;
