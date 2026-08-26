//! dma-buf: a shareable handle to a buffer's physical memory.
//!
//! Used by DRM PRIME (`PRIME_HANDLE_TO_FD` / `PRIME_FD_TO_HANDLE`) to pass GPU
//! buffers between DRM nodes — e.g. a buffer rendered on `/dev/dri/renderD128`
//! (Mesa llvmpipe) exported as a dma-buf fd and imported into `/dev/dri/card0`
//! for scanout. The dma-buf just carries the backing frames (a contiguous
//! `VmObject`) plus its physical address and size; the pixel layout
//! (width/height/format/pitch) travels separately via `ADDFB2`.

use super::*;
use alloc::sync::Arc;
use zircon_object::object::*;

/// A dma-buf file object.
pub struct DmaBuf {
    base: KObjectBase,
    /// Physical base address of the backing buffer.
    pub phys_addr: u64,
    /// Buffer size in bytes.
    pub size: usize,
    /// Backing frames — kept alive while the dma-buf (or any GEM handle
    /// imported from it) is referenced.
    vmo: Arc<VmObject>,
}

impl_kobject!(DmaBuf);

impl DmaBuf {
    /// Wrap a buffer's physical memory in a shareable dma-buf object.
    pub fn new(phys_addr: u64, size: usize, vmo: Arc<VmObject>) -> Arc<Self> {
        Arc::new(Self {
            base: KObjectBase::new(),
            phys_addr,
            size,
            vmo,
        })
    }

    /// The backing frames, for importing into another DRM node's GEM table.
    pub fn vmo(&self) -> Arc<VmObject> {
        self.vmo.clone()
    }
}

#[async_trait]
impl FileLike for DmaBuf {
    fn flags(&self) -> OpenFlags {
        OpenFlags::RDWR | OpenFlags::CLOEXEC
    }

    fn set_flags(&self, _f: OpenFlags) -> LxResult {
        Ok(())
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        Arc::new(Self {
            base: KObjectBase::new(),
            phys_addr: self.phys_addr,
            size: self.size,
            vmo: self.vmo.clone(),
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

    /// Mesa's software dma-buf import (`kms_sw_displaytarget_from_handle`) sizes
    /// the buffer with `lseek(fd, 0, SEEK_END)` before mmap()ing it. Report the
    /// backing size so the import succeeds; without this lseek failed (EBADF on
    /// a non-`File` fd) and eglCreateImageKHR returned EGL_BAD_ALLOC.
    fn seek(&self, pos: SeekFrom) -> LxResult<u64> {
        let offset = match pos {
            SeekFrom::Start(off) => off as i64,
            SeekFrom::End(off) => self.size as i64 + off,
            // dma-bufs are stateless here (no kept cursor); treat relative seeks
            // from a zero base, which is all Mesa needs.
            SeekFrom::Current(off) => off,
        };
        if offset < 0 {
            return Err(LxError::EINVAL);
        }
        Ok(offset as u64)
    }

    fn poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: true,
            error: false,
        })
    }

    async fn async_poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: true,
            error: false,
        })
    }

    /// mmap of the dma-buf maps the same backing frames (CPU access for the
    /// software renderer / scanout).
    fn get_vmo(&self, _offset: usize, _len: usize) -> LxResult<Arc<VmObject>> {
        Ok(self.vmo.clone())
    }
}
