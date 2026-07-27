//! File handle for process

use alloc::{boxed::Box, string::String, sync::Arc};

use async_trait::async_trait;
use lock::RwLock;

use rcore_fs::vfs::{FileType, FsError, INode, Metadata, PollStatus, Timespec};
use zircon_object::object::*;
use zircon_object::vm::{pages, VmObject};

use super::FileLike;
use crate::error::{LxError, LxResult};

bitflags::bitflags! {
    /// File open flags
    pub struct OpenFlags: usize {
        /// read only
        const RDONLY = 0;
        /// write only
        const WRONLY = 1;
        /// read write
        const RDWR = 2;
        /// create file if it does not exist
        const CREATE = 1 << 6;
        /// error if CREATE and the file exists
        const EXCLUSIVE = 1 << 7;
        /// truncate file upon open
        const TRUNCATE = 1 << 9;
        /// append on each write
        const APPEND = 1 << 10;
        /// non block open
        const NON_BLOCK = 1 << 11;
        /// close on exec
        const CLOEXEC = 1 << 19;
    }
}

impl OpenFlags {
    /// check if the OpenFlags is readable
    pub fn readable(self) -> bool {
        let b = self.bits() & 0b11;
        b == Self::RDONLY.bits() || b == Self::RDWR.bits()
    }
    /// check if the OpenFlags is writable
    pub fn writable(self) -> bool {
        let b = self.bits() & 0b11;
        b == Self::WRONLY.bits() || b == Self::RDWR.bits()
    }
    /// check if the OpenFlags caontains append
    pub fn is_append(self) -> bool {
        self.contains(Self::APPEND)
    }
    /// check if the OpenFlags caontains non-block
    pub fn non_block(self) -> bool {
        self.contains(Self::NON_BLOCK)
    }
    /// close on exec
    pub fn close_on_exec(self) -> bool {
        self.contains(Self::CLOEXEC)
    }
}

bitflags::bitflags! {
    pub struct PollEvents: u16 {
        /// There is data to read.
        const IN = 0x0001;
        /// Writing is now possible.
        const OUT = 0x0004;
        /// Error condition (return only)
        const ERR = 0x0008;
        /// Hang up (return only)
        const HUP = 0x0010;
        /// Invalid request: fd not open (return only)
        const INVAL = 0x0020;
    }
}

/// file seek type
#[derive(Debug)]
pub enum SeekFrom {
    /// seek from start point
    Start(u64),
    /// seek from end
    End(i64),
    /// seek from current
    Current(i64),
}

/// file inner mut data struct
#[derive(Clone)]
struct FileInner {
    /// content offset on read/write
    offset: u64,
    /// file open options
    flags: OpenFlags,
    /// file INode
    inode: Arc<dyn INode>,
}

/// file implement struct
pub struct File {
    /// object base
    base: KObjectBase,
    /// file path
    path: String,
    /// file inner mut data
    inner: RwLock<FileInner>,
}

impl_kobject!(File);

/// Demand-paging source for a file-backed `mmap` (see [`get_vmo`]).
///
/// Reads one page from the backing inode the first time that page is touched,
/// so a large mapping (e.g. `libLLVM.so`) is paged in lazily instead of being
/// read into memory in full at map time.
///
/// [`get_vmo`]: File::get_vmo
struct FileFrameFiller {
    inode: Arc<dyn INode>,
    /// File offset that VMO offset 0 maps to.
    file_offset: usize,
    /// Number of readable bytes from `file_offset` within the mapping. Pages
    /// past this are left zero (the BSS tail of the mapping).
    source_len: usize,
}

/// Per-inode `MAP_SHARED` VMO registry: inode number → (shared VMO, a weak
/// handle to the inode used to prune the entry once the file is fully closed).
type SharedVmoMap =
    alloc::collections::BTreeMap<usize, (Arc<VmObject>, alloc::sync::Weak<dyn INode>)>;

lazy_static::lazy_static! {
    /// Per-inode shared VMOs for `MAP_SHARED` file mappings, keyed by the
    /// inode's Arc data pointer. Every mapper of the same file gets the SAME
    /// VmObject, so stores by one process are visible to all others — the
    /// wl_shm contract. Without this each mmap produced an independent
    /// demand-paged snapshot: foot rendered its terminal into its own copy
    /// while labwc composited an all-zeros copy — every client window and the
    /// lunarbg background showed pure black.
    ///
    /// The VMO is held with a STRONG ref, anchored to the *inode's* lifetime
    /// (a Weak<INode> alongside it), NOT to any one mapping's. This is what
    /// makes the wl_keyboard keymap work: wlroots writes the keymap into a
    /// memfd via mmap(MAP_SHARED)+memcpy and then *munmaps and closes the write
    /// fd* before sending the read fd — with a Weak VMO the shared pages would
    /// be freed on that munmap, so the client's later read (and the MAP_PRIVATE
    /// COW off this VMO) would see zeros and foot would SIGSEGV on the empty
    /// keymap. There is no MAP_SHARED->inode writeback path (msync is a no-op),
    /// so the shared VMO *is* the file's storage for as long as an fd keeps the
    /// inode alive. Dead-inode entries are pruned on every access, freeing the
    /// VMO (and its frames) once the last fd closes.
    static ref SHARED_FILE_VMOS: lock::Mutex<SharedVmoMap> =
        lock::Mutex::new(alloc::collections::BTreeMap::new());
}

/// Drop shared-VMO entries whose backing inode has been freed (all fds closed).
/// Called under the registry lock before any lookup/insert.
fn prune_shared_vmos(
    registry: &mut alloc::collections::BTreeMap<
        usize,
        (Arc<VmObject>, alloc::sync::Weak<dyn INode>),
    >,
) {
    registry.retain(|_, (_, inode_weak)| inode_weak.strong_count() > 0);
}

impl zircon_object::vm::FrameFiller for FileFrameFiller {
    fn source_len(&self) -> usize {
        self.source_len
    }

    fn fill_page(&self, offset: usize, buf: &mut [u8]) {
        if offset >= self.source_len {
            return;
        }
        let want = (self.source_len - offset).min(buf.len());
        let file_pos = self.file_offset + offset;
        let mut done = 0;
        while done < want {
            match self.inode.read_at(file_pos + done, &mut buf[done..want]) {
                Ok(0) => break,
                Ok(n) => done += n,
                // A read error mid-mapping leaves the rest zero-filled; the
                // faulting access proceeds rather than wedging the kernel.
                Err(_) => break,
            }
        }
    }
}

/// Demand-paging source that reads from another VMO instead of the inode.
///
/// Used for a `MAP_PRIVATE` file mapping when the file already has a live
/// `MAP_SHARED` VMO: writes made through the shared mapping live only in that
/// VMO and never reach `inode.read_at`, so a private reader must copy-on-write
/// from the shared VMO or it sees stale/zero bytes. This is exactly the
/// wl_keyboard keymap path — the compositor writes the keymap into the memfd
/// via `MAP_SHARED` (mmap+memcpy), then the client mmaps the same fd
/// `MAP_PRIVATE, PROT_READ`; without this the client reads all zeros,
/// xkbcommon reports "[XKB-822] Failed to parse input xkb string", and foot
/// (and every xkbcommon client) then SIGSEGVs on the NULL keymap.
struct VmoFrameFiller {
    src: Arc<VmObject>,
    /// File offset (into `src`) that this mapping's VMO offset 0 corresponds to.
    base_offset: usize,
    source_len: usize,
}

impl zircon_object::vm::FrameFiller for VmoFrameFiller {
    fn source_len(&self) -> usize {
        self.source_len
    }

    fn fill_page(&self, offset: usize, buf: &mut [u8]) {
        if offset >= self.source_len {
            return;
        }
        let want = (self.source_len - offset).min(buf.len());
        // Best-effort: a short/failed read leaves the tail zero, matching the
        // inode path.
        let _ = self.src.read(self.base_offset + offset, &mut buf[..want]);
    }
}

impl FileInner {
    /// write to file
    fn write(&mut self, buf: &[u8]) -> LxResult<usize> {
        let offset = if self.flags.is_append() {
            self.inode.metadata()?.size as u64
        } else {
            self.offset
        };
        let len = self.write_at(offset, buf)?;
        self.offset = offset + len as u64;
        Ok(len)
    }

    /// write to file at given offset
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> LxResult<usize> {
        if !self.flags.writable() {
            return Err(LxError::EBADF);
        }
        let len = self.inode.write_at(offset as usize, buf)?;
        Ok(len)
    }
}

impl File {
    /// create a file struct
    pub fn new(inode: Arc<dyn INode>, flags: OpenFlags, path: String) -> Arc<Self> {
        Arc::new(File {
            base: KObjectBase::new(),
            path,
            inner: RwLock::new(FileInner {
                offset: 0,
                flags,
                inode,
            }),
        })
    }

    /// Returns the file path.
    pub fn path(&self) -> &String {
        &self.path
    }

    /// seek from given type and offset
    pub fn seek(&self, pos: SeekFrom) -> LxResult<u64> {
        let mut inner = self.inner.write();
        // Compute the new offset with checked arithmetic and reject results
        // that would be negative; otherwise a negative relative seek would wrap
        // to a huge `u64` and let later reads/writes use an out-of-range offset.
        let new_offset: i64 = match pos {
            SeekFrom::Start(offset) => offset as i64,
            SeekFrom::End(offset) => (inner.inode.metadata()?.size as i64)
                .checked_add(offset)
                .ok_or(LxError::EINVAL)?,
            SeekFrom::Current(offset) => (inner.offset as i64)
                .checked_add(offset)
                .ok_or(LxError::EINVAL)?,
        };
        if new_offset < 0 {
            return Err(LxError::EINVAL);
        }
        inner.offset = new_offset as u64;
        Ok(inner.offset)
    }

    /// resize the file
    pub fn set_len(&self, len: u64) -> LxResult {
        let inner = self.inner.write();
        if !inner.flags.writable() {
            return Err(LxError::EBADF);
        }
        inner.inode.resize(len as usize)?;
        Ok(())
    }

    /// Sync all data and metadata
    pub fn sync_all(&self) -> LxResult {
        self.inner.read().inode.sync_all()?;
        Ok(())
    }

    /// Sync data (not include metadata)
    pub fn sync_data(&self) -> LxResult {
        self.inner.read().inode.sync_data()?;
        Ok(())
    }

    /// get metadata of file
    /// fstat
    pub fn metadata(&self) -> LxResult<Metadata> {
        Ok(self.inner.read().inode.metadata()?)
    }

    /// lookup the file following the link
    pub fn lookup_follow(&self, path: &str, max_follow: usize) -> LxResult<Arc<dyn INode>> {
        Ok(self.inner.read().inode.lookup_follow(path, max_follow)?)
    }

    /// get the name of dir entry
    pub fn read_entry(&self) -> LxResult<String> {
        Ok(self.read_entry_with_metadata()?.1)
    }

    /// get the next directory entry and its metadata
    pub fn read_entry_with_metadata(&self) -> LxResult<(Metadata, String)> {
        let mut inner = self.inner.write();
        if !inner.flags.readable() {
            return Err(LxError::EBADF);
        }
        let offset = inner.offset as usize;
        match inner.inode.get_entry_with_metadata(offset) {
            Ok(entry) => {
                inner.offset += 1;
                Ok(entry)
            }
            Err(e) => {
                // `get_entry_with_metadata`'s default implementation resolves
                // the entry's metadata via `find(name)`, which can fail even
                // though the entry exists — e.g. the devfs root's ".." has no
                // parent, so `find("..")` returns EntryNotFound. Treating that
                // as end-of-directory truncates the listing (this made
                // `ls /dev` appear empty). Distinguish the two: if the name
                // still resolves, emit it with a synthetic directory metadata;
                // only a missing name means we reached the end.
                let name = inner.inode.get_entry(offset).map_err(|_| e)?;
                inner.offset += 1;
                let meta = Metadata {
                    dev: 0,
                    inode: 0,
                    size: 0,
                    blk_size: 0,
                    blocks: 0,
                    atime: Timespec { sec: 0, nsec: 0 },
                    mtime: Timespec { sec: 0, nsec: 0 },
                    ctime: Timespec { sec: 0, nsec: 0 },
                    type_: FileType::Dir,
                    mode: 0,
                    nlinks: 1,
                    uid: 0,
                    gid: 0,
                    rdev: 0,
                };
                Ok((meta, name))
            }
        }
    }

    /// get INode of this file
    pub fn inode(&self) -> Arc<dyn INode> {
        self.inner.read().inode.clone()
    }
}

#[async_trait]
impl FileLike for File {
    fn flags(&self) -> OpenFlags {
        self.inner.read().flags
    }

    fn set_flags(&self, f: OpenFlags) -> LxResult {
        let flags = &mut self.inner.write().flags;
        flags.set(OpenFlags::APPEND, f.contains(OpenFlags::APPEND));
        flags.set(OpenFlags::NON_BLOCK, f.contains(OpenFlags::NON_BLOCK));
        flags.set(OpenFlags::CLOEXEC, f.contains(OpenFlags::CLOEXEC));
        Ok(())
    }

    fn dup(&self) -> Arc<dyn FileLike> {
        Arc::new(Self {
            base: KObjectBase::new(),
            path: self.path.clone(),
            inner: RwLock::new(self.inner.read().clone()),
        })
    }

    fn seek(&self, pos: SeekFrom) -> LxResult<u64> {
        // Delegate to the inherent offset-tracking seek (UFCS avoids recursing
        // into this trait method).
        File::seek(self, pos)
    }

    async fn read(&self, buf: &mut [u8]) -> LxResult<usize> {
        let (offset, flags, inode) = {
            let inner = self.inner.read();
            (inner.offset, inner.flags, inner.inode.clone())
        };

        if !flags.readable() {
            return Err(LxError::EBADF);
        }

        let len = if !flags.non_block() {
            // block
            loop {
                match inode.read_at(offset as usize, buf) {
                    Ok(read_len) => break read_len,
                    Err(FsError::Again) => {
                        inode.async_poll().await?;
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        } else {
            inode.read_at(offset as usize, buf)?
        };

        let mut inner = self.inner.write();
        inner.offset += len as u64;
        Ok(len)
    }

    fn write(&self, buf: &[u8]) -> LxResult<usize> {
        self.inner.write().write(buf)
    }

    async fn read_at(&self, offset: u64, buf: &mut [u8]) -> LxResult<usize> {
        let (flags, inode) = {
            let inner = self.inner.read();
            (inner.flags, inner.inode.clone())
        };

        if !flags.readable() {
            return Err(LxError::EBADF);
        }

        if !flags.non_block() {
            // block
            loop {
                match inode.read_at(offset as usize, buf) {
                    Ok(read_len) => return Ok(read_len),
                    Err(FsError::Again) => {
                        inode.async_poll().await?;
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        }
        let len = inode.read_at(offset as usize, buf)?;
        Ok(len)
    }

    fn write_at(&self, offset: u64, buf: &[u8]) -> LxResult<usize> {
        self.inner.write().write_at(offset, buf)
    }

    fn poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        let inner = self.inner.read();
        // A FIFO node opened from the fs has no pipe-buffer / writer tracking
        // here, so a reader polling an empty FIFO (e.g. a control FIFO that
        // never gets a writer) would spin: the node reads as an
        // empty regular file (0 bytes = EOF) yet polls "readable". Treat it as
        // readable only when it actually holds bytes, so the reader blocks
        // instead of busy-looping on repeated 0-byte reads.
        //
        // Use metadata() best-effort: some fds (sockets, special devices) don't
        // implement it and return ENOSYS — those must fall through to the inode's
        // own poll(), NOT propagate the error (that regressed `poll()` on packet
        // sockets, e.g. udhcpc, to "Function not implemented").
        if let Ok(meta) = inner.inode.metadata() {
            if meta.type_ == FileType::NamedPipe {
                return Ok(PollStatus {
                    read: meta.size > 0,
                    write: true,
                    error: false,
                });
            }
        }
        Ok(inner.inode.poll()?)
    }

    async fn async_poll(&self, _events: PollEvents) -> LxResult<PollStatus> {
        let inode = self.inner.read().inode.clone();

        // Special-case DrmDev: since INode::async_poll doesn't accept the requested
        // PollEvents, its default implementation blocks on Event::READABLE even if
        // the caller only polled for writability (which is always ready on DRM).
        // Intercept it here and return immediately if the requested events are ready.
        use super::devfs::DrmDev;
        if let Some(drmdev) = inode.downcast_ref::<DrmDev>() {
            let want_read = _events.contains(PollEvents::IN);
            let want_write = _events.contains(PollEvents::OUT);
            let bus = super::devfs::drm::get_eventbus();
            loop {
                let status = drmdev.poll()?;
                let ready = (want_read && status.read)
                    || (want_write && status.write)
                    || (!want_read && !want_write);
                if ready {
                    return Ok(status);
                }
                crate::sync::wait_for_event(bus.clone(), crate::sync::Event::READABLE).await;
            }
        }

        // See `poll`: special-case an empty FIFO so the reader blocks, but only
        // when metadata() is available — sockets/special devices return ENOSYS
        // and must fall through to the inode's own async_poll() rather than
        // failing the whole poll() syscall.
        if let Ok(meta) = inode.metadata() {
            if meta.type_ == FileType::NamedPipe {
                return Ok(PollStatus {
                    read: meta.size > 0,
                    write: true,
                    error: false,
                });
            }
        }
        Ok(inode.async_poll().await?)
    }

    fn ioctl(&self, request: usize, arg1: usize, _arg2: usize, _arg3: usize) -> LxResult<usize> {
        // ioctl syscall
        self.inner.read().inode.io_control(request as u32, arg1)?;
        Ok(0)
    }

    fn is_input_device(&self) -> bool {
        use super::devfs::{EventDev, MiceDev};
        let inode = self.inner.read().inode.clone();
        inode.downcast_ref::<MiceDev>().is_some() || inode.downcast_ref::<EventDev>().is_some()
    }

    /// Returns the [`VmObject`] representing the file with given `offset` and `len`.
    fn get_vmo(&self, offset: usize, len: usize) -> LxResult<Arc<VmObject>> {
        let inner = self.inner.read();
        match inner.inode.metadata()?.type_ {
            FileType::File => {
                // Back the file mapping with a *paged* (non-contiguous) VMO that
                // is demand-paged from the file: each page is read in on the
                // page fault that first touches it, instead of reading the whole
                // mapping up front.
                //
                // Eagerly reading the whole mapping used to stall the machine:
                // the dynamic linker maps a library's entire LOAD span in one
                // `mmap`, and the ~150 MiB `libLLVM.so` pulled in by `perf`
                // forced ~9.6k synchronous 16 KiB reads plus a full commit of
                // every page before the syscall returned — on real hardware that
                // looked like a hard freeze (couldn't even switch VT). A non-PIE
                // program only touches a fraction of such a library, so paging it
                // in on demand reads (and commits) only what is actually used.
                //
                // The source captures the file inode and the file offset; bytes
                // past end-of-file stay zero (the BSS tail of a file mapping).
                let file_size = inner.inode.metadata()?.size;

                // If this file already has a live MAP_SHARED VMO, a MAP_PRIVATE
                // reader must copy-on-write from THAT, not from inode.read_at:
                // writes through the shared mapping (e.g. a compositor memcpy'ing
                // the wl_keyboard keymap into the memfd) never reach read_at, so
                // reading the inode would return stale zeros. See VmoFrameFiller.
                let key = Arc::as_ptr(&inner.inode) as *const () as usize;
                let shared_vmo = {
                    let mut registry = SHARED_FILE_VMOS.lock();
                    prune_shared_vmos(&mut registry);
                    registry.get(&key).map(|(vmo, _)| vmo.clone())
                };
                if let Some(shared) = shared_vmo {
                    let src_total = shared.len();
                    let source_len = src_total.saturating_sub(offset).min(len);
                    let source: Arc<dyn zircon_object::vm::FrameFiller> =
                        Arc::new(VmoFrameFiller {
                            src: shared,
                            base_offset: offset,
                            source_len,
                        });
                    return Ok(VmObject::new_paged_with_source(pages(len), source));
                }

                let source_len = file_size.saturating_sub(offset).min(len);
                if len >= 16 * 1024 * 1024 {
                    info!(
                        "get_vmo: demand-paged file map len={} MiB offset={:#x} source={} MiB",
                        len / (1024 * 1024),
                        offset,
                        source_len / (1024 * 1024),
                    );
                }
                let source: Arc<dyn zircon_object::vm::FrameFiller> = Arc::new(FileFrameFiller {
                    inode: inner.inode.clone(),
                    file_offset: offset,
                    source_len,
                });
                Ok(VmObject::new_paged_with_source(pages(len), source))
            }
            FileType::CharDevice => {
                use super::devfs::{DrmDev, FbDev};
                if let Some(fbdev) = inner.inode.downcast_ref::<FbDev>() {
                    fbdev.get_vmo(offset, len)
                } else if let Some(drmdev) = inner.inode.downcast_ref::<DrmDev>() {
                    drmdev.get_vmo(offset, len).map_err(Into::into)
                } else {
                    Err(LxError::ENOSYS)
                }
            }
            _ => Err(LxError::ENOSYS),
        }
    }

    fn get_vmo_shared(&self, offset: usize, len: usize) -> LxResult<(Arc<VmObject>, usize)> {
        let inner = self.inner.read();
        if inner.inode.metadata()?.type_ != FileType::File {
            // Devices keep their own get_vmo (fb/drm are inherently shared).
            drop(inner);
            return self.get_vmo(offset, len).map(|vmo| (vmo, 0));
        }
        if !offset.is_multiple_of(4096) {
            return Err(LxError::EINVAL);
        }
        let file_size = inner.inode.metadata()?.size;
        let key = Arc::as_ptr(&inner.inode) as *const () as usize;
        let mut registry = SHARED_FILE_VMOS.lock();
        prune_shared_vmos(&mut registry);
        if let Some((vmo, _)) = registry.get(&key) {
            let vmo = vmo.clone();
            if offset + len <= vmo.len() {
                return Ok((vmo, offset));
            }
            // Mapping reaches past the shared VMO (file grew after the first
            // MAP_SHARED). Rare; fall back to a snapshot rather than silently
            // truncating — but warn, because writes through this mapping will
            // NOT be visible to other mappers.
            warn!(
                "get_vmo_shared: mapping {:#x}+{:#x} exceeds shared vmo len {:#x} for {} — snapshot fallback",
                offset,
                len,
                vmo.len(),
                self.path()
            );
            drop(registry);
            drop(inner);
            return self.get_vmo(offset, len).map(|vmo| (vmo, 0));
        }
        // First MAP_SHARED of this file: build one VMO covering the whole file
        // (so later mappers at other offsets share it too), demand-paged from
        // the inode. Created under the registry lock so a concurrent first-map
        // cannot race us into two unshared VMOs. Held STRONG, anchored to the
        // inode's lifetime (see SHARED_FILE_VMOS) so MAP_SHARED writes survive
        // the writer's munmap — the wl_keyboard keymap depends on this.
        let vmo_len = file_size.max(offset + len);
        let source: Arc<dyn zircon_object::vm::FrameFiller> = Arc::new(FileFrameFiller {
            inode: inner.inode.clone(),
            file_offset: 0,
            source_len: file_size,
        });
        let vmo = VmObject::new_paged_with_source(pages(vmo_len), source);
        registry.insert(key, (vmo.clone(), Arc::downgrade(&inner.inode)));
        Ok((vmo, offset))
    }
}
