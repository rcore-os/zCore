use alloc::{sync::Arc, vec::Vec};

use rcore_fs::vfs::*;
use rcore_fs_devfs::{special::*, DevFS};
use rcore_fs_mountfs::MountFS;
use rcore_fs_ramfs::RamFS;

pub use self::file::*;
pub use self::pseudo::*;
pub use self::random::*;
pub use self::stdio::*;
pub use rcore_fs::vfs;

use crate::error::*;
use crate::process::LinuxProcess;
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
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> LxResult<usize>;
    fn write_at(&self, offset: u64, buf: &[u8]) -> LxResult<usize>;
    fn poll(&self) -> LxResult<PollStatus>;
    fn ioctl(&self, request: usize, arg1: usize, arg2: usize, arg3: usize) -> LxResult<usize>;
    fn fcntl(&self, cmd: usize, arg: usize) -> LxResult<usize>;
}

impl_downcast!(sync FileLike);

#[derive(Debug, Copy, Clone, Ord, PartialOrd, Eq, PartialEq)]
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

impl TryFrom<&str> for FileDesc {
    type Error = SysError;
    fn try_from(name: &str) -> LxResult<Self> {
        let x: i32 = name.parse().map_err(|_| SysError::EINVAL)?;
        Ok(FileDesc(x))
    }
}

impl Into<usize> for FileDesc {
    fn into(self) -> usize {
        self.0 as _
    }
}

pub fn create_root_fs(rootfs: Arc<dyn FileSystem>) -> Arc<dyn INode> {
    let rootfs = MountFS::new(rootfs);
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
            dirfd, self.cwd, path, follow
        );
        // hard code special path
        if path == "/proc/self/exe" {
            return Ok(Arc::new(Pseudo::new(&self.exec_path, FileType::SymLink)));
        }
        let (fd_dir_path, fd_name) = split_path(&path);
        if fd_dir_path == "/proc/self/fd" {
            let fd = FileDesc::try_from(fd_name)?;
            let fd_path = &self.get_file(fd)?.path;
            return Ok(Arc::new(Pseudo::new(fd_path, FileType::SymLink)));
        }

        let follow_max_depth = if follow { FOLLOW_MAX_DEPTH } else { 0 };
        if dirfd == FileDesc::CWD {
            Ok(self
                .root_inode()
                .lookup(&self.cwd)?
                .lookup_follow(path, follow_max_depth)?)
        } else {
            let file = self.get_file(dirfd)?;
            Ok(file.lookup_follow(path, follow_max_depth)?)
        }
    }

    pub fn lookup_inode(&self, path: &str) -> LxResult<Arc<dyn INode>> {
        self.lookup_inode_at(FileDesc::CWD, path, true)
    }
}

/// Split a `path` str to `(base_path, file_name)`
pub fn split_path(path: &str) -> (&str, &str) {
    let mut split = path.trim_end_matches('/').rsplitn(2, '/');
    let file_name = split.next().unwrap();
    let mut dir_path = split.next().unwrap_or(".");
    if dir_path == "" {
        dir_path = "/";
    }
    (dir_path, file_name)
}

const FOLLOW_MAX_DEPTH: usize = 1;
