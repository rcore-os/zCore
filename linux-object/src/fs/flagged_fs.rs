//! Filesystem wrapper that enforces per-mount flags (e.g. read-only).

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::any::Any;
use core::future::Future;
use core::pin::Pin;

use delegate::delegate;
use rcore_fs::vfs::{
    FileSystem, FileType, FsError, FsInfo, INode, MMapArea, Metadata, PollStatus, Result,
};

use super::mount_state::MountState;

fn ro_err() -> FsError {
    FsError::ReadOnly
}

pub fn wrap_fs(inner: Arc<dyn FileSystem>, state: Arc<MountState>) -> Arc<dyn FileSystem> {
    Arc::new(FlaggedFs { inner, state })
}

struct FlaggedFs {
    inner: Arc<dyn FileSystem>,
    state: Arc<MountState>,
}

impl FileSystem for FlaggedFs {
    // Pure pass-throughs to the wrapped filesystem.
    delegate! {
        to self.inner {
            fn sync(&self) -> Result<()>;
            fn info(&self) -> FsInfo;
        }
    }

    // root_inode must re-wrap so the flags propagate to the returned inode.
    fn root_inode(&self) -> Arc<dyn INode> {
        Arc::new(FlaggedINode {
            inner: self.inner.root_inode(),
            state: self.state.clone(),
        })
    }
}

struct FlaggedINode {
    inner: Arc<dyn INode>,
    state: Arc<MountState>,
}

impl FlaggedINode {
    fn check_write(&self) -> Result<()> {
        if self.state.is_read_only() {
            return Err(ro_err());
        }
        Ok(())
    }

    fn wrap(&self, inode: Arc<dyn INode>) -> Arc<dyn INode> {
        Arc::new(FlaggedINode {
            inner: inode,
            state: self.state.clone(),
        })
    }
}

impl INode for FlaggedINode {
    // Read-only / metadata operations forward straight to the inner inode.
    delegate! {
        to self.inner {
            fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize>;
            fn metadata(&self) -> Result<Metadata>;
            fn sync_all(&self) -> Result<()>;
            fn sync_data(&self) -> Result<()>;
            fn get_entry(&self, id: usize) -> Result<alloc::string::String>;
            fn get_entry_with_metadata(
                &self,
                id: usize,
            ) -> Result<(Metadata, alloc::string::String)>;
            fn io_control(&self, cmd: u32, data: usize) -> Result<usize>;
            fn mmap(&self, area: MMapArea) -> Result<()>;
            fn fs(&self) -> Arc<dyn FileSystem>;
        }
    }

    // `async_poll` ties the returned future's lifetime to `&self`; kept explicit
    // rather than delegated so the borrow plumbing stays obvious.
    fn async_poll<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<PollStatus>> + Send + Sync + 'a>> {
        self.inner.async_poll()
    }

    // Mutating operations are gated on the read-only flag.
    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize> {
        self.check_write()?;
        self.inner.write_at(offset, buf)
    }

    fn poll(&self) -> Result<PollStatus> {
        let mut status = self.inner.poll()?;
        if self.state.is_read_only() {
            status.write = false;
        }
        Ok(status)
    }

    fn set_metadata(&self, metadata: &Metadata) -> Result<()> {
        self.check_write()?;
        self.inner.set_metadata(metadata)
    }

    fn resize(&self, len: usize) -> Result<()> {
        self.check_write()?;
        self.inner.resize(len)
    }

    fn create(&self, name: &str, type_: FileType, mode: u32) -> Result<Arc<dyn INode>> {
        self.check_write()?;
        Ok(self.wrap(self.inner.create(name, type_, mode)?))
    }

    fn create2(
        &self,
        name: &str,
        type_: FileType,
        mode: u32,
        data: usize,
    ) -> Result<Arc<dyn INode>> {
        self.check_write()?;
        Ok(self.wrap(self.inner.create2(name, type_, mode, data)?))
    }

    fn link(&self, name: &str, other: &Arc<dyn INode>) -> Result<()> {
        self.check_write()?;
        self.inner.link(name, other)
    }

    fn unlink(&self, name: &str) -> Result<()> {
        self.check_write()?;
        self.inner.unlink(name)
    }

    fn move_(&self, old_name: &str, target: &Arc<dyn INode>, new_name: &str) -> Result<()> {
        self.check_write()?;
        self.inner.move_(old_name, target, new_name)
    }

    // find re-wraps so the returned child also carries the mount flags.
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        Ok(self.wrap(self.inner.find(name)?))
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}
