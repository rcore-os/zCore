//! btrfs mount support via the in-tree `btrfs` crate.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::any::Any;
use core::convert::TryInto;
use core::sync::atomic::{AtomicUsize, Ordering};

use btrfs::{Btrfs, Error as BtrfsError, FileKind};
use lock::Mutex;
use rcore_fs::vfs::{
    FileSystem, FileType, FsError, FsInfo, INode, Metadata, PollStatus, Result, Timespec,
};
use zcore_drivers::scheme::BlockScheme;

use super::block_mount::{device_from_backend, MountBackend};

/// Adapter: rcore-fs `Device` (+ explicit size) → `btrfs::BlockDevice`.
struct DevAdapter {
    inner: Arc<dyn rcore_fs::dev::Device>,
    size: u64,
    /// Adaptive cap on a single backend transfer. Starts at `IO_CHUNK_BYTES`
    /// and ratchets *down* (never up) the first time a larger transfer fails,
    /// so a device/controller that can't sustain big DMA requests settles to a
    /// working size instead of re-failing (and burning retries) on every chunk
    /// of a large file. See `chunked`.
    max_xfer: AtomicUsize,
}

/// How many times a single block transfer is retried before the error is
/// surfaced to btrfs. A large file (e.g. the ~130 MiB `libLLVM.so` pulled in by
/// `apk fix`) is written as *hundreds* of separate block commands, so even a
/// rare transient device error (an AHCI task-file error that the driver clears
/// with a port reset, a momentarily busy controller, …) becomes likely over the
/// whole file. Without a retry that single hiccup aborts the entire extraction
/// with EIO, which is exactly the "failed to extract …: I/O error" seen only on
/// the biggest package. Re-issuing the same offset/buffer is idempotent.
const IO_RETRIES: usize = 5;
/// Cap one backend transfer to a moderate size so block drivers that reject
/// very large requests don't fail a whole btrfs operation.
const IO_CHUNK_BYTES: usize = 128 * 1024;
/// Smallest transfer the shrink-on-error fallback drops to before giving up.
/// Some controllers / DMA paths reject (or intermittently fail) a large
/// request but accept smaller ones. The large requests only happen while
/// streaming a big file's data, so without this fallback a single ~130 MiB
/// package (`libLLVM.so`) fails extraction with EIO while every small package
/// installs fine. Shrinking the failing transfer lets the big file complete
/// (more, smaller commands) instead of aborting the whole operation.
const IO_MIN_BYTES: usize = 512;

impl DevAdapter {
    /// Transfer `len` bytes in chunks via `op`. Each chunk is retried
    /// `IO_RETRIES` times; if it still fails the chunk is halved (down to
    /// `IO_MIN_BYTES`) and retried, so a size-sensitive device failure on a
    /// large request degrades to slower-but-working smaller requests instead of
    /// a hard EIO. `op(rel_off, this_len)` performs one transfer and returns
    /// `true` on full success.
    fn chunked<F: FnMut(usize, usize) -> bool>(
        &self,
        offset: u64,
        len: usize,
        what: &str,
        mut op: F,
    ) -> btrfs::Result<()> {
        let mut done = 0usize;
        while done < len {
            let cap = self.max_xfer.load(Ordering::Relaxed).max(IO_MIN_BYTES);
            let mut piece = (len - done).min(cap);
            loop {
                let mut ok = false;
                for _ in 0..IO_RETRIES {
                    if op(done, piece) {
                        ok = true;
                        break;
                    }
                }
                if ok {
                    done += piece;
                    break;
                }
                if piece > IO_MIN_BYTES {
                    // Re-issuing the same offset/buffer is idempotent; try a
                    // smaller, more conservative transfer. Remember the smaller
                    // size so the rest of this (large) file uses it directly
                    // instead of re-failing the big transfer on every chunk.
                    piece = (piece / 2).max(IO_MIN_BYTES);
                    self.max_xfer.fetch_min(piece, Ordering::Relaxed);
                    warn!(
                        "btrfs: {} transfer shrunk to {} after failure (off={:#x}, +{})",
                        what, piece, offset, done,
                    );
                    continue;
                }
                warn!(
                    "btrfs: {}(off={:#x}, len={}) failed at +{} even at {}-byte transfers, dev_size={:#x}",
                    what, offset, len, done, IO_MIN_BYTES, self.size,
                );
                return Err(BtrfsError::Io);
            }
        }
        Ok(())
    }
}

impl btrfs::BlockDevice for DevAdapter {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> btrfs::Result<()> {
        let len = buf.len();
        self.chunked(offset, len, "read_at", |rel, n| {
            matches!(
                self.inner.read_at(offset as usize + rel, &mut buf[rel..rel + n]),
                Ok(got) if got == n
            )
        })
    }

    fn write_at(&self, offset: u64, buf: &[u8]) -> btrfs::Result<()> {
        let end = offset + buf.len() as u64;
        if end > self.size {
            // btrfs asked us to write past the end of the device it was told
            // about: this is an allocation/geometry bug in the FS layer, not a
            // device fault. Surface it loudly with the numbers needed to debug.
            warn!(
                "btrfs: write_at OUT OF BOUNDS off={:#x} len={} end={:#x} > dev_size={:#x}",
                offset,
                buf.len(),
                end,
                self.size,
            );
        }
        let len = buf.len();
        self.chunked(offset, len, "write_at", |rel, n| {
            matches!(
                self.inner.write_at(offset as usize + rel, &buf[rel..rel + n]),
                Ok(got) if got == n
            )
        })
    }

    fn sync(&self) -> btrfs::Result<()> {
        self.inner.sync().map_err(|_| BtrfsError::Io)
    }

    fn size(&self) -> u64 {
        self.size
    }
}

fn backend_size(backend: &MountBackend) -> Result<u64> {
    match backend {
        MountBackend::Block(block) => Ok(block.block_count() as u64 * 512),
        MountBackend::File(file) => Ok(file.metadata()?.size as u64),
    }
}

fn map_err(e: BtrfsError) -> FsError {
    match e {
        BtrfsError::Io => FsError::DeviceError,
        BtrfsError::BadSuperblock => FsError::WrongFs,
        BtrfsError::Corrupt(msg) => {
            warn!("btrfs: corrupt filesystem: {}", msg);
            FsError::DeviceError
        }
        BtrfsError::Unsupported(msg) => {
            warn!("btrfs: unsupported: {}", msg);
            FsError::NotSupported
        }
        BtrfsError::NotFound => FsError::EntryNotFound,
        BtrfsError::Exists => FsError::EntryExist,
        BtrfsError::NotDir => FsError::NotDir,
        BtrfsError::IsDir => FsError::IsDir,
        BtrfsError::NotEmpty => FsError::DirNotEmpty,
        BtrfsError::NoSpace => FsError::NoDeviceSpace,
        BtrfsError::Invalid => FsError::InvalidParam,
    }
}

fn wall_clock() -> (u64, u32) {
    let now = kernel_hal::timer::wall_clock_now();
    (now.as_secs(), now.subsec_nanos())
}

pub struct BtrfsMountFs {
    inner: Mutex<Btrfs>,
    this: Mutex<Weak<Self>>,
    /// Cached directory listings keyed by inode number (cleared on any
    /// mutation of that directory).
    dir_cache: Mutex<BTreeMap<u64, Arc<Vec<CachedDirEntry>>>>,
    /// Kernel-side write-back coalescing buffer (page-cache-lite). Sequential
    /// small writes to one file are accumulated here and handed to the FS in
    /// large chunks, turning the ~32000 tiny synchronous writes of a big
    /// package extraction (which stalled `apk`) into a few hundred large ones.
    /// At most one file is buffered at a time; any access that isn't a
    /// contiguous append flushes it first, so reads always observe written
    /// data. See `flush_inode` / `flush_any`.
    write_buf: Mutex<Option<PendingWrite>>,
}

/// Pending tail of buffered, not-yet-committed writes for a single inode.
struct PendingWrite {
    ino: u64,
    start: u64,
    data: Vec<u8>,
}

/// Flush the accumulated buffer once it reaches this size.
const WRITE_BUF_FLUSH: usize = 1024 * 1024;

#[derive(Clone)]
struct CachedDirEntry {
    name: String,
    ino: u64,
}

impl BtrfsMountFs {
    pub fn open(backend: &MountBackend, read_only: bool) -> Result<Arc<Self>> {
        let size = backend_size(backend)?;
        let device = device_from_backend(backend)?;
        let adapter: Arc<dyn btrfs::BlockDevice> = Arc::new(DevAdapter {
            inner: device,
            size,
            max_xfer: AtomicUsize::new(IO_CHUNK_BYTES),
        });
        warn!(
            "btrfs: mounting, device size = {:#x} ({} MiB), read_only={}",
            size,
            size / (1024 * 1024),
            read_only
        );
        let mut fs = Btrfs::mount(adapter, read_only).map_err(map_err)?;
        fs.set_clock(wall_clock);
        // Auto-expand to the partition size (the installer writes a small
        // image onto a larger partition and relies on this).
        if !read_only {
            match fs.grow_to_device() {
                Ok(true) => warn!("btrfs: filesystem expanded to device size"),
                Ok(false) => warn!("btrfs: NOT expanded (FS dev size >= partition size)"),
                Err(e) => warn!("btrfs: grow_to_device failed: {:?}", e),
            }
        }
        let arc = Arc::new(Self {
            inner: Mutex::new(fs),
            this: Mutex::new(Weak::new()),
            dir_cache: Mutex::new(BTreeMap::new()),
            write_buf: Mutex::new(None),
        });
        *arc.this.lock() = Arc::downgrade(&arc);
        Ok(arc)
    }

    fn arc(&self) -> Arc<Self> {
        self.this.lock().upgrade().expect("BtrfsMountFs dropped")
    }

    fn inode(&self, ino: u64) -> Arc<BtrfsMountINode> {
        Arc::new(BtrfsMountINode {
            fs: self.arc(),
            ino,
            kind: Mutex::new(None),
        })
    }

    fn cached_readdir(&self, dir: u64) -> Result<Arc<Vec<CachedDirEntry>>> {
        if let Some(entries) = self.dir_cache.lock().get(&dir) {
            return Ok(entries.clone());
        }
        let entries = {
            let mut fs = self.inner.lock();
            let entries = fs.readdir(dir).map_err(map_err)?;
            let mut cached = Vec::with_capacity(entries.len());
            for entry in entries {
                cached.push(CachedDirEntry {
                    name: entry.name,
                    ino: entry.ino,
                });
            }
            Arc::new(cached)
        };
        self.dir_cache.lock().insert(dir, entries.clone());
        Ok(entries)
    }

    fn invalidate_dir(&self, dir: u64) {
        self.dir_cache.lock().remove(&dir);
    }

    /// Write a pending buffer out to the filesystem in full. `fs` is the
    /// already-locked inner FS (callers must hold it; lock order is always
    /// `inner` before `write_buf`, so this never deadlocks).
    fn flush_pending(fs: &mut Btrfs, pw: PendingWrite) -> Result<()> {
        let mut off = pw.start;
        let mut done = 0usize;
        while done < pw.data.len() {
            let n = fs.write(pw.ino, off, &pw.data[done..]).map_err(map_err)?;
            if n == 0 {
                return Err(FsError::DeviceError);
            }
            off += n as u64;
            done += n;
        }
        Ok(())
    }

    /// Flush the buffer iff it belongs to `ino`.
    fn flush_inode(&self, fs: &mut Btrfs, ino: u64) -> Result<()> {
        let taken = {
            let mut wb = self.write_buf.lock();
            match &*wb {
                Some(pw) if pw.ino == ino => wb.take(),
                _ => None,
            }
        };
        if let Some(pw) = taken {
            Self::flush_pending(fs, pw)?;
        }
        Ok(())
    }

    /// Flush any pending buffer regardless of inode.
    fn flush_any(&self, fs: &mut Btrfs) -> Result<()> {
        let taken = self.write_buf.lock().take();
        if let Some(pw) = taken {
            Self::flush_pending(fs, pw)?;
        }
        Ok(())
    }

    /// Size contributed by a buffered tail for `ino`, if any (so `stat` reflects
    /// not-yet-flushed writes without forcing a flush).
    fn buffered_end(&self, ino: u64) -> Option<u64> {
        let wb = self.write_buf.lock();
        match &*wb {
            Some(pw) if pw.ino == ino => Some(pw.start + pw.data.len() as u64),
            _ => None,
        }
    }
}

impl FileSystem for BtrfsMountFs {
    fn sync(&self) -> Result<()> {
        let mut fs = self.inner.lock();
        self.flush_any(&mut fs)?;
        fs.sync().map_err(map_err)
    }

    fn root_inode(&self) -> Arc<dyn INode> {
        let root = self.inner.lock().root_ino();
        self.inode(root)
    }

    fn info(&self) -> FsInfo {
        let stat = self.inner.lock().fsinfo();
        let bsize = stat.block_size.max(1);
        FsInfo {
            bsize: bsize as usize,
            frsize: bsize as usize,
            blocks: (stat.total_bytes / bsize) as usize,
            bfree: (stat.total_bytes.saturating_sub(stat.bytes_used) / bsize) as usize,
            bavail: (stat.total_bytes.saturating_sub(stat.bytes_used) / bsize) as usize,
            files: 0,
            ffree: 0,
            namemax: 255,
        }
    }
}

struct BtrfsMountINode {
    fs: Arc<BtrfsMountFs>,
    ino: u64,
    /// The inode's kind, resolved once on first use. A btrfs object never
    /// changes kind while it exists, and `read_at` runs once per 4 KiB on the
    /// demand-paging path — a full B-tree `stat` per call is pure overhead.
    kind: Mutex<Option<FileKind>>,
}

impl BtrfsMountINode {
    /// The inode's kind, from cache or via one `stat` on the locked fs.
    fn kind(&self, fs: &mut Btrfs) -> Result<FileKind> {
        let mut cached = self.kind.lock();
        if let Some(kind) = *cached {
            return Ok(kind);
        }
        let kind = fs.stat(self.ino).map_err(map_err)?.kind;
        *cached = Some(kind);
        Ok(kind)
    }
}

fn vfs_type(kind: FileKind) -> FileType {
    match kind {
        FileKind::Regular => FileType::File,
        FileKind::Dir => FileType::Dir,
        FileKind::Symlink => FileType::SymLink,
        FileKind::CharDevice => FileType::CharDevice,
        FileKind::BlockDevice => FileType::BlockDevice,
        FileKind::Fifo => FileType::NamedPipe,
        FileKind::Socket => FileType::Socket,
    }
}

fn btrfs_kind(type_: FileType) -> Result<FileKind> {
    Ok(match type_ {
        FileType::File => FileKind::Regular,
        FileType::Dir => FileKind::Dir,
        FileType::SymLink => FileKind::Symlink,
        FileType::CharDevice => FileKind::CharDevice,
        FileType::BlockDevice => FileKind::BlockDevice,
        FileType::NamedPipe => FileKind::Fifo,
        FileType::Socket => FileKind::Socket,
    })
}

fn stat_to_metadata(st: &btrfs::InodeStat) -> Metadata {
    Metadata {
        dev: 0,
        inode: st.ino as usize,
        size: st.size as usize,
        blk_size: 512,
        blocks: st.nbytes.div_ceil(512) as usize,
        atime: Timespec {
            sec: st.atime.0 as i64,
            nsec: st.atime.1 as i32,
        },
        mtime: Timespec {
            sec: st.mtime.0 as i64,
            nsec: st.mtime.1 as i32,
        },
        ctime: Timespec {
            sec: st.ctime.0 as i64,
            nsec: st.ctime.1 as i32,
        },
        type_: vfs_type(st.kind),
        mode: (st.mode & 0o7777) as u16,
        nlinks: st.nlink as usize,
        uid: st.uid as usize,
        gid: st.gid as usize,
        rdev: st.rdev as usize,
    }
}

impl INode for BtrfsMountINode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        let mut fs = self.fs.inner.lock();
        // A read must observe everything written so far: flush this inode's
        // coalescing buffer before serving it.
        self.fs.flush_inode(&mut fs, self.ino)?;
        match self.kind(&mut fs)? {
            FileKind::Dir => Err(FsError::IsDir),
            FileKind::Symlink => {
                let target = fs.read_link(self.ino).map_err(map_err)?;
                if offset >= target.len() {
                    return Ok(0);
                }
                let take = buf.len().min(target.len() - offset);
                buf[..take].copy_from_slice(&target[offset..offset + take]);
                Ok(take)
            }
            _ => fs.read(self.ino, offset as u64, buf).map_err(|e| {
                // Surface the exact failing operation in dmesg (klog bypasses the
                // log-level filter), so an "I/O error" can be pinned to a btrfs
                // reason + offset instead of guessing.
                zcore_drivers::klog_err!(
                    "btrfs: read ino={} off={:#x} len={} -> {:?}",
                    self.ino,
                    offset,
                    buf.len(),
                    e,
                );
                map_err(e)
            }),
        }
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut fs = self.fs.inner.lock();
        let off = offset as u64;

        // Fast path: a contiguous append to the already-buffered file. The file
        // is known regular (only regular files are buffered, and unlink/rename
        // flush first), so we skip even the `stat` and just grow the buffer.
        {
            let mut wb = self.fs.write_buf.lock();
            let hit = matches!(
                &*wb,
                Some(pw) if pw.ino == self.ino && pw.start + pw.data.len() as u64 == off
            );
            if hit {
                let pw = wb.as_mut().unwrap();
                pw.data.extend_from_slice(buf);
                if pw.data.len() < WRITE_BUF_FLUSH {
                    return Ok(buf.len());
                }
                let full = wb.take().unwrap();
                drop(wb);
                BtrfsMountFs::flush_pending(&mut fs, full)?;
                return Ok(buf.len());
            }
        }

        // Slow path: new file / non-contiguous offset. Determine the kind and
        // flush any other inode's pending buffer first.
        let st = fs.stat(self.ino).map_err(map_err)?;
        match st.kind {
            FileKind::Dir => return Err(FsError::IsDir),
            FileKind::Symlink => {
                self.fs.flush_any(&mut fs)?;
                return fs.write_symlink(self.ino, off, buf).map_err(map_err);
            }
            _ => {}
        }
        self.fs.flush_any(&mut fs)?;
        // Small write: start a fresh buffer. Large write: straight through.
        if buf.len() < WRITE_BUF_FLUSH {
            *self.fs.write_buf.lock() = Some(PendingWrite {
                ino: self.ino,
                start: off,
                data: buf.to_vec(),
            });
            return Ok(buf.len());
        }
        fs.write(self.ino, off, buf).map_err(|e| {
            zcore_drivers::klog_err!(
                "btrfs: write ino={} off={:#x} len={} -> {:?}",
                self.ino,
                offset,
                buf.len(),
                e,
            );
            map_err(e)
        })
    }

    fn poll(&self) -> Result<PollStatus> {
        let st = {
            let mut fs = self.fs.inner.lock();
            fs.stat(self.ino).map_err(map_err)?
        };
        Ok(PollStatus {
            read: true,
            write: st.kind != FileKind::Dir,
            error: false,
        })
    }

    fn metadata(&self) -> Result<Metadata> {
        let mut fs = self.fs.inner.lock();
        let mut st = fs.stat(self.ino).map_err(map_err)?;
        // Reflect not-yet-flushed buffered writes in the reported size without
        // forcing a flush (keeps buffering effective when callers `fstat`).
        if let Some(end) = self.fs.buffered_end(self.ino) {
            if end > st.size {
                st.size = end;
                st.nbytes = st.nbytes.max(end);
            }
        }
        Ok(stat_to_metadata(&st))
    }

    fn set_metadata(&self, metadata: &Metadata) -> Result<()> {
        let mut fs = self.fs.inner.lock();
        // Flush first: a later flush would otherwise overwrite mtime/ctime with
        // "now" and clobber the times being set here (apk sets archive mtimes
        // right after extracting a file).
        self.fs.flush_inode(&mut fs, self.ino)?;
        fs.set_attr(
            self.ino,
            Some(metadata.mode as u32),
            Some(metadata.uid as u32),
            Some(metadata.gid as u32),
            Some((metadata.atime.sec as u64, metadata.atime.nsec as u32)),
            Some((metadata.mtime.sec as u64, metadata.mtime.nsec as u32)),
        )
        .map_err(map_err)
    }

    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." | "" => Ok(self.fs.inode(self.ino)),
            ".." => Err(FsError::EntryNotFound),
            name => {
                let ino = {
                    let mut fs = self.fs.inner.lock();
                    fs.lookup(self.ino, name).map_err(map_err)?
                };
                Ok(self.fs.inode(ino))
            }
        }
    }

    fn get_entry(&self, id: usize) -> Result<String> {
        Ok(self.get_entry_with_metadata(id)?.1)
    }

    fn get_entry_with_metadata(&self, id: usize) -> Result<(Metadata, String)> {
        match id {
            0 => Ok((self.metadata()?, String::from("."))),
            1 => Ok((self.metadata()?, String::from(".."))),
            i => {
                let entries = self.fs.cached_readdir(self.ino)?;
                let entry = entries.get(i - 2).ok_or(FsError::EntryNotFound)?;
                let metadata = {
                    let mut fs = self.fs.inner.lock();
                    let st = fs.stat(entry.ino).map_err(map_err)?;
                    stat_to_metadata(&st)
                };
                Ok((metadata, entry.name.clone()))
            }
        }
    }

    fn create2(
        &self,
        name: &str,
        type_: FileType,
        mode: u32,
        data: usize,
    ) -> Result<Arc<dyn INode>> {
        let kind = btrfs_kind(type_)?;
        let ino = {
            let mut fs = self.fs.inner.lock();
            // Flush any buffered file before mutating the namespace, so its data
            // is durable before another file/operation depends on it.
            self.fs.flush_any(&mut fs)?;
            fs.create(self.ino, name, kind, mode, data as u64)
                .map_err(map_err)?
        };
        self.fs.invalidate_dir(self.ino);
        Ok(self.fs.inode(ino))
    }

    fn unlink(&self, name: &str) -> Result<()> {
        {
            let mut fs = self.fs.inner.lock();
            self.fs.flush_any(&mut fs)?;
            fs.unlink(self.ino, name).map_err(map_err)?;
        }
        self.fs.invalidate_dir(self.ino);
        Ok(())
    }

    fn link(&self, name: &str, other: &Arc<dyn INode>) -> Result<()> {
        let other = other
            .downcast_ref::<BtrfsMountINode>()
            .ok_or(FsError::NotSameFs)?;
        if !Arc::ptr_eq(&self.fs, &other.fs) {
            return Err(FsError::NotSameFs);
        }
        {
            let mut fs = self.fs.inner.lock();
            self.fs.flush_any(&mut fs)?;
            fs.link(self.ino, name, other.ino).map_err(map_err)?;
        }
        self.fs.invalidate_dir(self.ino);
        Ok(())
    }

    fn move_(&self, old_name: &str, target: &Arc<dyn INode>, new_name: &str) -> Result<()> {
        let target = target
            .downcast_ref::<BtrfsMountINode>()
            .ok_or(FsError::NotSameFs)?;
        if !Arc::ptr_eq(&self.fs, &target.fs) {
            return Err(FsError::NotSameFs);
        }
        {
            let mut fs = self.fs.inner.lock();
            self.fs.flush_any(&mut fs)?;
            fs.rename(self.ino, old_name, target.ino, new_name)
                .map_err(map_err)?;
        }
        self.fs.invalidate_dir(self.ino);
        self.fs.invalidate_dir(target.ino);
        Ok(())
    }

    fn resize(&self, len: usize) -> Result<()> {
        let mut fs = self.fs.inner.lock();
        // Pending writes must land before the truncate so the final size/extents
        // are correct.
        self.fs.flush_inode(&mut fs, self.ino)?;
        fs.truncate(self.ino, len as u64).map_err(map_err)
    }

    fn sync_all(&self) -> Result<()> {
        let mut fs = self.fs.inner.lock();
        self.fs.flush_any(&mut fs)?;
        fs.sync().map_err(map_err)
    }

    fn sync_data(&self) -> Result<()> {
        self.sync_all()
    }

    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs.clone()
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}

/// Open a mount backend as a btrfs filesystem.
pub fn open_btrfs(backend: &MountBackend, read_only: bool) -> Result<Arc<dyn FileSystem>> {
    BtrfsMountFs::open(backend, read_only).map(|fs| fs as Arc<dyn FileSystem>)
}

/// Cheap pre-mount probe: does the backing device look like btrfs?
pub(crate) fn probe_btrfs_superblock(block: &Arc<dyn BlockScheme>) -> bool {
    // Primary superblock lives at byte 0x10000; magic at +0x40.
    const SB_SECTOR: usize = 0x10000 / 512;
    #[repr(align(4096))]
    struct SectorBuf([u8; 512]);
    let mut sb = SectorBuf([0u8; 512]);
    if block.read_block(SB_SECTOR, &mut sb.0).is_err() {
        return false;
    }
    let magic = u64::from_le_bytes(sb.0[0x40..0x48].try_into().unwrap());
    if magic != 0x4D5F53665248425F {
        return false;
    }
    let num_devices = u64::from_le_bytes(sb.0[0x88..0x90].try_into().unwrap());
    let total_bytes = u64::from_le_bytes(sb.0[0x70..0x78].try_into().unwrap());
    let device_bytes = block.block_count() as u64 * 512;
    num_devices == 1 && total_bytes > 0 && total_bytes <= device_bytes.saturating_mul(2)
}
