//! A file object wrapping a DRM syncobj handle, for `SYNCOBJ_HANDLE_TO_FD`/
//! `SYNCOBJ_FD_TO_HANDLE` -- lets a syncobj cross a `fork`/`exec` or be
//! passed between processes (e.g. `SCM_RIGHTS` on a Unix socket), the same
//! way [`DmaBuf`](super::DmaBuf) does for PRIME GEM buffers.
//!
//! The syncobj table itself (`zcore_drivers::scheme::syncobj`) is a single
//! GLOBAL handle space, not per-process (see that module's own doc) -- so
//! unlike a real dma-buf, which carries actual backing memory, "export"
//! here doesn't move or copy any state: the handle number is already
//! globally valid, and this file just carries it across the fd boundary.
//! "Import" hands back that same handle number. Closing the exported fd
//! does NOT destroy the underlying syncobj -- `SYNCOBJ_DESTROY` is still
//! the only way to remove one, consistent with the syncobj table having no
//! refcounting at all (a real DRM syncobj fd keeps the kernel object alive
//! past a `SYNCOBJ_DESTROY` on the handle that exported it; here, once the
//! handle is destroyed, an already-exported fd holds a stale handle number
//! that reads back the same "unknown handle" errors `WAIT`/`SIGNAL`/`QUERY`
//! already give for any other bad handle -- not a crash, just a known,
//! documented simplification).

use super::*;
use zircon_object::object::*;

/// A file object carrying a syncobj handle number across the fd boundary.
pub struct SyncobjHandle {
    base: KObjectBase,
    pub handle: u32,
    /// `None` for a syncobj fd (`HANDLE_TO_FD`): the fd names the object
    /// itself, and importing it hands the same handle back.
    ///
    /// `Some(point)` for a **`sync_file`** fd
    /// (`HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE`): the fd names a FENCE that was
    /// current on `handle` at export time, i.e. "`handle` reaching `point`".
    /// Importing one (`FD_TO_HANDLE_FLAGS_IMPORT_SYNC_FILE`) gives that fence
    /// to a DIFFERENT syncobj, which is how Mesa hands a client's completed
    /// work to the X server and back — the one thing the two fd kinds must
    /// not confuse, since importing a sync_file as if it were a syncobj would
    /// alias the two objects instead of copying one fence between them.
    pub sync_file_point: Option<u64>,
}

impl_kobject!(SyncobjHandle);

impl SyncobjHandle {
    pub fn new(handle: u32) -> Arc<Self> {
        Arc::new(Self {
            base: KObjectBase::new(),
            handle,
            sync_file_point: None,
        })
    }

    /// A `sync_file` fd: the fence "`handle` reaches `point`".
    pub fn new_sync_file(handle: u32, point: u64) -> Arc<Self> {
        Arc::new(Self {
            base: KObjectBase::new(),
            handle,
            sync_file_point: Some(point),
        })
    }
}

#[async_trait]
impl FileLike for SyncobjHandle {
    fn flags(&self) -> OpenFlags {
        OpenFlags::RDWR | OpenFlags::CLOEXEC
    }

    fn set_flags(&self, _f: OpenFlags) -> LxResult {
        Ok(())
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        Arc::new(Self {
            base: KObjectBase::new(),
            handle: self.handle,
            sync_file_point: self.sync_file_point,
        })
    }

    async fn read(&self, _buf: &mut [u8]) -> LxResult<usize> {
        Err(LxError::ENOSYS)
    }

    fn write(&self, _buf: &[u8]) -> LxResult<usize> {
        Err(LxError::ENOSYS)
    }

    async fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> LxResult<usize> {
        Err(LxError::ENOSYS)
    }

    /// Real userspace only ever `SYNCOBJ_FD_TO_HANDLE`s this immediately
    /// after receiving it -- never reads/polls it directly -- so "never
    /// ready" is an honest default, not a real gap.
    fn poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        Ok(PollStatus {
            read: false,
            write: false,
            error: false,
        })
    }

    async fn async_poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        Ok(PollStatus {
            read: false,
            write: false,
            error: false,
        })
    }
}
