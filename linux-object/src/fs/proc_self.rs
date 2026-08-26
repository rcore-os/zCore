use crate::fs::pseudo::Pseudo;
use crate::process::ProcessExt;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use rcore_fs::vfs::*;
use zircon_object::object::KernelObject;
use zircon_object::task::Process;

pub struct ProcSelfFdDir {
    pub process: Arc<Process>,
}

impl INode for ProcSelfFdDir {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
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
            dev: 0,
            inode: 40 + self.process.id() as usize,
            size: 0,
            blk_size: 0,
            blocks: 0,
            atime: Timespec { sec: 0, nsec: 0 },
            mtime: Timespec { sec: 0, nsec: 0 },
            ctime: Timespec { sec: 0, nsec: 0 },
            type_: FileType::Dir,
            mode: 0o555,
            nlinks: 2,
            uid: 0,
            gid: 0,
            rdev: 0,
        })
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(crate::fs::procfs::ProcFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." | ".." => Ok(Arc::new(Pseudo::new("/proc/self/fd", FileType::Dir))),
            _ => {
                let fd = name.parse::<i32>().map_err(|_| FsError::EntryNotFound)?;
                let file = self
                    .process
                    .try_linux()
                    .ok_or(FsError::EntryNotFound)?
                    .get_file(fd.into())
                    .map_err(|_| FsError::EntryNotFound)?;
                Ok(Arc::new(Pseudo::new(file.path(), FileType::SymLink)))
            }
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        let files = self
            .process
            .try_linux()
            .ok_or(FsError::DeviceError)?
            .get_files()
            .map_err(|_| FsError::DeviceError)?;
        let mut keys: Vec<_> = files.keys().collect();
        keys.sort();
        if id < keys.len() {
            let fd: i32 = (*keys[id]).into();
            Ok(fd.to_string())
        } else {
            Err(FsError::EntryNotFound)
        }
    }
}
