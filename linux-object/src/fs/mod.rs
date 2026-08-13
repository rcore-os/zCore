//! Linux file objects

mod block_mount;
mod btrfs_mount;
pub mod devfs;
mod dmabuf;
mod epoll;
mod eventfd;
mod fat_mount;
mod file;
mod flagged_fs;
pub mod hunter_config;
mod inotify;
pub mod ioctl;
mod mount_ops;
mod mount_state;
mod perf;
mod pidfd;
mod pipe;
mod proc_self;
mod procfs;
mod pseudo;
pub mod pty;
pub mod rcore_fs_wrapper;
pub mod record_lock;
mod signalfd;
pub mod stdio;
mod syncobj_file;
mod sysfs;
mod timerfd;

#[cfg(feature = "mock-disk")]
pub mod mock;

#[cfg(feature = "mock-disk")]
/// Start simulating the disk
pub fn mocking_block(initrd: &'static mut [u8]) -> ! {
    mock::mocking(initrd)
}

#[cfg(feature = "mock-disk")]
/// Drivers for the mock disk
pub fn mock_block() -> mock::MockBlock {
    mock::MockBlock::new()
}

use alloc::{
    boxed::Box, collections::BTreeMap, fmt::Write as _, string::String, string::ToString,
    sync::Arc, vec::Vec,
};
use core::convert::TryFrom;

use async_trait::async_trait;
use lazy_static::lazy_static;
use lock::Mutex;

use kernel_hal::drivers;
use rcore_fs::vfs::{FileSystem, FileType, INode, Result};
use rcore_fs_devfs::{
    special::{NullINode, ZeroINode},
    DevFS, DevINode,
};
use rcore_fs_mountfs::{MNode, MountFS};
use rcore_fs_ramfs::RamFS;

lazy_static! {
    pub(crate) static ref DEVFS_ROOT: Mutex<Option<Arc<DevINode>>> = Mutex::new(None);
}
use zircon_object::{object::KernelObject, vm::VmObject};

use crate::error::{LxError, LxResult};
use crate::net::Socket;
use crate::process::LinuxProcess;
use devfs::RandomINode;
use procfs::ProcFS;
use pseudo::Pseudo;
use sysfs::SysFS;

pub use dmabuf::DmaBuf;
pub use epoll::{Epoll, EpollEvent};

/// If `f` is a DRM-related fd (a /dev/dri device File, a PRIME dma-buf, or
/// an exported syncobj), return a short description for diagnostics; None
/// otherwise.
///
/// Used by the fd close paths to make every removal of a DRM fd visible on the
/// console: the labwc bring-up failed with a PRIME ioctl arriving on an fd
/// that was NOT in the fd table, and without close-side logging it is
/// undecidable who removed it (or which process the stale number belonged to).
pub fn drm_fd_desc(f: &alloc::sync::Arc<dyn FileLike>) -> Option<alloc::string::String> {
    if f.downcast_ref::<DmaBuf>().is_some() {
        return Some(alloc::string::String::from("dmabuf"));
    }
    if let Some(s) = f.downcast_ref::<SyncobjHandle>() {
        return Some(alloc::format!("syncobj(handle={})", s.handle));
    }
    if let Some(file) = f.downcast_ref::<File>() {
        let p = file.path();
        if p.starts_with("/dev/dri") {
            return Some(p.clone());
        }
    }
    None
}
pub use eventfd::EventFd;
pub use file::{File, OpenFlags, PollEvents, SeekFrom};
pub use inotify::Inotify;
pub use perf::{sample_user as perf_sample_user, PerfEvent};
pub use pidfd::{PidFd, PIDFD_THREAD};
pub use pipe::Pipe;
pub use rcore_fs::vfs::{self, PollStatus};
pub use signalfd::SignalFd;
pub use stdio::{STDIN, STDOUT};
pub use syncobj_file::SyncobjHandle;
pub use timerfd::TimerFd;

#[derive(Clone)]
struct MountEntry {
    source: String,
    target: String,
    fstype: String,
    options: String,
    state: Arc<mount_state::MountState>,
}

lazy_static! {
    static ref MOUNT_TABLE: Mutex<Vec<MountEntry>> = Mutex::new(Vec::new());
}

lazy_static! {
    /// Dedicated, never-mounted ramfs backing `memfd_create(2)`. Kept alive for
    /// the kernel's lifetime so the anonymous inodes' `Weak<RamFS>` back-refs
    /// always upgrade (rcore-fs ramfs panics on a dropped fs).
    static ref MEMFD_FS: Arc<RamFS> = RamFS::new();
}
static MEMFD_SEQ: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

lazy_static! {
    /// Weak refs to every memfd inode ever created, for leak diagnostics: the
    /// desktop OOM traced to ~456 MiB of LIVE ramfs page blocks reachable only
    /// from memfd files that should have been freed on close. `memfd_stats`
    /// prunes dead entries and reports how many inodes are still alive and how
    /// many bytes they pin.
    static ref MEMFD_LIVE: Mutex<Vec<(usize, alloc::sync::Weak<dyn INode>)>> =
        Mutex::new(Vec::new());
}

/// (created_total, live_count, live_bytes) for memfd inodes.
///
/// Uses `try_lock`: this is called from the OOM handler, and a concurrent
/// `new_memfd` registration (or a caller further up the stack) may hold the
/// registry lock — blocking here would wedge the OOM report (seen in the lab:
/// both CPUs froze mid-report). Returns zeros for live/bytes if contended.
pub fn memfd_stats() -> (usize, usize, usize) {
    let created = MEMFD_SEQ.load(core::sync::atomic::Ordering::Relaxed);
    let mut live = match MEMFD_LIVE.try_lock() {
        Some(guard) => guard,
        None => return (created, 0, 0),
    };
    live.retain(|(_, w)| w.strong_count() > 0);
    let mut bytes = 0usize;
    let count = live.len();
    for (_, w) in live.iter() {
        if let Some(inode) = w.upgrade() {
            if let Ok(m) = inode.metadata() {
                bytes += m.size;
            }
        }
    }
    (created, count, bytes)
}

/// One line per live memfd (seq, size, inode strong count), newest first,
/// up to `max` entries — enough to tell leaked wl_shm pools (MiB-sized, extra
/// strong refs) from cursor-sized scratch files.
pub fn memfd_dump_live(max: usize) {
    let live = match MEMFD_LIVE.try_lock() {
        Some(guard) => guard,
        None => return,
    };
    let (created, count) = (
        MEMFD_SEQ.load(core::sync::atomic::Ordering::Relaxed),
        live.iter().filter(|(_, w)| w.strong_count() > 0).count(),
    );
    warn!("[memfd] created={} live={}", created, count);
    for (seq, w) in live.iter().rev().take(max) {
        if let Some(inode) = w.upgrade() {
            let size = inode.metadata().map(|m| m.size).unwrap_or(0);
            // strong_count includes our temporary upgrade — report without it.
            warn!(
                "[memfd]   seq={} size={} strong={}",
                seq,
                size,
                alloc::sync::Weak::strong_count(w).saturating_sub(1)
            );
        }
    }
}

lazy_static! {
    /// Writable ramfs exposed as the `/dev/shm` directory so POSIX `shm_open()`
    /// can create files there (e.g. wlroots' xkb keymap fd). It is inserted
    /// straight into devfs (not mounted via MountFS): `/dev/*` lookups take the
    /// `lookup_virtual_fs` fast path, which resolves against the raw `DEVFS_ROOT`
    /// and never consults the MountFS overlay — so a RamFS *mounted* on a devfs
    /// `shm` dir would be invisible. Kept alive for the kernel's lifetime so the
    /// inodes' `Weak<RamFS>` back-refs always upgrade.
    static ref DEV_SHM_FS: Arc<RamFS> = RamFS::new();
}

/// Create an anonymous in-RAM file for `memfd_create(2)`. The inode lives in a
/// hidden ramfs and is immediately unlinked, so it has no name in any mounted
/// namespace and is freed once the last fd referring to it closes — while
/// `mmap`/`ftruncate`/`read`/`write` reuse the regular-file machinery. Wayland
/// (`os_create_anonymous_file`), wlroots and Mesa use this to share xkb keymaps
/// and shm pools.
pub fn new_memfd(name: &str, flags: usize) -> LxResult<Arc<File>> {
    use rcore_fs::vfs::FileType;
    /// `MFD_CLOEXEC` (the only flag we act on; `MFD_ALLOW_SEALING` is accepted
    /// and seals are no-ops, `MFD_HUGETLB` is ignored).
    const MFD_CLOEXEC: usize = 0x0001;

    let root = MEMFD_FS.root_inode();
    let seq = MEMFD_SEQ.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let backing = alloc::format!(".memfd.{}.{}", seq, name);
    let inode = root.create(&backing, FileType::File, 0o600)?;
    // Detach the directory entry: the open fd is the sole owner now, so a close
    // frees the RAM and nothing can resolve the (hidden) name.
    let _ = root.unlink(&backing);
    {
        // Never allocate while holding the registry lock: a Vec growth OOMing
        // under it self-deadlocks the OOM reporter (which takes this lock).
        // Grow by building a bigger buffer OUTSIDE the lock and swapping it in
        // (drain+extend under the lock is a plain memcpy, no allocation).
        let mut elem = Some((seq, alloc::sync::Arc::downgrade(&inode)));
        loop {
            let cap = {
                let mut live = MEMFD_LIVE.lock();
                if live.len() < live.capacity() {
                    live.push(elem.take().unwrap());
                    break;
                }
                live.capacity()
            };
            let mut spare = Vec::with_capacity((cap * 2).max(64));
            let mut live = MEMFD_LIVE.lock();
            if live.len() < spare.capacity() {
                spare.extend(live.drain(..));
                spare.push(elem.take().unwrap());
                *live = spare;
                break;
            }
            // Raced with another grower that filled even the doubled size;
            // retry with the fresh capacity.
        }
    }

    let mut open_flags = OpenFlags::RDWR;
    if flags & MFD_CLOEXEC != 0 {
        open_flags |= OpenFlags::CLOEXEC;
    }
    Ok(File::new(
        inode,
        open_flags,
        alloc::format!("/memfd:{name}"),
    ))
}

fn reset_mount_table() {
    MOUNT_TABLE.lock().clear();
}

fn boot_mount_state() -> Arc<mount_state::MountState> {
    Arc::new(mount_state::MountState::new(false))
}

/// Resolve a top-level mount directory on the pivoted block-device root
/// (btrfs/ext2) without `MNode::find` overlay/metadata overhead (VBox disk
/// boot).
fn boot_resolve_mount_dir(
    rootfs: &Arc<MountFS>,
    root: &Arc<MNode>,
    name: &str,
    mode: u32,
) -> Arc<MNode> {
    warn!("[boot] lookup /{} on backing", name);
    if let Ok(inode) = rootfs.inner_fs().root_inode().find(name) {
        warn!("[boot] found /{}", name);
        return MNode::from_backing(rootfs.clone(), inode);
    }
    warn!("[boot] mkdir /{}", name);
    root.create(name, FileType::Dir, mode)
        .expect("failed to mkdir")
}

fn resolve_mount_dir(
    rootfs: &Arc<MountFS>,
    root: &Arc<MNode>,
    root_fstype: &str,
    name: &str,
    mode: u32,
) -> Arc<MNode> {
    if root_fstype == "btrfs" {
        boot_resolve_mount_dir(rootfs, root, name, mode)
    } else {
        root.find(true, name).unwrap_or_else(|_| {
            root.create(name, FileType::Dir, mode)
                .expect("failed to mkdir")
        })
    }
}

pub(crate) fn register_mount(
    source: &str,
    target: &str,
    fstype: &str,
    options: &str,
    state: Arc<mount_state::MountState>,
) {
    MOUNT_TABLE.lock().push(MountEntry {
        source: source.to_string(),
        target: target.to_string(),
        fstype: fstype.to_string(),
        options: options.to_string(),
        state,
    });
}

pub(crate) fn unregister_mount(target: &str) {
    MOUNT_TABLE.lock().retain(|m| m.target != target);
}

pub(crate) fn remount_flags(target: &str, flags: usize, data: &str) -> LxResult<()> {
    let target = normalize_mount_target(target);
    let mut mounts = MOUNT_TABLE.lock();
    let entry = mounts
        .iter_mut()
        .find(|m| m.target == target)
        .ok_or(LxError::EINVAL)?;
    let ro = mount_state::flags_read_only(flags, data);
    entry.state.set_read_only(ro);
    entry.options = mount_state::build_options_string(flags, data);
    Ok(())
}

pub(crate) fn move_mount_entry(old_target: &str, new_target: &str) -> LxResult<()> {
    let old_target = normalize_mount_target(old_target);
    let new_target = normalize_mount_target(new_target);
    let mut mounts = MOUNT_TABLE.lock();
    let entry = mounts
        .iter_mut()
        .find(|m| m.target == old_target)
        .ok_or(LxError::EINVAL)?;
    entry.target = new_target;
    Ok(())
}

fn normalize_mount_target(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() || path == "/" {
        String::from("/")
    } else {
        String::from(path.trim_end_matches('/'))
    }
}

pub(crate) fn proc_mounts_content() -> String {
    let mounts = MOUNT_TABLE.lock();
    let mut out = String::new();
    for m in mounts.iter() {
        let _ = writeln!(
            out,
            "{} {} {} {} 0 0",
            m.source, m.target, m.fstype, m.options
        );
    }
    out
}

/// EventBus mask matching a poll interest set: readable/writable as
/// requested, plus error/close — a hangup must always wake a poller
/// regardless of what it asked for (POLLERR/POLLHUP semantics).
pub fn poll_events_to_bus_mask(events: PollEvents) -> crate::sync::Event {
    use crate::sync::Event;
    let mut mask = Event::ERROR | Event::CLOSED;
    if events.contains(PollEvents::IN) {
        mask |= Event::READABLE;
    }
    if events.contains(PollEvents::OUT) {
        mask |= Event::WRITABLE;
    }
    if !events.intersects(PollEvents::IN | PollEvents::OUT) {
        // Error/hup-only interest (or an empty set): any transition may
        // matter to the poller's re-scan.
        mask |= Event::READABLE | Event::WRITABLE;
    }
    mask
}

#[async_trait]
/// Generic file interface
///
/// - Normal file, Directory
/// - Socket
/// - Epoll instance
pub trait FileLike: KernelObject + downcast_rs::DowncastSync {
    /// Returns open flags.
    fn flags(&self) -> OpenFlags;
    /// Set open flags.
    fn set_flags(&self, f: OpenFlags) -> LxResult;
    /// Duplicate the file.
    fn dup(&self) -> Arc<dyn FileLike> {
        unimplemented!()
    }
    /// read to buffer
    async fn read(&self, buf: &mut [u8]) -> LxResult<usize>;
    /// write from buffer
    fn write(&self, buf: &[u8]) -> LxResult<usize>;
    /// read to buffer at given offset
    async fn read_at(&self, offset: u64, buf: &mut [u8]) -> LxResult<usize>;
    /// write from buffer at given offset
    fn write_at(&self, _offset: u64, _buf: &[u8]) -> LxResult<usize> {
        Err(LxError::ENOSYS)
    }
    /// reposition the file offset. Default: not seekable (`ESPIPE`), like a
    /// pipe/socket. Seekable objects (regular files, dma-bufs whose size Mesa
    /// probes with `lseek(SEEK_END)`) override this.
    fn seek(&self, _pos: SeekFrom) -> LxResult<u64> {
        Err(LxError::ESPIPE)
    }
    /// wait for some event on a file descriptor
    fn poll(&self, events: PollEvents) -> LxResult<PollStatus>;
    /// wait for some event on a file descriptor use async
    async fn async_poll(&self, events: PollEvents) -> LxResult<PollStatus>;
    /// Park `waker` to fire on this file's next readiness transition relevant
    /// to `events` (data arriving, buffer space freeing, error/hangup) — a
    /// flat, synchronous registration on the file's event source.
    ///
    /// Returns `None` when this file type has no subscribable event source;
    /// poll/select/epoll must then keep their short re-poll backstop for the
    /// set containing it, exactly as before this method existed. `Some(sub)`
    /// guarantees a wake on the next transition — or an immediate wake when
    /// the events were already pending (the EventBus latches its flags and
    /// fires at subscribe time, making check-then-subscribe race-free) — and
    /// dropping `sub` unregisters the waker.
    ///
    /// This is deliberately NOT `async_poll`: nesting one boxed readiness
    /// future per watched fd inside poll/select/epoll overflowed the
    /// coroutine stack when the desktop started (see `PollFuture` and
    /// `Epoll::wait`).
    fn subscribe_readiness(
        &self,
        events: PollEvents,
        waker: &core::task::Waker,
    ) -> Option<crate::sync::ReadinessSub> {
        let _ = (events, waker);
        None
    }
    /// manipulates the underlying device parameters of special files
    fn ioctl(&self, _request: usize, _arg1: usize, _arg2: usize, _arg3: usize) -> LxResult<usize> {
        Err(LxError::ENOSYS)
    }
    /// True if this fd is an input device node (`/dev/input/mice`, `event*`).
    ///
    /// These are char devices, not terminals, so the generic `TIOCGWINSZ`
    /// fallback must not synthesize a window size for them — otherwise
    /// `isatty()` (implemented in musl as a `TIOCGWINSZ` probe) wrongly
    /// reports a tty, and kdrive/TinyX then treats the mouse as a serial
    /// port and loops forever over serial mouse protocols.
    fn is_input_device(&self) -> bool {
        false
    }
    /// True if the underlying inode is a character device (`S_IFCHR`).
    ///
    /// Used to scope tty-ish ioctl fallbacks (e.g. `TIOCGWINSZ`) so pipes,
    /// sockets and regular files get `ENOTTY` instead of a fake success that
    /// makes `isatty()` lie.
    fn is_char_device(&self) -> bool {
        false
    }
    /// Returns the [`VmObject`] representing the file with given `offset` and `len`.
    fn get_vmo(&self, _offset: usize, _len: usize) -> LxResult<Arc<VmObject>> {
        Err(LxError::ENOSYS)
    }
    /// Like [`get_vmo`](Self::get_vmo), but for `MAP_SHARED` mappings: every
    /// mapper of the same file must receive the SAME `VmObject`, so stores by
    /// one process are visible to every other mapper (the wl_shm contract —
    /// clients render into a mapped memfd and the compositor reads the pixels
    /// through its own mapping). Returns `(vmo, vmo_offset)`: the vmo may span
    /// the whole file with the caller mapping at `vmo_offset`.
    ///
    /// The default falls back to the per-call snapshot; `File` overrides it
    /// with a per-inode registry.
    fn get_vmo_shared(&self, offset: usize, len: usize) -> LxResult<(Arc<VmObject>, usize)> {
        self.get_vmo(offset, len).map(|vmo| (vmo, 0))
    }
    /// Casting between trait objects, or use crate: cast_trait_object
    fn as_socket(&self) -> LxResult<&dyn Socket> {
        Err(LxError::ENOTSOCK)
    }
}

downcast_rs::impl_downcast!(sync FileLike);

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
    warn!("[boot] create_root_fs: begin");
    // Filesystem from the boot medium (initrd / SFS). We use it to read the boot
    // `/etc/fstab` and, when an installed btrfs/ext2 ROOT partition is
    // detected, to pivot the real root onto it (similar to an initramfs
    // `switch_root`).
    let boot_mountfs = MountFS::new(rootfs);
    let boot_root = boot_mountfs.mountpoint_root_inode();

    // Block devices / partitions registered in DevFS, used to locate the root.
    let mut block_candidates: Vec<(String, Arc<dyn INode>)> = Vec::new();

    // create DevFS
    let devfs = DevFS::new();
    let devfs_root = devfs.root();
    *DEVFS_ROOT.lock() = Some(devfs_root.clone());
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
    // `/dev/shm` is a POSIX shared-memory tmpfs *directory*, not a device node:
    // `shm_open(name, O_CREAT, ...)` (used by wlroots to allocate the keyboard
    // keymap fd, and by any program using POSIX shm) creates files under
    // `/dev/shm/`. Backing it with a single RandomINode made every such create
    // fail with ENOTDIR, and a plain `add_dir` gives a *read-only* devfs dir
    // (create → ENOSYS, "cannot set keymap"). Expose a real writable RamFS by
    // inserting its root inode directly as the `shm` entry: `/dev/shm/<name>`
    // lookups then traverse into the RamFS where create/write work. (Mounting
    // via MountFS does not work here because `/dev/*` resolution uses the
    // `lookup_virtual_fs` fast path against the raw `DEVFS_ROOT`.)
    match devfs_root.add("shm", DEV_SHM_FS.root_inode()) {
        // Unique marker so a running kernel can be confirmed fresh:
        // `dmesg | grep SHM_RAMFS_V2`. If absent, an older kernel is booted.
        Ok(()) => warn!("[boot] /dev/shm: writable ramfs ready (SHM_RAMFS_V2)"),
        Err(e) => warn!("failed to mknod /dev/shm: {:?}", e),
    }
    devfs_root
        .add("tty", stdio::STDIN.clone())
        .expect("failed to mknod /dev/tty");
    // `/dev/tty0` (current VT) and `/dev/console`: an X server opens `/dev/tty0`
    // to query/allocate a VT (VT_OPENQRY) and `deallocvt` opens the console to
    // release it. The VT-management ioctls consult the active VT internally, so
    // backing both by the first VT's stdin is sufficient.
    if let Err(e) = devfs_root.add("tty0", stdio::vt_stdin(0)) {
        warn!("failed to mknod /dev/tty0: {:?}", e);
    }
    if let Err(e) = devfs_root.add("console", stdio::vt_stdin(0)) {
        warn!("failed to mknod /dev/console: {:?}", e);
    }
    // One device node per virtual terminal: /dev/tty1 .. /dev/ttyN.
    for vt in 0..kernel_hal::console::NUM_VTS {
        let name = alloc::format!("tty{}", vt + 1);
        if let Err(e) = devfs_root.add(&name, stdio::vt_stdin(vt)) {
            warn!("failed to mknod /dev/{}: {:?}", name, e);
        }
    }
    // Pseudo-terminals (`fs/pty.rs`). Absolute opens of `/dev/ptmx` and
    // `/dev/pts/N` are special-cased in `openat` against that registry. The
    // VFS nodes exist for `stat`/`ls`; do not also register `devfs::PtmxINode`
    // / `PtsDir` (different registry + EntryExist left `/dev/pts` empty).
    if let Err(e) = devfs_root.add("ptmx", Arc::new(pty::PtmxINode)) {
        warn!("failed to mknod /dev/ptmx: {:?}", e);
    }
    if let Err(e) = devfs_root.add_dir("pts") {
        warn!("failed to mkdir /dev/pts: {:?}", e);
    }
    if let Some(display) = drivers::all_display().first() {
        use devfs::FbDev;

        // Add framebuffer device at `/dev/fb0`
        if let Err(e) = devfs_root.add("fb0", Arc::new(FbDev::new(display.clone()))) {
            warn!("failed to mknod /dev/fb0: {:?}", e);
        }
    }

    // Add input devices at `/dev/input/`
    {
        use devfs::{EventDev, MiceDev};
        if !drivers::all_input().as_vec().is_empty() {
            if let Ok(input_dev) = devfs_root.add_dir("input") {
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
            } else {
                warn!("failed to mkdir /dev/input");
            }
        }
    }

    // Register DRM drivers and add DRM devices
    {
        // Reclaim a process's GEM/dumb buffers when it dies. Nothing else does:
        // the buffer pool is only shrunk by an explicit DESTROY_DUMB/GEM_CLOSE
        // ioctl, so a crashed or killed client leaked every buffer it had
        // allocated -- contiguous physical memory, up to 64 MiB apiece, charged
        // to no address space. Registered here because this is the one place
        // that already knows the DRM subsystem exists.
        fn drm_release_on_exit(pid: zircon_object::object::KoID) {
            let _ = devfs::drm::release_process(pid);
            // Same reclaim, for driver-private resources the generic GEM
            // handle table above doesn't know about (currently: NvidiaGpu's
            // nouveau-uAPI channel + everything VM_BIND'd/GEM_NEW'd into it
            // -- see `DrmScheme::nouveau_release_process`'s doc).
            if let Some(driver) = devfs::drm::get_primary_driver() {
                driver.nouveau_release_process(pid);
            }
        }
        zircon_object::task::set_process_exit_hook(drm_release_on_exit);

        // Register DRM drivers from kernel-hal
        for drm in drivers::all_drm().as_vec().iter() {
            devfs::drm::register_driver(drm.clone());
        }

        // Expose /dev/dri/card0 when there is a real DRM/GPU driver OR just a
        // framebuffer display. In the latter case the DRM scheme provides a
        // software KMS path (synthetic CRTC/connector/encoder + dumb-buffer
        // scanout) so wlroots/labwc can drive the framebuffer via legacy KMS.
        let have_drm = !drivers::all_drm().as_vec().is_empty();
        let have_display = drivers::all_display().first().is_some();
        debug!(
            "[drm] graphics inventory: drm_drivers={} display={}",
            drivers::all_drm().as_vec().len(),
            have_display
        );
        // The plain-Xorg desktop (desktop=xorg) drives the framebuffer through
        // the fbdev X driver on /dev/fb0 and needs no DRM. Worse, Xorg's platform
        // bus enumerates /dev/dri/card0 and probes it, and on this kernel's
        // software-KMS that probe HANGS — the server stalls at "Platform probe
        // for /sys/class/drm/card0" and never starts, so startx times out and
        // init respawns it forever. So when the boot selects the Xorg session,
        // do NOT create the card0 KMS node: Xorg's DRM enumeration then finds no
        // card and stays on the configured fbdev screen. The render node
        // (renderD128) is still created for software GL, and the labwc/Wayland
        // session (which genuinely drives KMS) does not set desktop=xorg and
        // keeps card0.
        if have_drm || have_display {
            if let Ok(dri_dev) = devfs_root.add_dir("dri") {
                if let Err(e) = dri_dev.add("card0", Arc::new(devfs::DrmDev::new(0))) {
                    warn!("failed to mknod /dev/dri/card0: {:?}", e);
                } else {
                    debug!("[drm] /dev/dri/card0 created (sw_kms path available)");
                }
                // Render node (major 226, minor 128) — Mesa/EGL opens this for
                // GPU-less GL/Vulkan (llvmpipe/lavapipe via the swrast DRI).
                if let Err(e) = dri_dev.add("renderD128", Arc::new(devfs::DrmDev::new(128))) {
                    warn!("failed to mknod /dev/dri/renderD128: {:?}", e);
                } else {
                    debug!("[drm] /dev/dri/renderD128 created (render node)");
                }
                // On the NVIDIA/nouveau experiment, dump which PCI device the
                // render node backs onto: if it is not the RTX (vendor 0x10de),
                // NVK's vendor filter skips the node and finds 0 Vulkan GPUs
                // without issuing a single nouveau ioctl. Gated so the QEMU path
                // stays silent.
                if zcore_drivers::display::nouveau_uapi_enabled() {
                    sysfs::log_drm_pci_backing();
                }
            } else {
                warn!("failed to mkdir /dev/dri");
            }
        } else {
            warn!("[drm] no display and no DRM driver — /dev/dri/card0 NOT created");
        }
    }

    // Add uart devices at `/dev/ttyS{i}`
    for (i, uart) in drivers::all_uart().as_vec().iter().enumerate() {
        let fname = format!("ttyS{}", i);
        if let Err(e) = devfs_root.add(&fname, Arc::new(devfs::UartDev::new(i, uart.clone()))) {
            warn!("failed to mknod /dev/{}: {:?}", &fname, e);
        }
    }

    warn!("[boot] create_root_fs: devfs ready");

    // Add block devices at `/dev/` using Linux naming conventions
    let blocks = drivers::all_block().as_vec();
    warn!(
        "[boot] create_root_fs: scanning {} block device(s)",
        blocks.len()
    );
    for (i, block) in blocks.iter().enumerate() {
        let name = block.name();
        let fname = if name.starts_with("nvme") {
            let nvme_idx = blocks[..i]
                .iter()
                .filter(|b| b.name().starts_with("nvme"))
                .count();
            format!("nvme{}n1", nvme_idx)
        } else if name.starts_with("virtio") {
            let virtio_idx = blocks[..i]
                .iter()
                .filter(|b| b.name().starts_with("virtio"))
                .count();
            let name_char = (b'a' + (virtio_idx % 26) as u8) as char;
            format!("vd{}", name_char)
        } else {
            let other_idx = blocks[..i]
                .iter()
                .filter(|b| !b.name().starts_with("nvme") && !b.name().starts_with("virtio"))
                .count();
            let name_char = (b'a' + (other_idx % 26) as u8) as char;
            format!("sd{}", name_char)
        };

        // Use i * 16 as the base index for minor numbers to leave room for partitions
        let base_index = i * 16;
        let dev = Arc::new(devfs::BlockDev::new(
            base_index,
            block.clone(),
            fname.clone(),
        ));
        let dev_dyn: Arc<dyn INode> = dev.clone();
        if let Err(e) = devfs_root.add(&fname, dev) {
            warn!("failed to mknod /dev/{}: {:?}", &fname, e);
        } else {
            block_candidates.push((fname.clone(), dev_dyn));
        }

        // Scan for partitions on this block device
        let partitions = devfs::blockdev::scan_partitions(block);
        warn!(
            "[boot] create_root_fs: /dev/{} has {} partition(s)",
            fname,
            partitions.len()
        );
        for (part_idx, &(start_block, block_count)) in partitions.iter().enumerate() {
            let part_num = part_idx + 1;
            let part_name = if fname.starts_with("nvme") {
                format!("{}p{}", fname, part_num)
            } else {
                format!("{}{}", fname, part_num)
            };
            let partition_driver = Arc::new(devfs::blockdev::PartitionBlock::new(
                block.clone(),
                format!("{}-part{}", name, part_num),
                start_block,
                block_count,
            ));
            let part_dev_index = base_index + part_num;
            let part = Arc::new(devfs::BlockDev::new(
                part_dev_index,
                partition_driver,
                part_name.clone(),
            ));
            let part_dyn: Arc<dyn INode> = part.clone();
            if let Err(e) = devfs_root.add(&part_name, part) {
                warn!("failed to mknod /dev/{}: {:?}", &part_name, e);
            } else {
                info!(
                    "Registered partition /dev/{} (start: {}, count: {})",
                    part_name, start_block, block_count
                );
                block_candidates.push((part_name.clone(), part_dyn));
            }
        }
    }

    // Decide the real root filesystem: pivot from the boot medium onto an
    // installed btrfs/ext2 ROOT partition when one is available, otherwise
    // keep the boot medium as `/`.
    warn!(
        "[boot] create_root_fs: determine_real_root ({} candidate(s))",
        block_candidates.len()
    );
    // `ROOTKEEP=1` keeps the boot medium as `/` and skips the auto-pivot
    // entirely. Without it, ANY whole-disk btrfs/ext2 among the block devices
    // is grabbed as root — which is right for an installed system but wrong the
    // moment a data disk is attached (a benchmark scratch disk, a second
    // volume): it would hijack `/`. With the flag the extra disk stays
    // unmounted for userspace to `mount` where it wants.
    let keep_boot = kernel_hal::boot::cmdline()
        .split(':')
        .any(|o| o.trim().eq_ignore_ascii_case("ROOTKEEP=1"));
    let pivot = if keep_boot {
        warn!("[boot] create_root_fs: ROOTKEEP=1, keeping boot medium as /");
        None
    } else {
        determine_real_root(&boot_root, &block_candidates)
    };
    let (rootfs, root_source, root_fstype) =
        match pivot {
            Some((fs, source, fstype)) => {
                warn!("[boot] create_root_fs: pivot onto {} ({})", source, fstype);
                (MountFS::new(fs), source, fstype)
            }
            None => {
                warn!("[boot] create_root_fs: keep boot medium as /");
                (boot_mountfs, String::from("rootfs"), "rootfs")
            }
        };
    warn!("[boot] create_root_fs: root inode");
    let root = rootfs.mountpoint_root_inode();
    reset_mount_table();
    register_mount(&root_source, "/", root_fstype, "rw", boot_mount_state());

    // mount DevFS at /dev
    let dev = resolve_mount_dir(&rootfs, &root, root_fstype, "dev", 0o666);
    warn!("[boot] create_root_fs: mount devfs on /dev");
    if let Err(e) = dev.mount(devfs) {
        warn!("[boot] create_root_fs: mount /dev failed: {:?}", e);
    } else {
        register_mount("devfs", "/dev", "devtmpfs", "rw,nosuid", boot_mount_state());
        // `/dev/shm` is served by a RamFS inserted directly into devfs (see the
        // `add("shm", ...)` above); it is not a MountFS mount, because `/dev/*`
        // lookups bypass MountFS via the `lookup_virtual_fs` fast path. Register
        // it in the mount table only so `/proc/mounts` reflects reality.
        register_mount(
            "tmpfs",
            "/dev/shm",
            "tmpfs",
            "rw,nosuid,nodev",
            boot_mount_state(),
        );
    }

    // mount RamFS at /tmp
    warn!("[boot] create_root_fs: mount /tmp");
    let ramfs = RamFS::new();
    let tmp = resolve_mount_dir(&rootfs, &root, root_fstype, "tmp", 0o666);
    if let Err(e) = tmp.mount(ramfs) {
        warn!("[boot] create_root_fs: mount /tmp failed: {:?}", e);
    } else {
        register_mount(
            "tmpfs",
            "/tmp",
            "tmpfs",
            "rw,nosuid,nodev",
            boot_mount_state(),
        );
    }

    // mount RamFS at /run (essential for DHCP clients and other daemons)
    warn!("[boot] create_root_fs: mount /run");
    let run_ramfs = RamFS::new();
    // Emulate udevd's database so libudev/libinput treat the input devices as
    // initialized and seat-assigned. With no udevd running there is no
    // /run/udev/data, and libudev's enumerate (MATCH_INITIALIZED_COMPAT)
    // excludes every device that has a /dev node but lacks an "initialized"
    // (`I:`) marker — which is why labwc's libinput backend found zero input
    // devices despite /sys/class/input being fully populated. Write the same
    // records udevd would: the `I:` initialized line plus the `seat` tag, one
    // per `/dev/input/eventN` (char major 13, minor 64+N).
    {
        use kernel_hal::drivers::prelude::CapabilityType;
        use rcore_fs::vfs::FileType;
        let devs = drivers::all_input().as_vec();
        let n_input = devs.len();
        if n_input > 0 {
            let run_root = run_ramfs.root_inode();
            let data_dir = run_root
                .create("udev", FileType::Dir, 0o755)
                .and_then(|udev| udev.create("data", FileType::Dir, 0o755));
            if let Ok(data_dir) = data_dir {
                // Matches EVDEV_MAJOR / EVDEV_EVENT_MINOR_BASE in `sysfs.rs`.
                const EVDEV_MAJOR: usize = 13;
                const EVDEV_EVENT_MINOR_BASE: usize = 64;
                for (id, dev) in devs.iter().enumerate() {
                    // libinput's `evdev_configure_device` ignores any device
                    // tagged only `ID_INPUT` with no subtype ("not tagged as
                    // supported input device") — it assigns capabilities from
                    // the udev `ID_INPUT_KEYBOARD` / `ID_INPUT_MOUSE` properties,
                    // not from the evdev bits alone. With udevd absent we must
                    // synthesise those tags, mirroring udev's `input_id` builtin:
                    // a keyboard has key events (KEY_ESC), a mouse has REL_X +
                    // REL_Y + BTN_LEFT. Without them the device is added but
                    // dead — no pointer, no keyboard.
                    let keys = dev.capability(CapabilityType::Key);
                    let rel = dev.capability(CapabilityType::RelAxis);
                    let is_keyboard = keys.contains(1); // KEY_ESC
                    let is_mouse = rel.contains(0) && rel.contains(1) && keys.contains(0x110); // REL_X,REL_Y,BTN_LEFT
                    let mut body = String::from("I:1\nE:ID_INPUT=1\n");
                    if is_keyboard {
                        body.push_str("E:ID_INPUT_KEYBOARD=1\n");
                    }
                    if is_mouse {
                        body.push_str("E:ID_INPUT_MOUSE=1\n");
                    }
                    body.push_str("G:seat\nQ:seat\nV:1\n");
                    let name = alloc::format!("c{}:{}", EVDEV_MAJOR, EVDEV_EVENT_MINOR_BASE + id);
                    match data_dir.create(&name, FileType::File, 0o644) {
                        Ok(f) => {
                            let _ = f.write_at(0, body.as_bytes());
                        }
                        Err(e) => warn!("[boot] udev db {}: {:?}", name, e),
                    }
                    warn!(
                        "[boot] udev db event{}: keyboard={} mouse={}",
                        id, is_keyboard, is_mouse
                    );
                }
                warn!(
                    "[boot] wrote {} udev db record(s) for input devices",
                    n_input
                );
            }
        }
    }
    let run = resolve_mount_dir(&rootfs, &root, root_fstype, "run", 0o755);
    if let Err(e) = run.mount(run_ramfs) {
        warn!("[boot] create_root_fs: mount /run failed: {:?}", e);
    } else {
        register_mount(
            "tmpfs",
            "/run",
            "tmpfs",
            "rw,nosuid,nodev",
            boot_mount_state(),
        );
    }

    // Ensure /var/run exists. Skip while pivoting onto an installed block
    // root (btrfs/ext2): scanning /var during early boot has stalled some
    // VBox/VDI setups, and /run is already a dedicated tmpfs mount above.
    if root_fstype != "btrfs" {
        if let Ok(var) = root.find(true, "var") {
            if var.find(true, "run").is_err() {
                var.create("run", FileType::Dir, 0o755).ok();
            }
        }
        // Keep apk's download cache off the small initramfs SFS: edge indexes
        // plus .apk blobs can exceed the free space left after zip_dir.
        warn!("[boot] create_root_fs: mount /var/cache/apk on tmpfs");
        if let Ok(var) = root.find(true, "var") {
            let cache = var.find(true, "cache").unwrap_or_else(|_| {
                var.create("cache", FileType::Dir, 0o755)
                    .expect("failed to mkdir /var/cache")
            });
            let apk_cache = cache.find(true, "apk").unwrap_or_else(|_| {
                cache
                    .create("apk", FileType::Dir, 0o755)
                    .expect("failed to mkdir /var/cache/apk")
            });
            if apk_cache.mount(RamFS::new()).is_ok() {
                register_mount(
                    "tmpfs",
                    "/var/cache/apk",
                    "tmpfs",
                    "rw,nosuid,nodev",
                    boot_mount_state(),
                );
            } else {
                warn!("[boot] create_root_fs: mount /var/cache/apk failed");
            }
        }
    }

    // mount ProcFS at /proc
    warn!("[boot] create_root_fs: mount /proc");
    let proc = resolve_mount_dir(&rootfs, &root, root_fstype, "proc", 0o755);
    if let Err(e) = proc.mount(Arc::new(ProcFS::new())) {
        warn!("[boot] create_root_fs: mount /proc failed: {:?}", e);
    } else {
        register_mount(
            "proc",
            "/proc",
            "proc",
            "rw,nosuid,nodev,noexec,relatime",
            boot_mount_state(),
        );
    }

    // mount SysFS at /sys
    warn!("[boot] create_root_fs: mount /sys");
    let sys = resolve_mount_dir(&rootfs, &root, root_fstype, "sys", 0o755);
    if let Err(e) = sys.mount(Arc::new(SysFS::new())) {
        warn!("[boot] create_root_fs: mount /sys failed: {:?}", e);
    } else {
        register_mount(
            "sysfs",
            "/sys",
            "sysfs",
            "rw,nosuid,nodev,noexec,relatime",
            boot_mount_state(),
        );
    }

    mount_ops::set_vfs_root(root.clone());
    // Defer non-root fstab mounts (/boot vfat, /home, …) until after init starts.
    // Mounting them here can stall disk boot (AHCI + SMP) before the shell appears.
    warn!("[boot] create_root_fs: done");
    root
}

/// Choose the real root filesystem, pivoting from the boot medium onto an
/// installed btrfs (or legacy ext2) ROOT partition when one is available.
///
/// Resolution order:
/// 1. `ROOT=<dev>` on the kernel command line (e.g. `ROOT=/dev/sda2`) when it
///    resolves to a real btrfs/ext2 device — a deterministic, explicit pivot.
/// 2. The root (`/`) entry of the boot medium's `/etc/fstab`, when it names a
///    real, resolvable device.
/// 3. Auto-detection: the first partition block device that passes the btrfs
///    or ext2 superblock probe and mounts cleanly (typically `/dev/sda2` on an
///    AHCI install with EFI on `sda1`).  We intentionally avoid walking
///    installed root directories here — that has stalled some VBox/VDI
///    setups.
///
/// An unresolved `ROOT=` (for instance the unpatched placeholder baked into a
/// live medium's `rboot.conf`) is ignored and we fall through to auto-detection
/// instead of staying on the boot medium.
///
/// Returns the filesystem to use as `/` together with its device path, or
/// `None` to keep the boot medium as the root.
fn determine_real_root(
    boot_root: &Arc<MNode>,
    candidates: &[(String, Arc<dyn INode>)],
) -> Option<(Arc<dyn FileSystem>, String, &'static str)> {
    // 1. An explicit `ROOT=<dev>` that resolves to a real device wins.
    let cmdline = kernel_hal::boot::cmdline();
    if let Some(dev) = parse_root_cmdline(&cmdline) {
        if let Some(inode) = lookup_candidate(candidates, dev) {
            if let Some((fs, fstype)) = open_block_root(inode) {
                info!("create_root_fs: root via ROOT={} ({})", dev, fstype);
                return Some((fs, String::from(dev), fstype));
            }
            warn!("create_root_fs: ROOT={} no es un btrfs/ext2 montable", dev);
        } else {
            info!(
                "create_root_fs: ROOT={} sin resolver; se intenta fstab/auto-detección",
                dev
            );
        }
    }

    // 2. The boot medium's fstab root entry, if it names a real device.
    warn!("[boot] determine_real_root: boot fstab");
    if let Some(res) = root_fs_from_fstab(boot_root, candidates) {
        return Some(res);
    }

    // 3. First mountable btrfs/ext2 partition (vfat EFI on sda1 fails the
    //    probes).
    for (name, inode) in root_mount_candidates(candidates) {
        warn!("[boot] determine_real_root: probe /dev/{}", name);
        if let Some((fs, fstype)) = open_block_root(inode.clone()) {
            warn!(
                "[boot] determine_real_root: pivot /dev/{} ({})",
                name, fstype
            );
            return Some((fs, format!("/dev/{}", name), fstype));
        }
    }
    None
}

/// Extract the `ROOT=` device from the kernel command line, which is a
/// `:`-separated list of `KEY=value` pairs (e.g. `LOG=info:ROOT=/dev/sda2`).
fn parse_root_cmdline(cmdline: &str) -> Option<&str> {
    for opt in cmdline.split(':') {
        let mut it = opt.trim().splitn(2, '=');
        let key = it.next().unwrap_or("").trim();
        if key.eq_ignore_ascii_case("ROOT") {
            let val = it.next().unwrap_or("").trim();
            if !val.is_empty()
                && !val.starts_with("__ECLIPSE_")
                && val != "/dev/__ECLIPSE_CMDROOTDEV"
            {
                return Some(val);
            }
        }
    }
    None
}

/// True for partition nodes (`sda2`, `nvme0n1p3`, …), false for whole disks (`sda`).
fn is_partition_candidate(name: &str) -> bool {
    name.chars().last().is_some_and(|c| c.is_ascii_digit())
}

/// Prefer partition devices when GPT/MBR children exist; probing whole disks is
/// slow and often matches garbage superblocks on protective-MBR layouts.
fn root_mount_candidates(
    candidates: &[(String, Arc<dyn INode>)],
) -> impl Iterator<Item = &(String, Arc<dyn INode>)> {
    let prefer_partitions = candidates
        .iter()
        .any(|(name, _)| is_partition_candidate(name));
    candidates
        .iter()
        .filter(move |(name, _)| !prefer_partitions || is_partition_candidate(name))
}

/// Find a registered block device whose name matches the basename of `dev`
/// (e.g. `/dev/sda2` -> `sda2`).
fn lookup_candidate(candidates: &[(String, Arc<dyn INode>)], dev: &str) -> Option<Arc<dyn INode>> {
    let want = dev.trim().rsplit('/').next()?;
    candidates
        .iter()
        .find(|(n, _)| n.as_str() == want)
        .map(|(_, i)| i.clone())
}

/// Open a block-device (or loop file) inode as a btrfs filesystem, if possible.
/// btrfs is the only on-disk root filesystem Eclipse supports; ext2 was removed.
/// Returns the filesystem together with its fstype name.
fn open_block_root(inode: Arc<dyn INode>) -> Option<(Arc<dyn FileSystem>, &'static str)> {
    let backend = block_mount::MountBackend::from_inode(inode).ok()?;
    if let block_mount::MountBackend::Block(block) = &backend {
        if !btrfs_mount::probe_btrfs_superblock(block) {
            return None;
        }
    }
    let fs = mount_ops::open_filesystem(backend, "btrfs", false).ok()?;
    Some((fs, "btrfs"))
}

/// Open the root declared by the `/` entry of the boot medium's fstab, when
/// that entry names a real, resolvable device.
fn root_fs_from_fstab(
    boot_root: &Arc<MNode>,
    candidates: &[(String, Arc<dyn INode>)],
) -> Option<(Arc<dyn FileSystem>, String, &'static str)> {
    let etc = boot_root.find(true, "etc").ok()?;
    let fstab = etc.find(true, "fstab").ok()?;
    let fstab_dyn: Arc<dyn INode> = fstab;
    let content_vec = fstab_dyn.read_as_vec().ok()?;
    let content = core::str::from_utf8(&content_vec).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 || parts[1] != "/" {
            continue;
        }
        // Only btrfs block-device roots can be mounted here.
        let fstype = mount_ops::parse_fstype(parts[2]).ok()?;
        if fstype != "btrfs" {
            return None;
        }
        let inode = lookup_candidate(candidates, parts[0])?;
        let (fs, fstype) = open_block_root(inode)?;
        return Some((fs, String::from(parts[0]), fstype));
    }
    None
}

fn resolve_or_create_dir(root: &Arc<MNode>, path: &str) -> LxResult<Arc<MNode>> {
    let mut cur = root.clone();
    for comp in path.split('/').filter(|s| !s.is_empty()) {
        cur = match cur.find(true, comp) {
            Ok(node) => node,
            Err(_) => cur
                .create(comp, FileType::Dir, 0o755)
                .map_err(LxError::from)?,
        };
    }
    Ok(cur)
}

/// Mount entries from `/etc/fstab` (except `/`). Call after init is up.
pub fn mount_vfs_fstab(root: &Arc<MNode>) {
    mount_fstab(root);
}

/// Process `/etc/fstab` using the VFS root remembered by `create_root_fs`.
///
/// Intended to be called *after* init has started (e.g. spawned as a kernel
/// task), since mounting extra filesystems (/boot/efi vfat, /home, …) does
/// blocking block-device I/O that must not run on the early-boot path before
/// the shell appears.
pub fn mount_fstab_deferred() {
    match mount_ops::vfs_root() {
        Some(root) => {
            warn!("[boot] mount_fstab_deferred: processing /etc/fstab");
            mount_fstab(&root);
        }
        None => warn!("[boot] mount_fstab_deferred: no VFS root set; skipping"),
    }
}

fn mount_fstab(root: &Arc<MNode>) {
    info!("mount_fstab: parsing /etc/fstab");
    if let Ok(etc) = root.find(true, "etc") {
        if let Ok(fstab_inode) = etc.find(true, "fstab") {
            let fstab_dyn: Arc<dyn INode> = fstab_inode;
            if let Ok(content_vec) = fstab_dyn.read_as_vec() {
                if let Ok(content) = core::str::from_utf8(&content_vec) {
                    for line in content.lines() {
                        let line = line.trim();
                        if line.is_empty() || line.starts_with('#') {
                            continue;
                        }
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() < 3 {
                            continue;
                        }
                        let source = parts[0];
                        let target = parts[1];
                        let fstype = parts[2];
                        let options = parts.get(3).copied().unwrap_or("defaults");

                        if target == "/" || target == "none" || fstype == "swap" {
                            continue;
                        }

                        // Resolve the source inode using coerced root_dyn
                        let source_rel = source.trim_start_matches('/');

                        // When the installer left an unsubstituted __ECLIPSE_EFI_DEV*
                        // placeholder for /boot/efi, try to derive the EFI partition
                        // from ROOT= on the kernel command line (e.g. ROOT=/dev/sda2 →
                        // EFI=/dev/sda1). This covers the edge case where raw-block
                        // patching of fstab did not take effect on the first boot.
                        let derived_efi: Option<String>;
                        let effective_source = if source_rel.starts_with("__ECLIPSE_EFI")
                            && target == "/boot/efi"
                        {
                            derived_efi = efi_dev_from_root_cmdline();
                            match derived_efi.as_deref() {
                                Some(dev) => {
                                    warn!(
                                        "mount_fstab: fstab EFI placeholder sin sustituir; \
                                         intentando ROOT= derivado {:?} -> {:?}",
                                        dev, target
                                    );
                                    dev
                                }
                                None => {
                                    info!(
                                        "mount_fstab: skipping unsubstituted EFI placeholder \
                                         (ROOT= no disponible para derivar EFI)"
                                    );
                                    continue;
                                }
                            }
                        } else if source_rel.starts_with("__ECLIPSE_") {
                            // Other unsubstituted placeholders: skip silently.
                            info!(
                                "mount_fstab: skipping unsubstituted placeholder source {:?} -> {:?}",
                                source, target
                            );
                            continue;
                        } else {
                            derived_efi = None;
                            source_rel
                        };

                        let effective_source_rel = effective_source.trim_start_matches('/');
                        let root_dyn: Arc<dyn INode> = root.clone();
                        let source_inode = match root_dyn.lookup_follow(effective_source_rel, 4) {
                            Ok(inode) => inode,
                            Err(e) => {
                                warn!(
                                    "mount_fstab: failed to lookup source {:?}: {:?}",
                                    effective_source, e
                                );
                                continue;
                            }
                        };

                        let backend = match block_mount::MountBackend::from_inode(source_inode) {
                            Ok(b) => b,
                            Err(e) => {
                                warn!(
                                    "mount_fstab: failed to create MountBackend for {:?}: {:?}",
                                    effective_source, e
                                );
                                continue;
                            }
                        };

                        let fstype_parsed = match mount_ops::parse_fstype(fstype) {
                            Ok(ft) => ft,
                            Err(e) => {
                                warn!("mount_fstab: unsupported fstype {:?}: {:?}", fstype, e);
                                continue;
                            }
                        };

                        let target_node = match resolve_or_create_dir(root, target) {
                            Ok(node) => node,
                            Err(e) => {
                                warn!(
                                    "mount_fstab: failed to resolve/create target {:?}: {:?}",
                                    target, e
                                );
                                continue;
                            }
                        };

                        if target_node.is_mountpoint() {
                            warn!("mount_fstab: target {:?} is already a mountpoint", target);
                            continue;
                        }

                        // Parse options for flags
                        let mut flags = 0;
                        for opt in options.split(',') {
                            match opt.trim() {
                                "ro" => flags |= mount_state::MS_RDONLY,
                                "rw" => flags &= !mount_state::MS_RDONLY,
                                "nosuid" => flags |= mount_state::MS_NOSUID,
                                "nodev" => flags |= mount_state::MS_NODEV,
                                "noexec" => flags |= mount_state::MS_NOEXEC,
                                _ => {}
                            }
                        }

                        let fs = match mount_ops::open_filesystem(
                            backend,
                            fstype_parsed,
                            mount_state::flags_read_only(flags, options),
                        ) {
                            Ok(f) => f,
                            Err(e) => {
                                warn!(
                                    "mount_fstab: failed to open filesystem for {:?}: {:?}",
                                    effective_source, e
                                );
                                continue;
                            }
                        };

                        let (fs, state) = mount_ops::prepare_fs(fs, flags, options);
                        if let Err(e) = target_node.mount(fs) {
                            warn!(
                                "mount_fstab: failed to mount {:?} to {:?}: {:?}",
                                effective_source, target, e
                            );
                            continue;
                        }

                        let mount_source = derived_efi.as_deref().unwrap_or(source);
                        let opts = mount_state::build_options_string(flags, options);
                        register_mount(mount_source, target, fstype_parsed, &opts, state);
                        info!(
                            "mount_fstab: successfully mounted {:?} to {:?}",
                            mount_source, target
                        );
                    }
                }
            }
        }
    }
}

/// Derive the EFI partition path from `ROOT=` on the kernel command line.
///
/// `ROOT=` names the installed ext2 root (e.g. `/dev/sda2`). On a standard
/// Eclipse OS layout the EFI system partition is always partition 1 on the
/// same disk (e.g. `/dev/sda1`, `/dev/nvme0n1p1`, `/dev/vda1`).
///
/// Returns `None` when `ROOT=` is absent, unresolved (still a placeholder),
/// or cannot be mapped to a partition-1 path.
fn efi_dev_from_root_cmdline() -> Option<String> {
    let cmdline = kernel_hal::boot::cmdline();
    let root_dev = parse_root_cmdline(&cmdline)?;
    // Strip trailing partition number. For NVMe paths (…p2) strip the 'p'
    // separator too; for sda/vda paths the separator is implicit.
    let without_digits = root_dev.trim_end_matches(|c: char| c.is_ascii_digit());
    if without_digits.len() == root_dev.len() {
        // No trailing digits — not a partition path we can map.
        return None;
    }
    if let Some(stem) = without_digits.strip_suffix('p') {
        // NVMe style: /dev/nvme0n1p2 → /dev/nvme0n1p1
        Some(format!("{}p1", stem))
    } else {
        // SATA/virtio style: /dev/sda2 → /dev/sda1, /dev/vda2 → /dev/vda1
        Some(format!("{}1", without_digits))
    }
}

pub use mount_ops::{mount_fs, umount_fs};

/// VFS root for kernel services that need to read `/etc/*` (e.g. DNS/hosts).
pub fn dns_vfs_root() -> Option<Arc<dyn INode>> {
    mount_ops::vfs_root().map(|root| root as Arc<dyn INode>)
}

/// Per-inode cache of executable file images, keyed by inode identity.
///
/// [`INodeExt::read_as_vmo`] reads a whole file off the filesystem and copies it
/// into a freshly allocated VMO. On the `execve` hot path that runs for *every*
/// spawn, so a shell launching commands — or `apk` running hundreds of helper
/// processes — re-reads and re-copies the same handful of binaries (plus the
/// `ld-musl` interpreter, loaded on every dynamically-linked exec) over and over.
/// Each load is CPU work proportional to the binary size; on real hardware this
/// pegged every core for hours during a large `apk` transaction.
///
/// [`INodeExt::read_as_vmo_cached`] memoises the image so a repeatedly-exec'd
/// binary is read once and shared thereafter. Every caller (ELF parsing + the
/// loader's segment copy-*out*) treats the VMO as read-only, so handing the same
/// `Arc<VmObject>` to many processes is safe. The key includes the file size and
/// mtime, so rewriting the file changes the key and the next exec re-reads it.
type ElfVmoKey = (usize, usize, usize, i64, i32); // (dev, inode, size, mtime.sec, mtime.nsec)

/// Skip caching files larger than this (don't pin a giant binary like
/// `libLLVM.so` in the cache for one load).
const ELF_VMO_CACHE_FILE_MAX: usize = 8 * 1024 * 1024;
/// Total committed bytes the cache may hold; the oldest entries are evicted
/// (FIFO) once a new insert would exceed it.
const ELF_VMO_CACHE_MAX_BYTES: usize = 32 * 1024 * 1024;

struct ElfVmoCache {
    map: BTreeMap<ElfVmoKey, Arc<VmObject>>,
    fifo: Vec<ElfVmoKey>,
    bytes: usize,
}

impl ElfVmoCache {
    fn get(&self, key: &ElfVmoKey) -> Option<Arc<VmObject>> {
        self.map.get(key).cloned()
    }

    fn insert(&mut self, key: ElfVmoKey, vmo: Arc<VmObject>) {
        if self.map.contains_key(&key) {
            return;
        }
        let size = key.2;
        // Evict oldest entries until this one fits within the byte budget.
        while self.bytes + size > ELF_VMO_CACHE_MAX_BYTES && !self.fifo.is_empty() {
            let old = self.fifo.remove(0);
            if self.map.remove(&old).is_some() {
                self.bytes = self.bytes.saturating_sub(old.2);
            }
        }
        self.fifo.push(key);
        self.map.insert(key, vmo);
        self.bytes += size;
    }
}

lazy_static! {
    static ref ELF_VMO_CACHE: Mutex<ElfVmoCache> = Mutex::new(ElfVmoCache {
        map: BTreeMap::new(),
        fifo: Vec::new(),
        bytes: 0,
    });
}

/// extension for INode
pub trait INodeExt {
    /// similar to read, but return a u8 vector
    fn read_as_vec(&self) -> Result<Vec<u8>>;
    /// read to VmObject
    fn read_as_vmo(&self) -> Result<Arc<VmObject>>;
    /// Like [`read_as_vmo`](Self::read_as_vmo), but returns a shared, cached
    /// image for repeatedly-loaded executables. The returned VMO MUST be treated
    /// as read-only by the caller.
    fn read_as_vmo_cached(&self) -> Result<Arc<VmObject>>;
}

impl INodeExt for dyn INode {
    #[allow(unsafe_code, clippy::uninit_vec)]
    fn read_as_vec(&self) -> Result<Vec<u8>> {
        let size = self.metadata()?.size;
        let mut buf = Vec::with_capacity(size);
        unsafe {
            buf.set_len(size);
        }
        self.read_at(0, buf.as_mut_slice())?;
        Ok(buf)
    }

    fn read_as_vmo(&self) -> Result<Arc<VmObject>> {
        let size = self.metadata()?.size;
        let pages = (size + 0xfff) >> 12;
        let vmo = VmObject::new_paged(pages);
        let mut offset = 0;
        // Heap, not stack: a 16 KiB local here sits on the guard-page-less
        // coroutine stack during every execve ELF load (labwc/lunarbar bring-up
        // loads many binaries). That frame alone was a prime stack-smash
        // contributor — see docs/README-crash-repro.md.
        let mut buf = alloc::vec![0u8; 16384];
        while offset < size {
            let len = (size - offset).min(buf.len());
            let read_len = self.read_at(offset, &mut buf[..len])?;
            if read_len == 0 {
                break;
            }
            vmo.write(offset, &buf[..read_len])
                .map_err(|_| rcore_fs::vfs::FsError::DeviceError)?;
            offset += read_len;
        }
        vmo.set_content_size(size)
            .map_err(|_| rcore_fs::vfs::FsError::DeviceError)?;
        Ok(vmo)
    }

    fn read_as_vmo_cached(&self) -> Result<Arc<VmObject>> {
        let m = self.metadata()?;
        // Only cache regular files of a sane size; devices, pipes, empty and
        // oversized files fall through to a fresh, uncached read.
        if m.type_ != FileType::File || m.size == 0 || m.size > ELF_VMO_CACHE_FILE_MAX {
            return self.read_as_vmo();
        }
        let key: ElfVmoKey = (m.dev, m.inode, m.size, m.mtime.sec, m.mtime.nsec);
        if let Some(vmo) = ELF_VMO_CACHE.lock().get(&key) {
            return Ok(vmo);
        }
        // Read outside the cache lock so a slow filesystem read never serialises
        // other loads. A concurrent double-miss just reads twice and the second
        // `insert` is a no-op — both callers get a VMO with identical bytes.
        let vmo = self.read_as_vmo()?;
        ELF_VMO_CACHE.lock().insert(key, vmo.clone());
        Ok(vmo)
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
        self.lookup_inode_at_inner(dirfd, path, follow, FOLLOW_MAX_DEPTH)
    }

    fn lookup_inode_at_inner(
        &self,
        dirfd: FileDesc,
        path: &str,
        follow: bool,
        proc_self_exe_budget: usize,
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
            if follow {
                let exe = self.execute_path();
                // Recursion guard: if execute_path is itself the magic link
                // (a binary exec'd via execve("/proc/self/exe") before the
                // execve-side canonicalization existed, or any future path
                // that reintroduces it), recursing here would never terminate
                // — the runaway recursion overflows the guard-page-less
                // coroutine stack into neighbouring heap allocations (the
                // root cause of the `timeout -s TERM 1 sleep 5` corruption;
                // see docs/README-crash-repro.md). Fail with ELOOP like a
                // real symlink cycle instead.
                if proc_self_exe_budget == 0 || exe.is_empty() || exe == "/proc/self/exe" {
                    return Err(LxError::ELOOP);
                }
                return self.lookup_inode_at_inner(
                    FileDesc::CWD,
                    &exe,
                    true,
                    proc_self_exe_budget - 1,
                );
            }
            return Ok(Arc::new(Pseudo::new(
                &self.execute_path(),
                FileType::SymLink,
            )));
        }
        if path == "/proc/self/fd" || path == "/proc/self/fd/" {
            // "/proc/self" is the CALLING process. This used to take the Linux
            // parent's Process (via a misnamed helper that did
            // `parent.upgrade().unwrap()`). So readdir("/proc/self/fd") listed
            // the parent's descriptors: verified in QEMU, where
            //   sh -c 'exec 7>/tmp/f; ls /proc/self/fd'
            // printed 0 1 2 plus stale numbers, omitting the fd 7 that the
            // same shell could demonstrably still write through. Worse, for a
            // process that was never forked `parent` is Weak::default(), so
            // that unwrap would have panicked the kernel.
            //
            // Every caller reaches here from a syscall on behalf of the
            // current thread, so the current thread's process IS `self`; take
            // it from the HAL, which is the same pattern the signal code uses.
            // This matters beyond `ls`: libdbus's
            // _dbus_fd_set_all_close_on_exec() walks /proc/self/fd and applies
            // fcntl (or, in its _dbus_close_all() form, close) to every number
            // it reads back -- against a foreign listing that acts on the
            // wrong descriptors.
            let proc = kernel_hal::thread::get_current_thread()
                .and_then(|t| t.downcast::<zircon_object::task::Thread>().ok())
                .map(|t| t.proc().clone())
                .ok_or(LxError::ENOENT)?;
            return Ok(Arc::new(proc_self::ProcSelfFdDir { process: proc }));
        }
        let (fd_dir_path, fd_name) = split_path(path);
        if fd_dir_path == "/proc/self/fd" {
            let fd = FileDesc::try_from(fd_name)?;
            let file = self.get_file(fd)?;
            if follow {
                // Magic link: resolve to the open file itself (like Linux),
                // so execve("/proc/self/fd/N") runs the file, not the
                // symlink's path text.
                return Ok(file.inode());
            }
            return Ok(Arc::new(Pseudo::new(file.path(), FileType::SymLink)));
        }

        let follow_max_depth = if follow { FOLLOW_MAX_DEPTH } else { 0 };
        if path.starts_with('/') {
            if let Some(result) = lookup_virtual_fs(path, follow_max_depth) {
                return result;
            }
            // Absolute path on the real filesystem: the base inode is
            // irrelevant (`lookup_follow` jumps straight to the fs root for a
            // leading '/'), so skip the CWD walk entirely — it re-resolved the
            // whole working directory from the root on every lookup only to
            // throw the result away — and serve repeats from the path cache.
            // The dynamic linker and xkb/fontconfig setup of a desktop session
            // do hundreds of absolute open/stat calls over the same library
            // and data paths per process start; each was a full per-component
            // VFS walk (two String allocations + a directory scan per
            // component on SFS) before this.
            if follow {
                if let Some(inode) = dcache_get(path) {
                    return Ok(inode);
                }
            }
            let inode = self.root_inode().lookup_follow(path, follow_max_depth)?;
            if follow {
                dcache_put(path, &inode);
            }
            return Ok(inode);
        }
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

// ---------------------------------------------------------------------------
// Path (dentry) cache for absolute lookups on the real filesystem
// ---------------------------------------------------------------------------
//
// Maps a fully-resolved absolute path (symlinks followed) to its final inode.
// Only paths that reach the real-FS branch of `lookup_inode_at_inner` are ever
// cached — `/proc`, `/sys` and `/dev` are intercepted earlier by
// `lookup_virtual_fs`, so dynamic pseudo-fs content (per-process `/proc`
// entries, hotplugged device nodes) can never go stale here.
//
// Coherence is a single global epoch: every namespace-mutating operation
// (unlink/rename/mkdir/mknod/symlink/link, mount/umount, `O_CREAT` opens,
// unix-socket binds — see `dcache_invalidate` call sites in linux-syscall)
// bumps it, and a bumped epoch empties the cache on the next touch. Coarse,
// but mutations are rare next to the lookup storms this exists for (a desktop
// session start does thousands of absolute open/stat calls over immutable
// library/config paths), and correctness never depends on fine-grained
// invalidation.
mod dcache {
    use super::INode;
    use alloc::string::String;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicU64, Ordering};
    use hashbrown::HashMap;
    use lock::Mutex;

    /// Generation counter; see module doc.
    static EPOCH: AtomicU64 = AtomicU64::new(0);

    struct Inner {
        /// Epoch the map contents belong to; a mismatch clears the map.
        epoch: u64,
        map: HashMap<String, Arc<dyn INode>>,
    }

    lazy_static::lazy_static! {
        static ref CACHE: Mutex<Inner> = Mutex::new(Inner {
            epoch: 0,
            map: HashMap::new(),
        });
    }

    /// Entry bound: pins each cached inode `Arc` in memory, so keep it to the
    /// working set of a session start rather than letting it grow unbounded.
    /// 4096, not 1024: a desktop session start (ldso across ~50 libraries +
    /// xkb + fontconfig + icon themes) touches more than 1024 distinct paths,
    /// and hitting the cap drops the WHOLE cache mid-storm — exactly when the
    /// hits matter most. An inode Arc is small; 4096 entries stay in the
    /// hundreds of KiB.
    const MAX_ENTRIES: usize = 4096;

    /// Invalidate every cached path (a namespace mutation happened).
    pub fn invalidate() {
        EPOCH.fetch_add(1, Ordering::Release);
    }

    pub fn get(path: &str) -> Option<Arc<dyn INode>> {
        let mut c = CACHE.lock();
        let now = EPOCH.load(Ordering::Acquire);
        if c.epoch != now {
            c.map.clear();
            c.epoch = now;
            return None;
        }
        c.map.get(path).cloned()
    }

    pub fn put(path: &str, inode: &Arc<dyn INode>) {
        let mut c = CACHE.lock();
        let now = EPOCH.load(Ordering::Acquire);
        if c.epoch != now {
            c.map.clear();
            c.epoch = now;
        }
        if c.map.len() >= MAX_ENTRIES {
            // Full: drop everything rather than tracking recency — refill is
            // one miss per path and the storm patterns are bursty anyway.
            c.map.clear();
        }
        c.map.insert(String::from(path), inode.clone());
    }
}

/// Drop every cached path→inode mapping. Call after any operation that
/// creates, removes, renames or re-mounts anything in the file namespace.
pub fn dcache_invalidate() {
    dcache::invalidate();
}

fn dcache_get(path: &str) -> Option<Arc<dyn INode>> {
    dcache::get(path)
}

fn dcache_put(path: &str, inode: &Arc<dyn INode>) {
    dcache::put(path, inode)
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

/// Max number of symlinks to follow when resolving a single path. Linux uses
/// 40 (`MAXSYMLINKS`). It was 1, which is enough for a lone symlink but breaks
/// any path that chains several — notably the DRM sysfs hierarchy libdrm/Mesa
/// walk (`/sys/dev/char/226:0/device` -> PCI dir -> `subsystem`/`drm` symlinks).
/// When the budget ran out, `lookup_follow` left a symlink inode in place and
/// the next component failed with ENOTDIR ("Failed to get DRM device: Not a
/// directory").
const FOLLOW_MAX_DEPTH: usize = 40;

/// Fast path for virtual filesystems mounted at `/proc`, `/sys`, and `/dev`.
/// Avoids ext2 directory scans on every access (VBox AHCI can stall there).
fn lookup_virtual_fs(path: &str, follow_times: usize) -> Option<LxResult<Arc<dyn INode>>> {
    let path = path.trim_end_matches('/');
    if path == "/proc" || path.starts_with("/proc/") {
        return Some(procfs::lookup_path(path, follow_times).map_err(LxError::from));
    }
    if path == "/sys" || path.starts_with("/sys/") {
        return Some(sysfs::lookup_path(path, follow_times).map_err(LxError::from));
    }
    if path == "/dev" || path.starts_with("/dev/") {
        let root = DEVFS_ROOT.lock().clone()?;
        let root: Arc<dyn INode> = root;
        if path == "/dev" {
            return Some(Ok(root));
        }
        let rest = path.strip_prefix("/dev/").unwrap();
        return Some(
            root.lookup_follow(rest, follow_times)
                .map_err(LxError::from),
        );
    }
    None
}

/// Rescans and registers partitions for a block device in devfs.
pub fn rescan_partitions(
    fname: &str,
    block: &Arc<dyn zcore_drivers::scheme::BlockScheme>,
    base_index: usize,
) -> LxResult<()> {
    if let Some(devfs_root) = DEVFS_ROOT.lock().as_ref() {
        // First, remove existing partition nodes (e.g. sda1..=sda15)
        for part_num in 1..=15 {
            let part_name = if fname.starts_with("nvme") {
                format!("{}p{}", fname, part_num)
            } else {
                format!("{}{}", fname, part_num)
            };
            let _ = devfs_root.remove(&part_name);
        }

        // Now, scan partitions
        let partitions = devfs::blockdev::scan_partitions(block);
        for (part_idx, &(start_block, block_count)) in partitions.iter().enumerate() {
            let part_num = part_idx + 1;
            let part_name = if fname.starts_with("nvme") {
                format!("{}p{}", fname, part_num)
            } else {
                format!("{}{}", fname, part_num)
            };
            let partition_driver = Arc::new(devfs::blockdev::PartitionBlock::new(
                block.clone(),
                format!("{}-part{}", fname, part_num),
                start_block,
                block_count,
            ));
            let part_dev_index = base_index + part_num;
            if let Err(e) = devfs_root.add(
                &part_name,
                Arc::new(devfs::BlockDev::new(
                    part_dev_index,
                    partition_driver,
                    part_name.clone(),
                )),
            ) {
                warn!("failed to mknod /dev/{} during rescan: {:?}", &part_name, e);
            } else {
                info!(
                    "Rescanned and registered partition /dev/{} (start: {}, count: {})",
                    part_name, start_block, block_count
                );
            }
        }
    }
    Ok(())
}
