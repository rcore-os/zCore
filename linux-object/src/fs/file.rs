//! File handle for process

#![allow(dead_code)]

use alloc::{boxed::Box, string::String, sync::Arc};

use super::FileLike;
use crate::error::{LxError, LxResult};
use async_trait::async_trait;
use rcore_fs::vfs::{FsError, INode, Metadata, PollStatus};
use spin::Mutex;
use zircon_object::object::*;

pub struct File {
    base: KObjectBase,
    inode: Arc<dyn INode>,
    options: OpenOptions,
    pub path: String,
    inner: Mutex<FileInner>,
}

impl_kobject!(File);

#[derive(Default)]
struct FileInner {
    offset: u64,
}

#[derive(Debug)]
pub struct OpenOptions {
    pub read: bool,
    pub write: bool,
    /// Before each write, the file offset is positioned at the end of the file.
    pub append: bool,
    pub nonblock: bool,
}

#[derive(Debug)]
pub enum SeekFrom {
    Start(u64),
    End(i64),
    Current(i64),
}

impl File {
    pub fn new(inode: Arc<dyn INode>, options: OpenOptions, path: String) -> Arc<Self> {
        Arc::new(File {
            base: KObjectBase::new(),
            inode,
            options,
            path,
            inner: Mutex::new(FileInner::default()),
        })
    }

    pub fn read(&self, buf: &mut [u8]) -> LxResult<usize> {
        let mut inner = self.inner.lock();
        let len = self.read_at(inner.offset, buf)?;
        inner.offset += len as u64;
        Ok(len)
    }

    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> LxResult<usize> {
        if !self.options.read {
            return Err(LxError::EBADF);
        }
        if !self.options.nonblock {
            // block
            loop {
                match self.inode.read_at(offset as usize, buf) {
                    Ok(read_len) => return Ok(read_len),
                    Err(FsError::Again) => {
                        //thread::yield_now();
                        //unimplemented!()
                        self.poll()?;
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        }
        let len = self.inode.read_at(offset as usize, buf)?;
        Ok(len)
    }

    pub fn write(&self, buf: &[u8]) -> LxResult<usize> {
        let mut inner = self.inner.lock();
        let offset = if self.options.append {
            self.inode.metadata()?.size as u64
        } else {
            inner.offset
        };
        let len = self.write_at(offset, buf)?;
        inner.offset = offset + len as u64;
        Ok(len)
    }

    pub fn write_at(&self, offset: u64, buf: &[u8]) -> LxResult<usize> {
        if !self.options.write {
            return Err(LxError::EBADF);
        }
        let len = self.inode.write_at(offset as usize, buf)?;
        Ok(len)
    }

    pub fn seek(&self, pos: SeekFrom) -> LxResult<u64> {
        let mut inner = self.inner.lock();
        inner.offset = match pos {
            SeekFrom::Start(offset) => offset,
            SeekFrom::End(offset) => (self.inode.metadata()?.size as i64 + offset) as u64,
            SeekFrom::Current(offset) => (inner.offset as i64 + offset) as u64,
        };
        Ok(inner.offset)
    }

    pub fn set_len(&self, len: u64) -> LxResult {
        if !self.options.write {
            return Err(LxError::EBADF);
        }
        self.inode.resize(len as usize)?;
        Ok(())
    }

    pub fn sync_all(&self) -> LxResult {
        self.inode.sync_all()?;
        Ok(())
    }

    pub fn sync_data(&self) -> LxResult {
        self.inode.sync_data()?;
        Ok(())
    }

    pub fn metadata(&self) -> LxResult<Metadata> {
        let metadata = self.inode.metadata()?;
        Ok(metadata)
    }

    pub fn lookup_follow(&self, path: &str, max_follow: usize) -> LxResult<Arc<dyn INode>> {
        let inode = self.inode.lookup_follow(path, max_follow)?;
        Ok(inode)
    }

    pub fn read_entry(&self) -> LxResult<String> {
        if !self.options.read {
            return Err(LxError::EBADF);
        }
        let mut inner = self.inner.lock();
        let name = self.inode.get_entry(inner.offset as usize)?;
        inner.offset += 1;
        Ok(name)
    }

    pub fn poll(&self) -> LxResult<PollStatus> {
        let status = self.inode.poll()?;
        Ok(status)
    }

    pub async fn async_poll(&self) -> LxResult<PollStatus> {
        Ok(self.inode.async_poll().await?)
    }

    pub fn io_control(&self, cmd: u32, arg: usize) -> LxResult<usize> {
        self.inode.io_control(cmd, arg)?;
        Ok(0)
    }

    pub fn inode(&self) -> Arc<dyn INode> {
        self.inode.clone()
    }

    pub fn fcntl(&self, cmd: usize, arg: usize) -> LxResult<usize> {
        if arg & 0x800 > 0 && cmd == 4 {
            unimplemented!()
            //            self.options.nonblock = true;
        }
        Ok(0)
    }
}

#[async_trait]
impl FileLike for File {
    fn read(&self, buf: &mut [u8]) -> LxResult<usize> {
        self.read(buf)
    }

    fn write(&self, buf: &[u8]) -> LxResult<usize> {
        self.write(buf)
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> LxResult<usize> {
        self.read_at(offset, buf)
    }

    fn write_at(&self, offset: u64, buf: &[u8]) -> LxResult<usize> {
        self.write_at(offset, buf)
    }

    fn poll(&self) -> LxResult<PollStatus> {
        self.poll()
    }

    async fn async_poll(&self) -> LxResult<PollStatus> {
        self.async_poll().await
    }

    fn ioctl(&self, request: usize, arg1: usize, _arg2: usize, _arg3: usize) -> LxResult<usize> {
        self.io_control(request as u32, arg1)
    }

    fn fcntl(&self, cmd: usize, arg: usize) -> LxResult<usize> {
        self.fcntl(cmd, arg)
    }
}
