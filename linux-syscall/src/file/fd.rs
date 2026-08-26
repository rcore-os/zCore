//! File descriptor operations
//!
//! - open(at)
//! - close
//! - dup2
//! - pipe

use super::*;
use alloc::string::String;
use linux_object::fs::{SignalFd, TimerFd};
use linux_object::time::TimeSpec;

/// `struct itimerspec` for `timerfd_settime`/`timerfd_gettime`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ITimerSpec {
    it_interval: TimeSpec,
    it_value: TimeSpec,
}

impl ITimerSpec {
    fn value_ns(&self) -> u64 {
        self.it_value.sec as u64 * 1_000_000_000 + self.it_value.nsec as u64
    }
    fn interval_ns(&self) -> u64 {
        self.it_interval.sec as u64 * 1_000_000_000 + self.it_interval.nsec as u64
    }
    fn from_ns(interval_ns: u64, value_ns: u64) -> Self {
        let ts = |ns: u64| TimeSpec {
            sec: (ns / 1_000_000_000) as usize,
            nsec: (ns % 1_000_000_000) as usize,
        };
        ITimerSpec {
            it_interval: ts(interval_ns),
            it_value: ts(value_ns),
        }
    }
}

impl Syscall<'_> {
    /// `timerfd_create(2)`: a timer delivered through a readable fd. The
    /// `wl_event_loop` (libwayland) arms one for all its timers.
    pub fn sys_timerfd_create(&self, clockid: usize, flags: usize) -> SysResult {
        info!("timerfd_create: clockid={}, flags={:#x}", clockid, flags);
        const TFD_CLOEXEC: usize = 0x80000;
        const TFD_NONBLOCK: usize = 0x800;
        let mut open_flags = OpenFlags::empty();
        if flags & TFD_CLOEXEC != 0 {
            open_flags |= OpenFlags::CLOEXEC;
        }
        if flags & TFD_NONBLOCK != 0 {
            open_flags |= OpenFlags::NON_BLOCK;
        }
        let tfd = TimerFd::new(open_flags);
        let fd = self.linux_process().add_file(tfd)?;
        Ok(fd.into())
    }

    /// `timerfd_settime(2)`: arm/disarm the timer (`TFD_TIMER_ABSTIME` = bit 0).
    pub fn sys_timerfd_settime(
        &self,
        fd: FileDesc,
        flags: usize,
        new_value: UserInPtr<ITimerSpec>,
        mut old_value: UserOutPtr<ITimerSpec>,
    ) -> SysResult {
        const TFD_TIMER_ABSTIME: usize = 1;
        let file_like = self.linux_process().get_file_like(fd)?;
        let tfd = file_like.downcast_ref::<TimerFd>().ok_or(LxError::EINVAL)?;
        if !old_value.is_null() {
            let (iv, rem) = tfd.get_time();
            old_value.write(ITimerSpec::from_ns(iv, rem))?;
        }
        let v = new_value.read()?;
        info!(
            "timerfd_settime: fd={:?}, flags={:#x}, value_ns={}, interval_ns={}",
            fd,
            flags,
            v.value_ns(),
            v.interval_ns()
        );
        tfd.set_time(
            v.value_ns(),
            v.interval_ns(),
            flags & TFD_TIMER_ABSTIME != 0,
        );
        Ok(0)
    }

    /// `timerfd_gettime(2)`: report the time until the next expiration.
    pub fn sys_timerfd_gettime(
        &self,
        fd: FileDesc,
        mut curr_value: UserOutPtr<ITimerSpec>,
    ) -> SysResult {
        let file_like = self.linux_process().get_file_like(fd)?;
        let tfd = file_like.downcast_ref::<TimerFd>().ok_or(LxError::EINVAL)?;
        let (iv, rem) = tfd.get_time();
        curr_value.write(ITimerSpec::from_ns(iv, rem))?;
        Ok(0)
    }

    /// `signalfd4(2)`: accept the signals in `mask` through a readable fd. With
    /// `fd == -1` a new signalfd is created; otherwise the existing fd's mask is
    /// replaced. The caller is expected to also block those signals
    /// (`sigprocmask`) so they stay pending for the fd — which libwayland does.
    pub fn sys_signalfd4(
        &self,
        fd: FileDesc,
        mask: UserInPtr<u64>,
        _sizemask: usize,
        flags: usize,
    ) -> SysResult {
        const SFD_CLOEXEC: usize = 0x80000;
        const SFD_NONBLOCK: usize = 0x800;
        let sigmask = mask.read()?;
        info!(
            "signalfd4: fd={:?}, mask={:#x}, flags={:#x}",
            fd, sigmask, flags
        );
        let proc = self.linux_process();
        if <FileDesc as Into<i32>>::into(fd) >= 0 {
            // Update an existing signalfd's accepted-signal set.
            let file_like = proc.get_file_like(fd)?;
            let sfd = file_like
                .downcast_ref::<SignalFd>()
                .ok_or(LxError::EINVAL)?;
            sfd.set_mask(sigmask);
            return Ok(fd.into());
        }
        let mut open_flags = OpenFlags::empty();
        if flags & SFD_CLOEXEC != 0 {
            open_flags |= OpenFlags::CLOEXEC;
        }
        if flags & SFD_NONBLOCK != 0 {
            open_flags |= OpenFlags::NON_BLOCK;
        }
        let sfd = SignalFd::new(sigmask, open_flags);
        let new_fd = proc.add_file(sfd)?;
        Ok(new_fd.into())
    }
    /// Opens or creates a file, depending on the flags passed to the call. Returns an integer with the file descriptor.
    pub fn sys_open(&self, path: UserInPtr<u8>, flags: usize, mode: usize) -> SysResult {
        self.sys_openat(FileDesc::CWD, path, flags, mode)
    }

    /// open file relative to directory file descriptor
    pub fn sys_openat(
        &self,
        dir_fd: FileDesc,
        path: UserInPtr<u8>,
        flags: usize,
        mode: usize,
    ) -> SysResult {
        let proc = self.linux_process();
        let path = path.as_c_str()?;
        // hard code special path
        let path = if path == "/dev/shm/testshm" {
            "/testshm"
        } else {
            path
        };
        let flags = OpenFlags::from_bits_truncate(flags);
        info!(
            "openat: dir_fd={:?}, path={:?}, flags={:?}, mode={:#o}",
            dir_fd, path, flags, mode
        );

        // The whole resolution runs inside a closure so its many `?`/`return`
        // exit points funnel into one `ret`, which the boot-trace recorder
        // (below) sees — including the ENOENT misses that reveal how ld.so
        // probes its library search path. The closure is a zero-cost wrapper:
        // no allocation, no extra work, just structure.
        let ret: SysResult = (|| {
        // Pseudo-terminals. Opening `/dev/ptmx` mints a brand-new master (each
        // open must yield an independent PTY pair, which the generic INode open
        // path cannot express), and `/dev/pts/N` resolves to the matching slave
        // from the live PTY registry rather than a static device node.
        if path == "/dev/ptmx" {
            let inode = pty::alloc_ptmx();
            let file = File::new(inode, flags, String::from("/dev/ptmx"));
            let fd = proc.add_file(file)?;
            return Ok(fd.into());
        }
        if let Some(id) = pty::pts_id_from_path(path) {
            let inode = pty::open_pts(id).ok_or(LxError::ENXIO)?;
            let file = File::new(inode, flags, String::from(path));
            let fd = proc.add_file(file)?;
            return Ok(fd.into());
        }
        // `/dev/tty` is the *controlling terminal* of the calling process, which
        // for our per-VT shells is that process's own virtual terminal. Resolve
        // it per-caller instead of through a single shared node: otherwise a
        // background-VT shell's job-control query — `tcgetpgrp("/dev/tty")` —
        // returns the *active* VT's foreground pgrp, never equals its own pgrp,
        // and busybox spins forever on `killpg(0, SIGTTIN)` (a CPU-burning busy
        // loop on every spare VT — the dominant idle heat once the signal
        // self-deadlock is fixed).
        if path == "/dev/tty" {
            // A process RUNNING ON A PTY (the shell inside foot/alacritty) must
            // get its own pts back, not the VT. busybox ash opens /dev/tty for
            // job control, and handing it the VT reads/writes ANOTHER
            // terminal's foreground pgrp: the first pty shell's tcsetpgrp()
            // stamped its pid into the VT's global fg_pgrp, and every LATER pty
            // shell then saw that stale pid from tcgetpgrp(), never matched its
            // own pgrp, and spun forever in killpg(0, SIGTTIN) without printing
            // a prompt — foot worked exactly once per boot, then never again.
            // There is no session/ctty tracking to consult (setsid is a stub),
            // so use the fds: stdin/stdout/stderr on a pts means the caller's
            // controlling terminal is that pty.
            let pts = (0i32..3).find_map(|n| {
                let f = proc.get_file_like(FileDesc::from(n)).ok()?;
                let file = f.downcast_ref::<File>()?;
                let inode = file.inode();
                let slave = inode.as_any_ref().downcast_ref::<pty::PtySlave>()?;
                pty::open_pts(slave.pty_id())
            });
            if let Some(inode) = pts {
                let file = File::new(inode, flags, String::from("/dev/tty"));
                let fd = proc.add_file(file)?;
                return Ok(fd.into());
            }
            let inode = linux_object::fs::stdio::vt_stdin(proc.vt());
            let file = File::new(inode, flags, String::from("/dev/tty"));
            let fd = proc.add_file(file)?;
            return Ok(fd.into());
        }

        let inode = if flags.contains(OpenFlags::CREATE) {
            let (dir_path, file_name) = split_path(path);
            // relative to cwd
            let dir_inode = proc.lookup_inode_at(dir_fd, dir_path, true)?;
            let dir_metadata = dir_inode.metadata()?;
            proc.check_access(&dir_metadata, 0o3, true)?;
            match dir_inode.find(file_name) {
                Ok(file_inode) => {
                    if flags.contains(OpenFlags::EXCLUSIVE) {
                        return Err(LxError::EEXIST);
                    }
                    let metadata = file_inode.metadata()?;
                    if flags.writable() || flags.contains(OpenFlags::TRUNCATE) {
                        proc.check_access(&metadata, 0o2, true)?;
                    }
                    if flags.readable() {
                        proc.check_access(&metadata, 0o4, true)?;
                    }
                    file_inode
                }
                Err(FsError::EntryNotFound) => {
                    let create_mode = proc.apply_umask(mode as u16);
                    let inode = dir_inode.create(file_name, FileType::File, create_mode as u32)?;
                    linux_object::fs::dcache_invalidate();
                    proc.initialize_created_metadata(
                        &inode,
                        Some(&dir_metadata),
                        create_mode,
                        false,
                    )?;
                    inode
                }
                Err(e) => return Err(LxError::from(e)),
            }
        } else {
            let inode = proc.lookup_inode_at(dir_fd, path, true)?;
            let metadata = inode.metadata()?;
            if flags.readable() {
                proc.check_access(&metadata, 0o4, true)?;
            }
            if flags.writable() {
                proc.check_access(&metadata, 0o2, true)?;
            }
            inode
        };
        let metadata = inode.metadata()?;
        if metadata.type_ == FileType::Dir && flags.writable() {
            return Err(LxError::EISDIR);
        }
        if flags.contains(OpenFlags::TRUNCATE) && metadata.type_ == FileType::File {
            proc.check_access(&metadata, 0o2, true)?;
            inode.resize(0)?;
        }
        // `/dev/ptmx` is a cloning device: each open allocates a fresh PTY
        // master (and publishes its slave at `/dev/pts/N`). Prefer the
        // `fs/pty` registry (absolute opens already special-cased above); the
        // legacy `devfs::PtmxINode` path remains for any leftover node.
        let inode = if inode
            .downcast_ref::<linux_object::fs::pty::PtmxINode>()
            .is_some()
        {
            linux_object::fs::pty::alloc_ptmx()
        } else if let Some(ptmx) = inode.downcast_ref::<linux_object::fs::devfs::PtmxINode>() {
            ptmx.open_master().map_err(LxError::from)?
        } else {
            inode
        };
        let abs_path = proc.get_absolute_path(dir_fd, path)?;
        let file = File::new(inode, flags, abs_path);
        let fd = proc.add_file(file)?;
        Ok(fd.into())
        })();

        // Boot-time file-access recorder. Gated on a single relaxed atomic that
        // is false unless `BOOTTRACE=<comm>` was on the kernel command line, so
        // this is free on every open when tracing is off. When on, it records
        // this open (path + result + timestamp) for the one process whose
        // `comm` matches — the raw material for /proc/bootprofile and the
        // desktop preload list. `comm` is computed lazily inside record_open.
        if linux_object::boot_trace::enabled() {
            let pid = self.zircon_process().id();
            let code = match &ret {
                Ok(v) => (*v).min(i32::MAX as usize) as i32,
                Err(e) => -(*e as i32),
            };
            linux_object::boot_trace::record_open(
                pid,
                || {
                    let p = self.linux_process().execute_path();
                    String::from(p.rsplit('/').next().unwrap_or(p.as_str()))
                },
                path,
                code,
            );
        }
        ret
    }

    /// Closes a file descriptor, so that it no longer refers to any file and may be reused.
    pub fn sys_close(&self, fd: FileDesc) -> SysResult {
        info!("close: fd={:?}", fd);
        let proc = self.linux_process();
        // DRM diagnostics: removal of a DRM/dmabuf fd, with the pid, so a
        // stale-fd DRM ioctl can be traced to whoever closed it. debug level:
        // closing card0 is normal application behavior (X and every DRM client
        // probe-and-close at startup); available under LOG=debug when hunting.
        if let Ok(f) = proc.get_file_like(fd) {
            if let Some(desc) = linux_object::fs::drm_fd_desc(&f) {
                debug!(
                    "[drm] pid={} close(fd={:?}) of {}",
                    self.zircon_process().id(),
                    fd,
                    desc
                );
            }
        }
        proc.close_file(fd)?;
        Ok(0)
    }

    /// `close_range(2)`: act on every open descriptor in `[first, last]`.
    ///
    /// `flags` is load-bearing and must NOT be dropped:
    /// - `CLOSE_RANGE_CLOEXEC` means "MARK this range close-on-exec"; the
    ///   descriptors stay open and usable. Treating it as "close" turns a
    ///   routine hardening call (glibc, dbus, systemd, GLib all make one)
    ///   into a mass close of live fds.
    /// - `CLOSE_RANGE_UNSHARE` asks for a private fd table first; this kernel
    ///   never shares one between processes, so it is a no-op rather than an
    ///   error.
    /// - Any other bit must be `EINVAL`, which is how callers detect an old
    ///   kernel and fall back.
    pub fn sys_close_range(&self, first: usize, last: usize, flags: usize) -> SysResult {
        const CLOSE_RANGE_UNSHARE: usize = 1 << 1;
        const CLOSE_RANGE_CLOEXEC: usize = 1 << 2;
        let proc = self.linux_process();
        // Diagnostic at klog level: a mass close of a live fd table is
        // invisible at the default log level otherwise.
        kernel_hal::klog_info!(
            "[close-range] proc={:?} first={} last={} flags={:#x}",
            proc.execute_path(),
            first,
            last,
            flags
        );
        if flags & !(CLOSE_RANGE_UNSHARE | CLOSE_RANGE_CLOEXEC) != 0 || first > last {
            return Err(LxError::EINVAL);
        }
        // `FileDesc` is an i32, so the canonical `close_range(3, ~0U, 0)`
        // idiom would wrap `last` to -1 and silently match nothing. Clamp.
        let first = FileDesc::from(first.min(i32::MAX as usize));
        let last = FileDesc::from(last.min(i32::MAX as usize));
        if flags & CLOSE_RANGE_CLOEXEC != 0 {
            proc.set_range_cloexec(first, last);
        } else {
            proc.close_range(first, last);
        }
        Ok(0)
    }

    /// `dup3(2)`: like dup2, but equal descriptors are an error and
    /// `O_CLOEXEC` can be applied atomically to the new descriptor — the whole
    /// reason the syscall exists, and what the old alias-to-dup2 dropped.
    pub fn sys_dup3(&self, fd1: FileDesc, fd2: FileDesc, flags: usize) -> SysResult {
        info!("dup3: from {:?} to {:?} flags {:#o}", fd1, fd2, flags);
        const O_CLOEXEC: usize = 0o2000000;
        if fd1 == fd2 || flags & !O_CLOEXEC != 0 {
            return Err(LxError::EINVAL);
        }
        self.sys_dup2(fd1, fd2)?;
        if flags & O_CLOEXEC != 0 {
            // Per-descriptor CLOEXEC on the new fd — set in the fd table, not
            // in the File object (whose flags are only a creation-time record).
            self.linux_process().set_fd_cloexec(fd2, true)?;
        }
        Ok(fd2.into())
    }

    /// create a copy of the file descriptor oldfd.
    pub fn sys_dup2(&self, fd1: FileDesc, fd2: FileDesc) -> SysResult {
        info!("dup2: from {:?} to {:?}", fd1, fd2);
        let proc = self.linux_process();
        if fd1 == fd2 {
            let _ = proc.get_file_like(fd1)?;
            return Ok(fd2.into());
        }
        let file_like = proc.get_file_like(fd1)?.dup();
        let mut flags = file_like.flags();
        flags -= OpenFlags::CLOEXEC;
        file_like.set_flags(flags)?;
        // Atomic replace (Linux dup2 semantics). The previous close-then-insert
        // pair took the fd-table lock twice, leaving a window where fd2 was
        // absent — a concurrent syscall on fd2 in that window got a spurious
        // EBADF.
        let old = proc.replace_file(fd2, file_like)?;
        if let Some(old) = old {
            if let Some(desc) = linux_object::fs::drm_fd_desc(&old) {
                error!(
                    "[drm] pid={} dup2 clobbered fd={:?} ({})",
                    self.zircon_process().id(),
                    fd2,
                    desc
                );
            }
        }
        Ok(fd2.into())
    }

    /// create a copy of the file descriptor fd, and uses the lowest-numbered unused descriptor for the new descriptor.
    pub fn sys_dup(&self, fd1: FileDesc) -> SysResult {
        info!("dup: from {:?}", fd1);
        let proc = self.linux_process();
        let file_like = proc.get_file_like(fd1)?.dup();
        let mut flags = file_like.flags();
        flags -= OpenFlags::CLOEXEC;
        file_like.set_flags(flags)?;
        let fd2 = proc.add_file(file_like)?;
        Ok(fd2.into())
    }

    /// Creates a pipe, a unidirectional data channel that can be used for interprocess communication.
    pub fn sys_pipe(&self, fds: UserOutPtr<[i32; 2]>) -> SysResult {
        self.sys_pipe2(fds, 0)
    }

    /// Creates a pipe, a unidirectional data channel that can be used for interprocess communication.
    pub fn sys_pipe2(&self, mut fds: UserOutPtr<[i32; 2]>, flags: usize) -> SysResult {
        info!("pipe2: fds={:?}, flags: {:#x}", fds, flags);

        let proc = self.linux_process();
        let (read, write) = Pipe::create_pair();

        let base_flags =
            OpenFlags::from_bits_truncate(flags) & (OpenFlags::NON_BLOCK | OpenFlags::CLOEXEC);
        let read_fd = proc.add_file(File::new(
            Arc::new(read),
            base_flags | OpenFlags::RDONLY,
            String::from("pipe_r:[]"),
        ))?;

        let write_fd = proc.add_file(File::new(
            Arc::new(write),
            base_flags | OpenFlags::WRONLY,
            String::from("pipe_w:[]"),
        ))?;
        fds.write([read_fd.into(), write_fd.into()])?;

        info!(
            "pipe2: created rfd={:?} wfd={:?} fds={:?}",
            read_fd, write_fd, fds
        );

        Ok(0)
    }

    /// apply or remove an advisory lock on an open file
    /// TODO: handle operation
    pub fn sys_flock(&mut self, fd: FileDesc, operation: usize) -> SysResult {
        bitflags! {
            struct Operation: u8 {
                const LOCK_SH = 1;
                const LOCK_EX = 2;
                const LOCK_NB = 4;
                const LOCK_UN = 8;
            }
        }
        let operation = Operation::from_bits(operation as u8).ok_or(LxError::EINVAL)?;
        info!("flock: fd: {:?}, operation: {:?}", fd, operation);
        let proc = self.linux_process();

        proc.get_file(fd)?;
        Ok(0)
    }

    /// `memfd_create(2)`: create an anonymous in-RAM file referred to by the
    /// returned fd. Supports `ftruncate`, `mmap` and `read`/`write`; seals
    /// (`fcntl` `F_ADD_SEALS`) are accepted as no-ops. Wayland/wlroots/Mesa use
    /// it to share xkb keymaps and shm pools.
    pub fn sys_memfd_create(&self, name: UserInPtr<u8>, flags: usize) -> SysResult {
        let name = name.as_c_str().unwrap_or("memfd");
        info!("memfd_create: name={:?}, flags={:#x}", name, flags);
        let file = linux_object::fs::new_memfd(name, flags)?;
        let fd = self.linux_process().add_file(file)?;
        Ok(fd.into())
    }

    /// creates an eventfd object that can be used as an event notification mechanism by user-space applications,
    /// and by the kernel to notify user-space applications of events.
    pub fn sys_eventfd2(&self, initval: u32, flags: usize) -> SysResult {
        info!("eventfd2: initval={}, flags={:#x}", initval, flags);
        let proc = self.linux_process();
        let eventfd = EventFd::new(initval, OpenFlags::from_bits_truncate(flags));
        let fd = proc.add_file(eventfd)?;
        Ok(fd.into())
    }

    /// `inotify_init1(2)`: create an inotify instance. `flags` may carry
    /// `IN_NONBLOCK` (0o4000) / `IN_CLOEXEC` (0o2000000), sharing the
    /// `O_NONBLOCK` / `O_CLOEXEC` bit values. `inotify_init(2)` is this with
    /// flags = 0. labwc and GTK apps call this to watch their config dirs.
    pub fn sys_inotify_init1(&self, flags: usize) -> SysResult {
        info!("inotify_init1: flags={:#x}", flags);
        // Only NONBLOCK/CLOEXEC are valid; reject anything else like Linux.
        const IN_NONBLOCK: usize = 0o4000;
        const IN_CLOEXEC: usize = 0o2000000;
        if flags & !(IN_NONBLOCK | IN_CLOEXEC) != 0 {
            return Err(LxError::EINVAL);
        }
        let inotify = linux_object::fs::Inotify::new(OpenFlags::from_bits_truncate(flags));
        let fd = self.linux_process().add_file(inotify)?;
        Ok(fd.into())
    }

    /// `inotify_add_watch(2)`: add `pathname` to the watch list of the inotify
    /// instance `fd`, returning a watch descriptor.
    pub fn sys_inotify_add_watch(
        &self,
        fd: usize,
        pathname: UserInPtr<u8>,
        mask: u32,
    ) -> SysResult {
        let path = pathname.as_c_str()?;
        info!(
            "inotify_add_watch: fd={}, path={:?}, mask={:#x}",
            fd, path, mask
        );
        let file = self.linux_process().get_file_like(fd.into())?;
        let inotify = file
            .downcast_arc::<linux_object::fs::Inotify>()
            .map_err(|_| LxError::EINVAL)?;
        inotify.add_watch(path, mask)
    }

    /// `inotify_rm_watch(2)`: remove watch descriptor `wd` from inotify `fd`.
    pub fn sys_inotify_rm_watch(&self, fd: usize, wd: i32) -> SysResult {
        info!("inotify_rm_watch: fd={}, wd={}", fd, wd);
        let file = self.linux_process().get_file_like(fd.into())?;
        let inotify = file
            .downcast_arc::<linux_object::fs::Inotify>()
            .map_err(|_| LxError::EINVAL)?;
        inotify.rm_watch(wd)
    }

    /// `perf_event_open(2)`: open a performance-monitoring file descriptor.
    ///
    /// Implements software CPU-clock sampling (no hardware PMU). The returned fd
    /// supports `mmap` (ring buffer), `ioctl(ENABLE/DISABLE/...)`, `poll` and
    /// `read`; the timer tick feeds `PERF_RECORD_SAMPLE` records into the ring.
    pub fn sys_perf_event_open(
        &self,
        attr_ptr: usize,
        pid: i32,
        cpu: i32,
        group_fd: i32,
        flags: usize,
    ) -> SysResult {
        info!(
            "perf_event_open: attr={:#x} pid={} cpu={} group_fd={} flags={:#x}",
            attr_ptr, pid, cpu, group_fd, flags
        );
        if attr_ptr == 0 {
            return Err(LxError::EFAULT);
        }
        // `attr.size` is the u32 at byte offset 4; clamp to a sane window.
        let attr_size = UserInPtr::<u32>::from(attr_ptr + 4).read()? as usize;
        let attr_size = attr_size.clamp(64, 4096);
        let attr_bytes = UserInPtr::<u8>::from(attr_ptr).read_array(attr_size)?;
        let event = PerfEvent::new(&attr_bytes, pid, cpu, OpenFlags::from_bits_truncate(flags));
        let fd = self.linux_process().add_file(event)?;
        Ok(fd.into())
    }
}
