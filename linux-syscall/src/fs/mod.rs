use alloc::{sync::Arc, vec::Vec};

use rcore_fs::vfs::*;
use rcore_fs_devfs::{special::*, DevFS};
use rcore_fs_mountfs::MountFS;
use rcore_fs_ramfs::RamFS;
use rcore_fs_sfs::SimpleFileSystem;

pub use self::file::*;
//pub use self::stdio::*;
//pub use self::pseudo::*;
use self::device::MemBuf;
pub use self::random::*;
use crate::error::*;
use crate::FileDesc;
use lazy_static::lazy_static;
use zircon_object::object::{HandleValue, KernelObject};
use zircon_object::task::Process;
use zircon_object::ZxError;

mod device;
mod file;
mod ioctl;
//mod pseudo;
mod random;
//mod stdio;

pub trait FileLike: KernelObject {
    fn read(&self, buf: &mut [u8]) -> LxResult<usize>;
    fn write(&self, buf: &[u8]) -> LxResult<usize>;
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> LxResult<usize>;
    fn write_at(&self, offset: usize, buf: &[u8]) -> LxResult<usize>;
    fn poll(&self) -> LxResult<PollStatus>;
    fn ioctl(&self, request: usize, arg1: usize, arg2: usize, arg3: usize) -> LxResult<()>;
    fn fcntl(&self, cmd: usize, arg: usize) -> LxResult<()>;
}

pub trait ProcessExt {
    fn get_file_like(&self, fd: FileDesc) -> LxResult<Arc<dyn FileLike>>;
}

impl ProcessExt for Process {
    fn get_file_like(&self, fd: isize) -> LxResult<Arc<dyn FileLike>> {
        match self.get_object::<File>(fd as HandleValue) {
            Ok(file) => return Ok(file as Arc<dyn FileLike>),
            Err(ZxError::WRONG_TYPE) => {}
            Err(e) => return Err(e.into()),
        }
        unimplemented!("unknown file type")
    }
}

lazy_static! {
    /// The root of file system
    pub static ref ROOT_INODE: Arc<dyn INode> = {
        let device = Arc::new(MemBuf::new(Vec::new()));

        // use SFS as rootfs
        let sfs = SimpleFileSystem::open(device).expect("failed to open SFS");
        let rootfs = MountFS::new(sfs);
        let root = rootfs.root_inode();

        // create DevFS
        let devfs = DevFS::new();
        devfs.add("null", Arc::new(NullINode::default())).expect("failed to mknod /dev/null");
        devfs.add("zero", Arc::new(ZeroINode::default())).expect("failed to mknod /dev/zero");
        devfs.add("random", Arc::new(RandomINode::new(false))).expect("failed to mknod /dev/zero");
        devfs.add("urandom", Arc::new(RandomINode::new(true))).expect("failed to mknod /dev/zero");

        // mount DevFS at /dev
        let dev = root.find(true, "dev").unwrap_or_else(|_| {
            root.create("dev", FileType::Dir, 0o666).expect("failed to mkdir /dev")
        });
        dev.mount(devfs).expect("failed to mount DevFS");

        // mount RamFS at /tmp
        let ramfs = RamFS::new();
        let tmp = root.find(true, "tmp").unwrap_or_else(|_| {
            root.create("tmp", FileType::Dir, 0o666).expect("failed to mkdir /tmp")
        });
        tmp.mount(ramfs).expect("failed to mount RamFS");

        root
    };
}

//pub const FOLLOW_MAX_DEPTH: usize = 1;

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
