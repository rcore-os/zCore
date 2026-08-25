//! Linux Process

use crate::{
    error::{LxError, LxResult},
    fs::{File, FileDesc, FileLike, OpenFlags},
    ipc::*,
    net::SOCKET_FD,
    signal::{Signal as LinuxSignal, SignalAction},
};
use alloc::{
    boxed::Box,
    string::String,
    sync::{Arc, Weak},
    vec,
    vec::Vec,
};
use core::convert::TryFrom;
use core::sync::atomic::AtomicI32;
use hashbrown::{HashMap, HashSet};
use kernel_hal::VirtAddr;
use lock::{Mutex, MutexGuard};
use rcore_fs::vfs::{FileSystem, FileType, INode, Metadata};

use zircon_object::{
    object::{KernelObject, KoID, Signal},
    signal::Futex,
    task::{Job, Process, Status, Thread, ROOT_JOB},
    ZxError, ZxResult,
};

pub use rcore_fs::vfs::FsInfo;

/// Process extension for linux
pub trait ProcessExt {
    /// create Linux process with a fixed Linux PID (`pid`): 1 for init, or the
    /// reserved 101.. range for the per-terminal shells.
    fn create_linux(
        job: &Arc<Job>,
        rootfs: Arc<dyn FileSystem>,
        vt: usize,
        shared_root: Option<Arc<dyn INode>>,
        pid: KoID,
    ) -> ZxResult<Arc<Self>>;
    /// get linux process
    fn linux(&self) -> &LinuxProcess;
    /// Like [`linux`](Self::linux) but returns `None` instead of panicking when
    /// the extension is not a [`LinuxProcess`]. Use this on the lock-free
    /// process-walk / reaper / signal-callback paths: racing against a process
    /// that is going away (or whose extension cannot be resolved during SMP
    /// teardown churn) must degrade to "skip it", not bring down the kernel.
    fn try_linux(&self) -> Option<&LinuxProcess>;
    /// fork from current linux process
    fn fork_from(parent: &Arc<Self>, vfork: bool) -> ZxResult<Arc<Self>>;
}

const ROOT_UID: u32 = 0;
const NO_ID: u32 = u32::MAX;
const ACCESS_WRITE: u16 = 0o2;
const ACCESS_EXEC: u16 = 0o1;
const MODE_PERM_MASK: u16 = 0o7777;
const MODE_SET_UID: u16 = 0o4000;
const MODE_SET_GID: u16 = 0o2000;
const MODE_STICKY: u16 = 0o1000;

#[derive(Clone, Debug)]
pub struct Credentials {
    pub ruid: u32,
    pub euid: u32,
    pub suid: u32,
    pub rgid: u32,
    pub egid: u32,
    pub sgid: u32,
    pub groups: Vec<u32>,
    pub umask: u16,
}

impl Default for Credentials {
    fn default() -> Self {
        Self {
            ruid: ROOT_UID,
            euid: ROOT_UID,
            suid: ROOT_UID,
            rgid: ROOT_UID,
            egid: ROOT_UID,
            sgid: ROOT_UID,
            groups: vec![ROOT_UID],
            umask: 0o022,
        }
    }
}

impl ProcessExt for Process {
    fn create_linux(
        job: &Arc<Job>,
        rootfs: Arc<dyn FileSystem>,
        vt: usize,
        shared_root: Option<Arc<dyn INode>>,
        pid: KoID,
    ) -> ZxResult<Arc<Self>> {
        let linux_proc = match shared_root {
            Some(root) => LinuxProcess::with_root(root, vt),
            None => LinuxProcess::new(rootfs, vt),
        };
        // Each process is given an explicit, stable Linux PID by the boot code:
        // 1 for init and the reserved 101.. range for the per-terminal shells.
        // (Reusing PID 1 for every VT shell once made `top`/`ps` list PID 1 N
        // times and made `find_process(1)`, signals, `kill` and `/proc/1` all
        // resolve to whichever process happened to be enumerated first.)
        let proc = Process::create_with_fixed_id_ext(job, pid, "root", linux_proc)?;
        let weak_proc = Arc::downgrade(&proc);
        proc.add_signal_callback(Box::new(move |signal| {
            if signal.contains(Signal::PROCESS_TERMINATED) {
                if let Some(proc) = weak_proc.upgrade() {
                    // Reparent any still-live children to INIT before tearing
                    // this process down, so they are not stranded on a dead
                    // parent that will never `wait` for them.
                    reparent_live_children_to_init(&proc);
                    // try_linux (not linux): this callback runs from the
                    // object layer on PROCESS_TERMINATED, concurrently with
                    // SMP teardown churn. If the extension can no longer be
                    // resolved, skip the cleanup rather than panic the kernel.
                    if let Some(lp) = proc.try_linux() {
                        // Take the file table out and drop it AFTER the lock is
                        // released — file teardown can re-enter this process's
                        // accessors (see close_file).
                        let dropped_files = {
                            let mut inner = lp.inner.lock();
                            let files = core::mem::take(&mut inner.files);
                            inner.cloexec_fds.clear();
                            inner.futexes.clear();
                            inner.semaphores = Default::default();
                            inner.shm_identifiers = Default::default();
                            files
                        };
                        drop(dropped_files);
                    }
                }
                return true;
            }
            false
        }));
        Ok(proc)
    }

    fn linux(&self) -> &LinuxProcess {
        // A failed downcast here is "impossible" (every process the Linux layer
        // creates carries the LinuxProcess ext, and ext is immutable) — so if
        // it ever fires it means either a kernel-internal Zircon process leaked
        // into a Linux-only path, or the ext Box was corrupted. Name the
        // process so the report is actionable instead of a bare unwrap panic.
        self.ext()
            .downcast_ref::<LinuxProcess>()
            .unwrap_or_else(|| {
                // Enumeration says this is UNREACHABLE by construction in a
                // `linux` build: process.rs:109 (create_with_fixed_id_ext) and
                // :212 (create_with_ext, the fork path) are the only creators
                // and both pass a LinuxProcess by value; `ext` is written once
                // in the constructor, never replaced, and Process has no Drop.
                // `Process::create` (ext = ()) is `zircon`-feature only. So a
                // failure here is NOT "a kernel process wandered in" -- it is
                // the ext fat pointer having been CORRUPTED. Dump both of its
                // words: a plausible-looking vtable pointer means a structured
                // overwrite by another allocation, zeros or garbage mean a
                // spray. That distinction is what makes the next occurrence
                // actionable, so print it before dying.
                let fat: [usize; 2] = unsafe {
                    core::mem::transmute::<&dyn core::any::Any, [usize; 2]>(
                        &**self.ext() as &dyn core::any::Any
                    )
                };
                // What the field held when the process was built, snapshotted
                // before it was published to its job. Comparing against it says
                // which word moved -- and a match would mean the field was
                // never a LinuxProcess, disproving corruption entirely.
                let (born_data, born_vtable) = self.ext_born();
                // The vtable's own header identifies the concrete type without
                // a symbol table from a matching build: `size` and `align`
                // belong to the type, not to the link.
                let vt = zircon_object::task::vtable_info(fat[1]);
                // The canonical `LinuxProcess` vtable IN THIS BUILD, built from
                // a null thin pointer (never dereferenced -- only the vtable
                // word is read). If this equals the observed vtable then the
                // ext IS a LinuxProcess and the DOWNCAST is what is wrong, not
                // the field; the two TypeIds below then say why.
                let want_vtable: usize = {
                    let p: *const LinuxProcess = core::ptr::null();
                    let d: *const dyn core::any::Any = p;
                    unsafe { core::mem::transmute::<*const dyn core::any::Any, [usize; 2]>(d)[1] }
                };
                let want_id = core::any::TypeId::of::<LinuxProcess>();
                let got_id = core::any::Any::type_id(&**self.ext());
                // Ask again. `ext` is immutable, so a downcast that fails once
                // and succeeds now would mean the first read was torn -- i.e.
                // the object is being written concurrently, which is a
                // different bug from a wrong type.
                let retry = self.ext().downcast_ref::<LinuxProcess>();
                let retry_ok = retry.is_some();
                if let Some(lp) = retry {
                    // The field is written once at construction and never
                    // again, so a downcast that fails and then immediately
                    // succeeds did not see a different TYPE -- it saw an
                    // inconsistent READ of a 16-byte field. Killing the kernel
                    // over a transient read of a value we can now see is
                    // correct is strictly worse than continuing with it. Log
                    // loudly: this is a mitigation, and every line it prints is
                    // still the bug.
                    error!(
                        "[ext-glitch] Process::linux(): pid={} name={:?} downcast failed then \
                         SUCCEEDED on retry -- ext read inconsistently, not a wrong type. \
                         fat data={:#x} vtable={:#x}, at birth data={:#x} vtable={:#x}, \
                         canaries lo={:#x} hi={:#x}",
                        self.id(),
                        self.name(),
                        fat[0],
                        fat[1],
                        born_data,
                        born_vtable,
                        self.ext_canary_values().0,
                        self.ext_canary_values().1,
                    );
                    return lp;
                }
                panic!(
                    "Process::linux(): pid={} name={:?} status={:?} has no LinuxProcess ext \
                     (ext fat pointer: data={:#x} vtable={:#x} -> {:x?} (drop, size, align), \
                     LinuxProcess would be size={} align={} vtable={:#x} -> {}; \
                     TypeId want={:?} got={:?} -> {}; downcast retry={}; actual type: {}; \
                     canaries lo={:#x} hi={:#x} -> {}; \
                     at birth: data={:#x} vtable={:#x} -> {}) -- \
                     ext is immutable and always installed for Linux processes, so \
                     this means the ext was CORRUPTED, not that a kernel-internal \
                     process leaked in",
                    self.id(),
                    self.name(),
                    self.status(),
                    fat[0],
                    fat[1],
                    vt,
                    core::mem::size_of::<LinuxProcess>(),
                    core::mem::align_of::<LinuxProcess>(),
                    want_vtable,
                    if want_vtable == fat[1] {
                        "SAME vtable: the ext IS a LinuxProcess and the downcast is what fails"
                    } else {
                        "DIFFERENT vtable: the ext really is another type"
                    },
                    want_id,
                    got_id,
                    if want_id == got_id {
                        "EQUAL: downcast_ref should have succeeded"
                    } else {
                        "DIFFERENT: two type identities for one type"
                    },
                    retry_ok,
                    // Name the type that is actually there. Across boots the
                    // vtable word has been CONSTANT (0xffffff0000a655e8) while
                    // `data` varied and stayed page-aligned -- so this is one
                    // specific type being written over the field, not a spray.
                    // A `Mutex<LinuxThread>` here would mean a Thread's ext
                    // landed on a Process's, i.e. the two objects overlap.
                    if self
                        .ext()
                        .downcast_ref::<Mutex<crate::thread::LinuxThread>>()
                        .is_some()
                    {
                        "Mutex<LinuxThread> -- a THREAD's ext on a PROCESS"
                    } else if self.ext().downcast_ref::<()>().is_some() {
                        "() -- a zircon-created process"
                    } else {
                        "unrecognised"
                    },
                    self.ext_canary_values().0,
                    self.ext_canary_values().1,
                    // Intact guards => the writer hit `ext` EXACTLY, so hunt
                    // something computing that field's address. Broken guards
                    // => a wider overrun, and the side names its direction.
                    match self.ext_canaries() {
                        (true, true) => "both INTACT: a precise write to ext alone",
                        (false, true) => "LOW broken: overrun growing upward from below",
                        (true, false) => "HIGH broken: overrun growing downward from above",
                        (false, false) => "BOTH broken: wide overrun across the field",
                    },
                    born_data,
                    born_vtable,
                    match (born_data == fat[0], born_vtable == fat[1]) {
                        (true, true) =>
                            "UNCHANGED: the ext was never a LinuxProcess -- not corruption, \
                             a construction path installs the wrong type",
                        (true, false) =>
                            "VTABLE ONLY: one 8-byte store over the vtable word, data untouched",
                        (false, true) =>
                            "DATA ONLY: one 8-byte store over the data word, vtable untouched",
                        (false, false) =>
                            "BOTH words replaced: a whole fat pointer was assigned over ext",
                    },
                )
            })
    }

    fn try_linux(&self) -> Option<&LinuxProcess> {
        self.ext().downcast_ref::<LinuxProcess>()
    }

    /// [Fork] the process.
    ///
    /// [Fork]: http://man7.org/linux/man-pages/man2/fork.2.html
    fn fork_from(parent: &Arc<Self>, _vfork: bool) -> ZxResult<Arc<Self>> {
        let linux_parent = parent.linux();
        // mmap_lock, WRITE side: freeze the parent's address-space LAYOUT for
        // the entire fork — snapshot of the mapping list AND the whole copy
        // loop below. Without this, another thread of a multithreaded parent
        // (llvmpipe's JIT mprotect/mmap storm in labwc) mutates the VMAR while
        // `fork_from` walks its stale snapshot, and the child materializes an
        // address space that never existed (the Xwayland fork-child dying in
        // its first mallocs). Taken BEFORE `inner` — that is the global order
        // (see the `aspace_lock` field doc).
        let _aspace = linux_parent.aspace_lock().lock();
        let mut linux_parent_inner = linux_parent.inner.lock();
        // Child joins the parent's process group: copy the parent's *effective*
        // pgid so the inherited value is concrete even if the parent never
        // called setpgid (raw 0 → own pid). This is what makes a Ctrl-C reach a
        // child like `ping` that the shell spawned without job control.
        let parent_pgid = if linux_parent_inner.pgid == 0 {
            parent.id()
        } else {
            linux_parent_inner.pgid
        };
        // Same resolution for the session: the child joins the parent's
        // session, and an unset (`0`) parent sid means "the parent's own pid".
        let parent_sid = if linux_parent_inner.sid == 0 {
            parent.id()
        } else {
            linux_parent_inner.sid
        };
        let new_linux_proc = LinuxProcess {
            root_inode: linux_parent.root_inode.clone(),
            parent: Arc::downgrade(parent),
            vt: linux_parent.vt,
            perf: crate::perf::ProcPerf::new(),
            itimers: Default::default(),
            aspace_lock: Mutex::new(()),
            inner: Mutex::new(LinuxProcessInner {
                execute_path: linux_parent_inner.execute_path.clone(),
                cmdline: linux_parent_inner.cmdline.clone(),
                current_working_directory: linux_parent_inner.current_working_directory.clone(),
                files: linux_parent_inner.files.clone(),
                // POSIX fork(2): the child gets its own COPY of each fd's
                // FD_CLOEXEC flag — later fcntl(F_SETFD) in either process
                // must not affect the other.
                cloexec_fds: linux_parent_inner.cloexec_fds.clone(),
                signal_actions: linux_parent_inner.signal_actions.clone(),
                credentials: linux_parent_inner.credentials.clone(),
                pgid: parent_pgid,
                sid: parent_sid,
                // fork(2)/prctl(2) inheritance: no_new_privs, dumpable, the
                // execution domain and THP setting carry over; pdeathsig and
                // the subreaper attribute deliberately do not.
                no_new_privs: linux_parent_inner.no_new_privs,
                dumpable: linux_parent_inner.dumpable,
                personality: linux_parent_inner.personality,
                abi: linux_parent_inner.abi,
                thp_disable: linux_parent_inner.thp_disable,
                ..Default::default()
            }),
        };
        let new_proc = Process::create_with_ext(&parent.job(), "", new_linux_proc)?;
        // Batch the fork's cross-CPU TLB shootdowns into one, but only when
        // the parent has a single thread. That is the condition under which no
        // other CPU can be executing in the parent's address space, and so the
        // one under which the widened write-protect window cannot be observed —
        // see `VmAddressRegion::fork_from`. It is also the overwhelmingly common
        // case: a shell, or anything that forks to exec, forks single-threaded.
        // A multi-threaded parent keeps the per-mapping shootdown exactly as
        // before.
        let single_threaded = parent.thread_count() == 1;
        new_proc.vmar().fork_from(&parent.vmar(), single_threaded)?;
        // `create_with_ext` publishes the child into ROOT_JOB before its address
        // space exists and before it has any thread, so a concurrent kill can
        // terminate it inside this window. `set_status_running` refuses to
        // resurrect such a child; abandon it here rather than returning a
        // process that is "running" but already torn down and out of its job.
        // The child never ran a single user instruction, so ECANCELED-shaped
        // failure of the fork is the truthful outcome.
        if !new_proc.set_status_running() {
            return Err(ZxError::BAD_STATE);
        }
        linux_parent_inner
            .children
            .insert(new_proc.id(), new_proc.clone());

        // On termination: reparent this process's own still-live children to
        // INIT, then notify whoever reaps *this* process — its real parent
        // while alive, otherwise INIT (orphan reparenting). See `reaper_for` /
        // `reparent_live_children_to_init`.
        let parent = parent.clone();
        let weak_proc = Arc::downgrade(&new_proc);
        new_proc.add_signal_callback(Box::new(move |signal| {
            if signal.contains(Signal::PROCESS_TERMINATED) {
                if let Some(child) = weak_proc.upgrade() {
                    let exit_code = match child.status() {
                        Status::Exited(code) => code,
                        _ => 0,
                    };
                    reparent_live_children_to_init(&child);
                    // try_linux (not linux): this callback fires from the object
                    // layer on PROCESS_TERMINATED, concurrently with SMP teardown
                    // churn. A process whose extension can no longer be resolved
                    // must be skipped, not panic the kernel.
                    if let Some(lp) = child.try_linux() {
                        // Drop the file table AFTER releasing the lock — file
                        // teardown can re-enter process accessors (see
                        // close_file).
                        let dropped_files = {
                            let mut inner = lp.inner.lock();
                            let files = core::mem::take(&mut inner.files);
                            inner.cloexec_fds.clear();
                            inner.futexes.clear();
                            inner.semaphores = Default::default();
                            inner.shm_identifiers = Default::default();
                            files
                        };
                        drop(dropped_files);
                    }
                    if let Some(reaper) = reaper_for(&parent) {
                        if let Some(reaper_lp) = reaper.try_linux() {
                            reaper.signal_set(Signal::SIGCHLD);
                            reaper_lp.record_child_exit(child.id(), exit_code, child_cpu(&child));
                        }
                    }
                }
                return true;
            }
            false
        }));
        Ok(new_proc)
    }
}

/// Wait for state changes in a child of the calling process, and obtain information about
/// the child whose state has changed.
///
/// A state change is considered to be:
/// - the child terminated.
/// - the child was stopped by a signal. TODO
/// - the child was resumed by a signal. TODO
///
/// CPU usage a child had accumulated by the time it exited: what `wait4(2)`
/// reports through its rusage out-parameter and what the parent adds to its
/// `RUSAGE_CHILDREN` totals when it reaps the child.
#[derive(Debug, Default, Clone, Copy)]
pub struct ChildCpu {
    /// User-mode nanoseconds (every thread is dead by exit, so the process's
    /// dead-thread accumulator holds the complete figure).
    pub utime_ns: u64,
    /// Kernel nanoseconds from the per-process syscall accounting.
    pub stime_ns: u64,
}

/// A reaped child's zombie record as carried through reparenting: its pid and
/// the `(exit_code, cpu_usage)` pair kept until the reaper collects it.
type ReapedChild = (KoID, (i64, ChildCpu));

/// Read an exited child's final CPU usage off its still-live object.
fn child_cpu(child: &Arc<Process>) -> ChildCpu {
    ChildCpu {
        utime_ns: child.dead_threads_time(),
        stime_ns: child
            .try_linux()
            .map(|lp| lp.perf().totals().1)
            .unwrap_or(0),
    }
}

pub async fn wait_child(
    proc: &Arc<Process>,
    pid: KoID,
    nonblock: bool,
    reap: bool,
) -> LxResult<(ExitCode, ChildCpu)> {
    loop {
        {
            let mut inner = proc.linux().inner.lock();
            if let Some(&(code, cpu)) = inner.reaped_children.get(&pid) {
                if reap {
                    inner.reaped_children.remove(&pid);
                    inner.add_children_cpu(cpu);
                }
                return Ok(((code as i32) << 8, cpu));
            }
        }
        let child = {
            let inner = proc.linux().inner.lock();
            inner.children.get(&pid).cloned().ok_or(LxError::ECHILD)?
        };
        if let Status::Exited(code) = child.status() {
            let cpu = child_cpu(&child);
            if reap {
                let mut inner = proc.linux().inner.lock();
                inner.children.remove(&pid);
                inner.reaped_children.remove(&pid);
                inner.add_children_cpu(cpu);
            }
            return Ok(((code as i32) << 8, cpu));
        }
        if nonblock {
            return Err(LxError::EAGAIN);
        }
        let child_obj: Arc<dyn KernelObject> = child.clone();
        child_obj.wait_signal(Signal::PROCESS_TERMINATED).await;

        // Check again after wait
        if let Status::Exited(code) = child.status() {
            let cpu = child_cpu(&child);
            if reap {
                let mut inner = proc.linux().inner.lock();
                inner.children.remove(&pid);
                inner.reaped_children.remove(&pid);
                inner.add_children_cpu(cpu);
            }
            return Ok(((code as i32) << 8, cpu));
        }
        continue;
    }
}

/// Wait for state changes in a child of the calling process.
pub async fn wait_child_any(
    proc: &Arc<Process>,
    nonblock: bool,
    reap: bool,
) -> LxResult<(KoID, ExitCode, ChildCpu)> {
    loop {
        let mut inner = proc.linux().inner.lock();
        if inner.children.is_empty() && inner.reaped_children.is_empty() {
            return Err(LxError::ECHILD);
        }
        if let Some((pid, (code, cpu))) = inner.reaped_children.iter().next().map(|(&p, &c)| (p, c))
        {
            if reap {
                inner.reaped_children.remove(&pid);
                inner.add_children_cpu(cpu);
            }
            return Ok((pid, (code as i32) << 8, cpu));
        }
        let mut exited_pid = None;
        trace!("wait_child_any: checking {} children", inner.children.len());
        for (&pid, child) in inner.children.iter() {
            let status = child.status();
            trace!("  child {}: status={:?}", pid, status);
            if let Status::Exited(code) = status {
                exited_pid = Some((pid, code));
                break;
            }
        }
        if let Some((pid, code)) = exited_pid {
            trace!("wait_child_any: reaping child {}", pid);
            let cpu = inner.children.get(&pid).map(child_cpu).unwrap_or_default();
            if reap {
                inner.children.remove(&pid);
                inner.reaped_children.remove(&pid);
                inner.add_children_cpu(cpu);
            }
            return Ok((pid, (code as i32) << 8, cpu));
        }
        if nonblock {
            return Err(LxError::EAGAIN);
        }
        let proc_obj: Arc<dyn KernelObject> = proc.clone();
        proc_obj.signal_clear(Signal::SIGCHLD);
        // Check again after clear to avoid race
        let mut found_exited = false;
        for child in inner.children.values() {
            if let Status::Exited(_) = child.status() {
                found_exited = true;
                break;
            }
        }
        drop(inner);
        if found_exited {
            trace!("wait_child_any: found exited child after clear, continuing");
            continue;
        }
        trace!("wait_child_any: waiting for SIGCHLD");
        proc_obj.wait_signal(Signal::SIGCHLD).await;
        trace!("wait_child_any: woke up from SIGCHLD");
    }
}

/// System-call personality of a process: which operating system's ABI its
/// binary speaks. Detected from the ELF header at load time (see
/// `crate::loader`) and consulted by the trap handler to route each syscall to
/// the right translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Abi {
    /// Native Linux ABI (the default for every binary this kernel runs).
    #[default]
    Linux,
    /// FreeBSD/amd64 ABI (`ELFOSABI_FREEBSD` or a FreeBSD ABI note). Only
    /// meaningful on x86_64; other architectures always run as [`Abi::Linux`].
    Freebsd,
}

/// Linux specific process information.
pub struct LinuxProcess {
    /// The root INode of file system
    root_inode: Arc<dyn INode>,
    /// Parent process
    parent: Weak<Process>,
    /// Virtual terminal this process is attached to (its stdin/stdout VT).
    /// Used to keep background (inactive-VT) shells from busy-polling stdin for
    /// input that can only arrive on the active terminal.
    vt: usize,
    /// Inner
    inner: Mutex<LinuxProcessInner>,
    /// Per-process syscall accounting (surfaced at `/proc/<pid>/perf`).
    perf: crate::perf::ProcPerf,
    /// Interval timers (`setitimer(2)`), indexed by ITIMER_REAL/VIRTUAL/PROF.
    /// Outside `inner` so timer-wheel callbacks never contend the big lock.
    /// A fresh process starts disarmed, and `fork` deliberately does not copy
    /// this field — fork(2): "timers are not inherited by the child".
    itimers: Mutex<[crate::time::ItimerSlot; 3]>,
    /// This kernel's `mmap_lock`: serializes every MUTATION of the process's
    /// address-space LAYOUT (mmap / munmap / mprotect / mremap / brk / shmat /
    /// shmdt / execve's teardown) against `fork`.
    ///
    /// Why it exists: `VmAddressRegion::fork_from` snapshots the mapping list
    /// ONCE and then clones each mapping with no VMAR lock held (deliberately
    /// — the copy of a big process takes tens of ms and the inner locks are
    /// IRQ-off spinlocks). Its "a point-in-time snapshot is sound" argument
    /// holds only for the forking THREAD; any OTHER thread of a multithreaded
    /// parent could still mmap/mprotect/munmap DURING the copy loop, and the
    /// child then materialized an address space that never existed: split-off
    /// pieces missing (SIGSEGV on a mapping fork should have provided — the
    /// historical openrc-init mis-replication), stale geometry from `cut()`,
    /// or content torn across the loop. labwc is exactly that parent: llvmpipe
    /// JIT threads mprotect/mmap continuously (the hunter W^X storm) while
    /// wlroots forks its Xwayland server — whose child died in musl mallocng
    /// on reshaped heap mappings before ever reaching execve, so Xwayland
    /// never came up. Linux forbids the race wholesale with `mmap_lock`; this
    /// is the same rule.
    ///
    /// Lock ORDER: `aspace_lock` is taken BEFORE `inner` everywhere (fork
    /// takes it first thing; syscall paths take it before any `get_file_like`
    /// / inner access). Page FAULTS do not touch it — they never change the
    /// layout — so a fault on another thread cannot deadlock against a fork
    /// holding this.
    aspace_lock: Mutex<()>,
}

/// Linux process mut inner data
#[derive(Default)]
struct LinuxProcessInner {
    /// Execute path
    execute_path: String,
    /// argv as seen by userland (`/proc/<pid>/cmdline`)
    cmdline: Vec<String>,
    /// Environment as of the last `execve` (`/proc/<pid>/environ`)
    environ: Vec<String>,
    /// Current Working Directory
    ///
    /// Omit leading '/'.
    current_working_directory: String,
    /// file open number limit
    file_limit: RLimit,
    /// Opened files
    files: HashMap<FileDesc, Arc<dyn FileLike>>,
    /// Per-descriptor `FD_CLOEXEC` state — the set of fds the next `execve`
    /// must close. POSIX makes this a property of the DESCRIPTOR, not the open
    /// file description: `fork` copies it per-process and plain `dup` clears it
    /// on the new fd. It therefore CANNOT live in the `File` object, which is
    /// shared via `Arc` between parent and child after `fork` — storing it
    /// there let one process's `fcntl(F_SETFD)` retag another process's fd,
    /// and the execve sweep then closed fds that should have survived
    /// (observed: dbus-daemon's `--print-address` pipe dying with EBADF).
    /// Creation-time registration happens in `insert_file`/`replace_file` from
    /// the newly built object's flags; after that, only `set_fd_cloexec`
    /// (fcntl) mutates membership.
    cloexec_fds: HashSet<FileDesc>,
    /// Semaphore
    semaphores: SemProc,
    /// Share Memory
    shm_identifiers: ShmProc,
    /// Futexes
    futexes: HashMap<VirtAddr, Arc<Futex>>,
    /// Child processes
    children: HashMap<KoID, Arc<Process>>,
    /// Exit codes and final CPU usage for children already detached (freed
    /// `Arc<Process>` at exit).
    reaped_children: HashMap<KoID, (i64, ChildCpu)>,
    /// CPU totals of children this process has reaped (`wait*` with reap):
    /// what getrusage(RUSAGE_CHILDREN) and times() cutime/cstime report.
    children_utime_ns: u64,
    /// Kernel-side counterpart of `children_utime_ns`.
    children_stime_ns: u64,
    /// Process group id (job control). `0` means "unset" and resolves to the
    /// process's own pid, so a fresh session/group leader is its own group.
    /// `fork` copies the parent's *effective* pgid (children join the parent's
    /// group); `setpgid` overrides it. Terminal-generated signals (Ctrl-C/\\/Z)
    /// go to every process whose effective pgid matches the tty's foreground
    /// group — see [`send_signal_to_pgrp`].
    pgid: u64,
    /// Session id (`setsid`/`getsid`). Same convention as `pgid`: `0` means
    /// "unset" and resolves to the process's own pid. `fork` copies the
    /// parent's *effective* sid (children stay in the parent's session);
    /// `setsid` starts a fresh session with `sid == pgid == pid`.
    sid: u64,
    /// Signal delivered to this process when its parent terminates
    /// (`prctl(PR_SET_PDEATHSIG)`); `0` = none. Deliberately NOT copied on
    /// `fork` — prctl(2): "the value is cleared for the child of a fork".
    pdeathsig: u8,
    /// `prctl(PR_SET_CHILD_SUBREAPER)`: orphaned descendants are reparented to
    /// the nearest live subreaper ancestor instead of init. Like Linux, the
    /// attribute itself is not inherited across `fork`.
    child_subreaper: bool,
    /// `prctl(PR_SET_NO_NEW_PRIVS)` (Documentation/userspace-api/no_new_privs.rst):
    /// once set it can never be cleared, is inherited across fork and execve,
    /// and stops `execve` from granting setuid/setgid privilege elevation.
    no_new_privs: bool,
    /// `prctl(PR_SET_DUMPABLE)`: `None` means "never set" and reads as the
    /// Linux default `SUID_DUMP_USER` (1).
    dumpable: Option<u8>,
    /// Execution domain (`personality(2)`). `0` is `PER_LINUX`; bits like
    /// `ADDR_NO_RANDOMIZE` are stored so a later query reads back what was
    /// set. Inherited across fork and execve.
    personality: u32,
    /// `prctl(PR_SET_THP_DISABLE)`: recorded and read back; there is no
    /// transparent-hugepage machinery for it to steer.
    thp_disable: bool,
    /// Signal actions
    signal_actions: SignalActions,
    /// Program break (top of heap).
    ///
    /// Initialized to 0; set to the end of the loaded ELF image by the loader
    /// via [`LinuxProcess::set_brk`] before the first user instruction runs.
    /// Updated by `sys_brk` as the heap grows or shrinks.
    brk: usize,
    /// Upper bound of the address range actually mapped to back the heap.
    ///
    /// `sys_brk` reserves heap pages in [`BRK_CHUNK`]-sized strides instead of
    /// one VMO per user-visible grow, so the user-visible `brk` typically
    /// trails `mapped_brk`. Lazily initialised on first `sys_brk`: a value of
    /// 0 means "matches `brk`".
    mapped_brk: usize,
    /// Process credentials.
    credentials: Credentials,
    /// System-call personality (Linux vs FreeBSD). Set by the loader from the
    /// ELF header and inherited across `fork`; re-evaluated at `execve`.
    abi: Abi,
}

#[derive(Clone)]
struct SignalActions {
    table: [SignalAction; LinuxSignal::RTMAX + 1],
}

impl Default for SignalActions {
    fn default() -> Self {
        Self {
            table: [SignalAction::default(); LinuxSignal::RTMAX + 1],
        }
    }
}

/// resource limit
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct RLimit {
    /// soft limit
    pub cur: u64,
    /// hard limit
    pub max: u64,
}

impl Default for RLimit {
    fn default() -> Self {
        RLimit {
            cur: 1024,
            max: 1024,
        }
    }
}

/// The type of process exit code.
pub type ExitCode = i32;

impl LinuxProcess {
    /// Drop the live child handle and keep only the exit code (plus the
    /// child's final CPU usage, for the reaper's rusage) for a future `wait`.
    pub fn record_child_exit(&self, child_id: KoID, exit_code: i64, cpu: ChildCpu) {
        let mut inner = self.inner.lock();
        inner.children.remove(&child_id);
        inner.reaped_children.insert(child_id, (exit_code, cpu));
    }

    /// CPU totals of already-reaped children, in nanoseconds (utime, stime).
    pub fn children_cpu_ns(&self) -> (u64, u64) {
        let inner = self.inner.lock();
        (inner.children_utime_ns, inner.children_stime_ns)
    }

    /// Create a new process bound to virtual terminal `vt`, building a fresh
    /// root filesystem.
    pub fn new(rootfs: Arc<dyn FileSystem>, vt: usize) -> Self {
        Self::with_root(crate::fs::create_root_fs(rootfs), vt)
    }

    /// Create a new process reusing an already-built root filesystem (shared by
    /// `Arc`, like `fork`), bound to virtual terminal `vt`. Used to spawn the
    /// extra per-VT shells without re-scanning disks / re-mounting.
    pub fn with_root(root_inode: Arc<dyn INode>, vt: usize) -> Self {
        // fd 0/1/2 are the process's controlling terminal: the per-VT console
        // device node `/dev/tty{vt+1}`. Naming the path after the real device
        // node — rather than `/dev/stdin`, which has no devfs entry — lets
        // `/proc/self/fd/N` resolve to a node `stat()` can find. musl's
        // `ttyname()` reads that symlink and then requires `stat(path)` and
        // `fstat(fd)` to report the same (st_dev, st_ino); both resolve to the
        // same `Stdin`/`Stdout` inode, so they match and `isatty()`/`ttyname()`
        // (and the `tty` command) report the terminal instead of failing with
        // ENOTTY ("not a tty").
        let tty_path = alloc::format!("/dev/tty{}", vt + 1);
        let stdin = File::new(
            crate::fs::stdio::vt_stdin(vt),
            OpenFlags::RDONLY,
            tty_path.clone(),
        ) as Arc<dyn FileLike>;
        let stdout_dev = crate::fs::stdio::vt_stdout(vt);
        let stdout =
            File::new(stdout_dev.clone(), OpenFlags::WRONLY, tty_path.clone()) as Arc<dyn FileLike>;
        let stderr = File::new(stdout_dev, OpenFlags::WRONLY, tty_path) as Arc<dyn FileLike>;
        let mut files = HashMap::new();
        files.insert(0.into(), stdin);
        files.insert(1.into(), stdout);
        files.insert(2.into(), stderr);

        LinuxProcess {
            root_inode,
            parent: Weak::default(),
            vt,
            perf: crate::perf::ProcPerf::new(),
            itimers: Default::default(),
            aspace_lock: Mutex::new(()),
            inner: Mutex::new(LinuxProcessInner {
                files,
                ..Default::default()
            }),
        }
    }

    /// The process's `mmap_lock` (see the field doc): hold it across any
    /// address-space LAYOUT mutation, and across the whole of `fork`'s
    /// address-space copy. Taken BEFORE `inner` wherever both are needed.
    pub fn aspace_lock(&self) -> &Mutex<()> {
        &self.aspace_lock
    }

    /// Interval-timer slots (`setitimer(2)`), indexed by
    /// ITIMER_REAL (0) / ITIMER_VIRTUAL (1) / ITIMER_PROF (2).
    pub fn itimers(&self) -> &Mutex<[crate::time::ItimerSlot; 3]> {
        &self.itimers
    }

    /// Per-process syscall accounting (see [`crate::perf`]).
    pub fn perf(&self) -> &crate::perf::ProcPerf {
        &self.perf
    }

    /// The virtual terminal this process is attached to.
    pub fn vt(&self) -> usize {
        self.vt
    }

    /// Raw process-group id (`0` = unset → resolves to the process's own pid).
    pub fn pgid_raw(&self) -> u64 {
        self.inner.lock().pgid
    }

    /// Set this process's group id (`setpgid`). A `0` argument is stored as-is
    /// and resolves to the own pid; callers normally pass a concrete pgid.
    pub fn set_pgid_raw(&self, pgid: u64) {
        self.inner.lock().pgid = pgid;
    }

    /// Raw session id (`0` = unset → resolves to the process's own pid).
    pub fn sid_raw(&self) -> u64 {
        self.inner.lock().sid
    }

    /// `setsid`: make this process a session (and group) leader in one step,
    /// so both ids resolve to `pid` and job control sees a fresh group.
    pub fn become_session_leader(&self, pid: u64) {
        let mut inner = self.inner.lock();
        inner.sid = pid;
        inner.pgid = pid;
    }

    /// Parent-death signal (`prctl(PR_SET_PDEATHSIG)`); `0` = none.
    pub fn pdeathsig(&self) -> u8 {
        self.inner.lock().pdeathsig
    }

    /// Set the parent-death signal; `0` clears it.
    pub fn set_pdeathsig(&self, sig: u8) {
        self.inner.lock().pdeathsig = sig;
    }

    /// Whether this process volunteered as a child subreaper
    /// (`prctl(PR_SET_CHILD_SUBREAPER)`).
    pub fn is_child_subreaper(&self) -> bool {
        self.inner.lock().child_subreaper
    }

    /// Mark/unmark this process as a child subreaper.
    pub fn set_child_subreaper(&self, on: bool) {
        self.inner.lock().child_subreaper = on;
    }

    /// `no_new_privs` flag (see Documentation/userspace-api/no_new_privs.rst).
    pub fn no_new_privs(&self) -> bool {
        self.inner.lock().no_new_privs
    }

    /// Set `no_new_privs`. One-way: the kernel never clears it once set.
    pub fn set_no_new_privs(&self) {
        self.inner.lock().no_new_privs = true;
    }

    /// Dumpable attribute (`prctl(PR_GET_DUMPABLE)`); defaults to
    /// `SUID_DUMP_USER` (1) like Linux.
    pub fn dumpable(&self) -> u8 {
        self.inner.lock().dumpable.unwrap_or(1)
    }

    /// Set the dumpable attribute (0, 1 or 2).
    pub fn set_dumpable(&self, value: u8) {
        self.inner.lock().dumpable = Some(value);
    }

    /// Current execution domain (`personality(2)`).
    pub fn personality(&self) -> u32 {
        self.inner.lock().personality
    }

    /// Replace the execution domain, returning the previous one.
    pub fn set_personality(&self, persona: u32) -> u32 {
        let mut inner = self.inner.lock();
        core::mem::replace(&mut inner.personality, persona)
    }

    /// `PR_GET_THP_DISABLE` state.
    pub fn thp_disable(&self) -> bool {
        self.inner.lock().thp_disable
    }

    /// Record `PR_SET_THP_DISABLE`.
    pub fn set_thp_disable(&self, on: bool) {
        self.inner.lock().thp_disable = on;
    }

    /// Get futex object.
    ///
    /// Returns `None` if `uaddr` is null or not aligned for an `AtomicI32`;
    /// dereferencing such an address would otherwise fault or be undefined
    /// behaviour.
    #[allow(unsafe_code)]
    pub fn get_futex(&self, uaddr: VirtAddr) -> Option<Arc<Futex>> {
        if uaddr == 0 || !uaddr.is_multiple_of(core::mem::align_of::<AtomicI32>()) {
            return None;
        }
        let mut inner = self.inner.lock();
        Some(
            inner
                .futexes
                .entry(uaddr)
                .or_insert_with(|| {
                    let value = unsafe { &*(uaddr as *const AtomicI32) };
                    Futex::new(value)
                })
                .clone(),
        )
    }

    /// Get lowest free fd
    pub fn get_free_fd(&self) -> FileDesc {
        self.inner.lock().get_free_fd()
    }

    /// get the lowest available fd great than or equal to `start`.
    pub fn get_free_fd_from(&self, start: usize) -> FileDesc {
        self.inner.lock().get_free_fd_from(start)
    }

    /// Add a file to the file descriptor table.
    pub fn add_file(&self, file: Arc<dyn FileLike>) -> LxResult<FileDesc> {
        let inner = self.inner.lock();
        let fd = inner.get_free_fd();
        self.insert_file(inner, fd, file)
    }

    /// Add a socket to the fd table.
    pub fn add_socket(&self, file: Arc<dyn FileLike>) -> LxResult<FileDesc> {
        let inner = self.inner.lock();
        let fd = inner.get_free_fd_from(SOCKET_FD);
        self.insert_file(inner, fd, file)
    }

    /// Add a file to the file descriptor table at given `fd`.
    pub fn add_file_at(&self, fd: FileDesc, file: Arc<dyn FileLike>) -> LxResult<FileDesc> {
        let inner = self.inner.lock();
        self.insert_file(inner, fd, file)
    }

    /// Atomically replace the file at `fd` (Linux dup2 semantics): the old
    /// entry (if any) is removed and the new one inserted under a SINGLE lock
    /// acquisition. The previous close-then-insert sequence left a window in
    /// which `fd` was absent from the table, so a concurrent thread's syscall
    /// on that fd got a spurious EBADF. Returns the previously installed file,
    /// if any, so the caller can log/inspect it.
    pub fn replace_file(
        &self,
        fd: FileDesc,
        file: Arc<dyn FileLike>,
    ) -> LxResult<Option<Arc<dyn FileLike>>> {
        let mut inner = self.inner.lock();
        let old = inner.files.remove(&fd);
        // Net table size is unchanged (replace) or +1 (plain insert); apply the
        // same limit check as insert_file for the growth case.
        if old.is_none() && inner.files.len() >= inner.file_limit.cur as usize {
            return Err(LxError::EMFILE);
        }
        // Per-fd CLOEXEC: the entry is (re)created, so its close-on-exec state
        // is whatever the incoming object was created with (dup paths clear the
        // flag on the duped object first, per POSIX).
        if file.flags().close_on_exec() {
            inner.cloexec_fds.insert(fd);
        } else {
            inner.cloexec_fds.remove(&fd);
        }
        inner.files.insert(fd, file);
        Ok(old)
    }

    /// insert a file and fd into the file descriptor table
    fn insert_file(
        &self,
        mut inner: MutexGuard<LinuxProcessInner>,
        fd: FileDesc,
        file: Arc<dyn FileLike>,
    ) -> LxResult<FileDesc> {
        if inner.files.len() < inner.file_limit.cur as usize {
            // Same creation-time CLOEXEC registration as `replace_file`.
            if file.flags().close_on_exec() {
                inner.cloexec_fds.insert(fd);
            } else {
                inner.cloexec_fds.remove(&fd);
            }
            inner.files.insert(fd, file);
            Ok(fd)
        } else {
            Err(LxError::EMFILE)
        }
    }

    /// Set or clear this descriptor's `FD_CLOEXEC` flag (`fcntl(F_SETFD)`).
    /// Per-descriptor by design — never touches the shared `File` object.
    pub fn set_fd_cloexec(&self, fd: FileDesc, on: bool) -> LxResult {
        let mut inner = self.inner.lock();
        if !inner.files.contains_key(&fd) {
            return Err(LxError::EBADF);
        }
        if on {
            inner.cloexec_fds.insert(fd);
        } else {
            inner.cloexec_fds.remove(&fd);
        }
        Ok(())
    }

    /// This descriptor's `FD_CLOEXEC` flag (`fcntl(F_GETFD)`).
    pub fn fd_cloexec(&self, fd: FileDesc) -> LxResult<bool> {
        let inner = self.inner.lock();
        if !inner.files.contains_key(&fd) {
            return Err(LxError::EBADF);
        }
        Ok(inner.cloexec_fds.contains(&fd))
    }

    /// get and set file limit number
    pub fn file_limit(&self, new_limit: Option<RLimit>) -> RLimit {
        let mut inner = self.inner.lock();
        let old = inner.file_limit;
        if let Some(limit) = new_limit {
            inner.file_limit = limit;
        }
        old
    }

    /// Get the `File` with given `fd`.
    pub fn get_file(&self, fd: FileDesc) -> LxResult<Arc<File>> {
        let file = self
            .get_file_like(fd)?
            .downcast_arc::<File>()
            .map_err(|_| LxError::EBADF)?;
        Ok(file)
    }

    /*
        /// Get the `Socket` with given `fd`.
        pub fn get_socket(&self, fd: FileDesc) -> LxResult<Arc<dyn Socket>> {
            let socket = self
                .get_file_like(fd)?
                .as_socket()
            .map_err(|_| LxError::EBADF)?;
            Ok(Arc::new(socket))
        }
    */

    /// Get the `FileLike` with given `fd`.
    pub fn get_file_like(&self, fd: FileDesc) -> LxResult<Arc<dyn FileLike>> {
        let inner = self.inner.lock();
        trace!("get_file_like: {:x?}", inner.files);
        inner.files.get(&fd).cloned().ok_or(LxError::EBADF)
    }

    /// get all files
    pub fn get_files(&self) -> LxResult<HashMap<FileDesc, Arc<dyn FileLike>>> {
        let inner = self.inner.lock();
        Ok(inner.files.clone())
    }

    /// Close file descriptor `fd`.
    ///
    /// The removed file is dropped AFTER `inner` is released. Dropping the
    /// last reference runs the file's teardown, which can re-enter this very
    /// process's accessors — e.g. closing a controlling TTY delivers SIGHUP to
    /// the foreground process group, which reads `pgid_raw()` and takes the
    /// same spinlock. Seen live as a hard SMP deadlock:
    ///   [DEADLOCK cpu=1 at pgid_raw, HOLDER cpu=1 at close_file].
    pub fn close_file(&self, fd: FileDesc) -> LxResult {
        let removed = {
            let mut inner = self.inner.lock();
            inner.cloexec_fds.remove(&fd);
            inner.files.remove(&fd)
        };
        removed.map(drop).ok_or(LxError::EBADF)
    }

    /// Whether `pid` is a tracked child of this process (live or not yet reaped).
    pub fn has_child(&self, pid: KoID) -> bool {
        let inner = self.inner.lock();
        inner.children.contains_key(&pid) || inner.reaped_children.contains_key(&pid)
    }

    /// Mark every open descriptor in `[first, last]` close-on-exec.
    ///
    /// This is `close_range(2)`'s `CLOSE_RANGE_CLOEXEC` mode: the descriptors
    /// stay open and usable and only disappear at the next `execve`.
    pub fn set_range_cloexec(&self, first: FileDesc, last: FileDesc) {
        let mut inner = self.inner.lock();
        let fds: Vec<_> = inner
            .files
            .keys()
            .filter(|&&fd| fd >= first && fd <= last)
            .cloned()
            .collect();
        for fd in fds {
            inner.cloexec_fds.insert(fd);
        }
    }

    /// Close all file descriptors between `first` and `last`.
    pub fn close_range(&self, first: FileDesc, last: FileDesc) {
        // Collect the removed files and drop them only after `inner` is
        // released — see `close_file` for the re-entrancy deadlock this avoids.
        let removed: Vec<(FileDesc, Arc<dyn FileLike>)> = {
            let mut inner = self.inner.lock();
            let fds: Vec<_> = inner
                .files
                .keys()
                .filter(|&&fd| fd >= first && fd <= last)
                .cloned()
                .collect();
            fds.into_iter()
                .filter_map(|fd| {
                    inner.cloexec_fds.remove(&fd);
                    inner.files.remove(&fd).map(|f| (fd, f))
                })
                .collect()
        };
        for (fd, f) in removed {
            // DRM diagnostics: see fs::drm_fd_desc.
            if let Some(desc) = crate::fs::drm_fd_desc(&f) {
                debug!("[drm] fd {:?} ({}) closed by close_range", fd, desc);
            }
        }
    }

    /// Get root INode of the process.
    pub fn root_inode(&self) -> &Arc<dyn INode> {
        &self.root_inode
    }

    /// Get a snapshot of current credentials.
    pub fn credentials(&self) -> Credentials {
        self.inner.lock().credentials.clone()
    }

    /// Get real uid.
    pub fn uid(&self) -> u32 {
        self.inner.lock().credentials.ruid
    }

    /// Get effective uid.
    pub fn euid(&self) -> u32 {
        self.inner.lock().credentials.euid
    }

    /// Get saved uid.
    pub fn suid(&self) -> u32 {
        self.inner.lock().credentials.suid
    }

    /// Get real gid.
    pub fn gid(&self) -> u32 {
        self.inner.lock().credentials.rgid
    }

    /// Get effective gid.
    pub fn egid(&self) -> u32 {
        self.inner.lock().credentials.egid
    }

    /// Get saved gid.
    pub fn sgid(&self) -> u32 {
        self.inner.lock().credentials.sgid
    }

    /// Get supplementary groups.
    pub fn groups(&self) -> Vec<u32> {
        self.inner.lock().credentials.groups.clone()
    }

    /// Get umask.
    pub fn umask(&self) -> u16 {
        self.inner.lock().credentials.umask
    }

    /// Set umask and return the previous one.
    pub fn set_umask(&self, mask: u16) -> u16 {
        let mut inner = self.inner.lock();
        let old = inner.credentials.umask;
        inner.credentials.umask = mask & 0o777;
        old
    }

    /// Whether the current effective uid is root.
    pub fn is_superuser(&self) -> bool {
        self.euid() == ROOT_UID
    }

    /// Apply umask to file creation mode.
    pub fn apply_umask(&self, mode: u16) -> u16 {
        mode & !self.umask()
    }

    fn gid_in_groups(creds: &Credentials, gid: u32) -> bool {
        creds.egid == gid || creds.rgid == gid || creds.groups.contains(&gid)
    }

    fn allowed_uid(creds: &Credentials, uid: u32) -> bool {
        uid == creds.ruid || uid == creds.euid || uid == creds.suid
    }

    fn allowed_gid(creds: &Credentials, gid: u32) -> bool {
        gid == creds.rgid || gid == creds.egid || gid == creds.sgid
    }

    fn check_requested_access(mode: u16, requested: u16) -> bool {
        requested == 0 || (mode & requested) == requested
    }

    fn access_bits_for(creds: &Credentials, metadata: &Metadata, use_effective: bool) -> u16 {
        let uid = if use_effective {
            creds.euid
        } else {
            creds.ruid
        };
        let gid = if use_effective {
            creds.egid
        } else {
            creds.rgid
        };
        if uid == ROOT_UID {
            return metadata.mode & 0o777;
        }
        if uid == metadata.uid as u32 {
            return (metadata.mode >> 6) & 0o7;
        }
        let in_group = if use_effective {
            Self::gid_in_groups(creds, metadata.gid as u32)
        } else {
            gid == metadata.gid as u32 || creds.groups.contains(&(metadata.gid as u32))
        };
        if in_group {
            return (metadata.mode >> 3) & 0o7;
        }
        metadata.mode & 0o7
    }

    /// Check inode access against current credentials.
    pub fn check_access(
        &self,
        metadata: &Metadata,
        requested: u16,
        use_effective: bool,
    ) -> LxResult {
        let creds = self.credentials();
        let selected_uid = if use_effective {
            creds.euid
        } else {
            creds.ruid
        };
        let granted = Self::access_bits_for(&creds, metadata, use_effective);
        if selected_uid == ROOT_UID {
            // CAP_DAC_OVERRIDE semantics: root bypasses permission checks
            // except executing a non-directory with no exec bit set anywhere
            // (mode & 0o111 == 0). Directories are always searchable by root;
            // testing only the others-exec bit here used to lock root out of
            // 0700 directories (e.g. apk's /lib/apk/exec, breaking triggers).
            if requested & ACCESS_EXEC != 0
                && metadata.type_ != FileType::Dir
                && metadata.mode & 0o111 == 0
            {
                return Err(LxError::EACCES);
            }
            return Ok(());
        }
        if Self::check_requested_access(granted, requested) {
            Ok(())
        } else {
            Err(LxError::EACCES)
        }
    }

    /// Check inode access by fetching metadata first.
    pub fn check_inode_access(
        &self,
        inode: &Arc<dyn INode>,
        requested: u16,
        use_effective: bool,
    ) -> LxResult {
        let metadata = inode.metadata()?;
        self.check_access(&metadata, requested, use_effective)
    }

    /// Check parent directory mutation rights.
    pub fn check_directory_write(&self, inode: &Arc<dyn INode>) -> LxResult {
        self.check_inode_access(inode, ACCESS_WRITE | ACCESS_EXEC, true)
    }

    /// Check if sticky-directory removal/rename is allowed.
    pub fn check_sticky(&self, dir_metadata: &Metadata, target_metadata: &Metadata) -> LxResult {
        if (dir_metadata.mode & MODE_STICKY) == 0 {
            return Ok(());
        }
        let creds = self.credentials();
        if creds.euid == ROOT_UID
            || creds.euid == dir_metadata.uid as u32
            || creds.euid == target_metadata.uid as u32
        {
            Ok(())
        } else {
            Err(LxError::EPERM)
        }
    }

    /// Change mode if current process is owner or root.
    pub fn chmod_metadata(&self, metadata: &mut Metadata, mode: u16) -> LxResult {
        let creds = self.credentials();
        if creds.euid != ROOT_UID && creds.euid != metadata.uid as u32 {
            return Err(LxError::EPERM);
        }
        metadata.mode = (metadata.mode & !MODE_PERM_MASK | (mode & MODE_PERM_MASK)) as _;
        if creds.euid != ROOT_UID {
            metadata.mode &= !(MODE_SET_UID | MODE_SET_GID);
        }
        Ok(())
    }

    /// Change owner/group following a conservative POSIX-compatible policy.
    pub fn chown_metadata(&self, metadata: &mut Metadata, uid: u32, gid: u32) -> LxResult {
        let creds = self.credentials();
        let privileged = creds.euid == ROOT_UID;
        if !privileged {
            if uid != NO_ID && uid != metadata.uid as u32 {
                return Err(LxError::EPERM);
            }
            if creds.euid != metadata.uid as u32 {
                return Err(LxError::EPERM);
            }
            if gid != NO_ID && !Self::gid_in_groups(&creds, gid) {
                return Err(LxError::EPERM);
            }
        }
        if uid != NO_ID {
            metadata.uid = uid as _;
        }
        if gid != NO_ID {
            metadata.gid = gid as _;
        }
        metadata.mode &= !(MODE_SET_UID | MODE_SET_GID);
        Ok(())
    }

    /// Set owner/group for a newly created inode.
    pub fn initialize_created_metadata(
        &self,
        inode: &Arc<dyn INode>,
        parent_metadata: Option<&Metadata>,
        mode: u16,
        is_dir: bool,
    ) -> LxResult {
        let creds = self.credentials();
        let mut metadata = inode.metadata()?;
        metadata.uid = creds.euid as _;
        metadata.gid = parent_metadata
            .filter(|meta| (meta.mode & MODE_SET_GID) != 0)
            .map(|meta| meta.gid)
            .unwrap_or(creds.egid as _);
        let mut final_mode = mode & MODE_PERM_MASK;
        if let Some(parent) = parent_metadata {
            if (parent.mode & MODE_SET_GID) != 0 && is_dir {
                final_mode |= MODE_SET_GID;
            }
        }
        metadata.mode = (metadata.mode & !MODE_PERM_MASK | final_mode) as _;
        inode.set_metadata(&metadata)?;
        Ok(())
    }

    /// Apply setuid/setgid exec transitions.
    pub fn apply_exec_metadata(&self, metadata: &Metadata) {
        let mut inner = self.inner.lock();
        // no_new_privs (Documentation/userspace-api/no_new_privs.rst): execve
        // must not grant privileges the process could not have gained on its
        // own — setuid/setgid bits on the image are simply not honoured.
        if inner.no_new_privs {
            return;
        }
        if (metadata.mode & MODE_SET_UID) != 0 {
            inner.credentials.euid = metadata.uid as u32;
            inner.credentials.suid = metadata.uid as u32;
        }
        if (metadata.mode & MODE_SET_GID) != 0 {
            inner.credentials.egid = metadata.gid as u32;
            inner.credentials.sgid = metadata.gid as u32;
        }
    }

    /// Set supplementary groups.
    pub fn set_groups(&self, groups: Vec<u32>) {
        self.inner.lock().credentials.groups = groups;
    }

    /// Set uid according to current privileges.
    pub fn set_uid(&self, uid: u32) -> LxResult {
        let mut inner = self.inner.lock();
        let privileged = inner.credentials.euid == ROOT_UID;
        if privileged {
            inner.credentials.ruid = uid;
            inner.credentials.euid = uid;
            inner.credentials.suid = uid;
            return Ok(());
        }
        if Self::allowed_uid(&inner.credentials, uid) {
            inner.credentials.euid = uid;
            Ok(())
        } else {
            Err(LxError::EPERM)
        }
    }

    /// Set gid according to current privileges.
    pub fn set_gid(&self, gid: u32) -> LxResult {
        let mut inner = self.inner.lock();
        let privileged = inner.credentials.euid == ROOT_UID;
        if privileged {
            inner.credentials.rgid = gid;
            inner.credentials.egid = gid;
            inner.credentials.sgid = gid;
            return Ok(());
        }
        if Self::allowed_gid(&inner.credentials, gid) {
            inner.credentials.egid = gid;
            Ok(())
        } else {
            Err(LxError::EPERM)
        }
    }

    /// Set real/effective uid.
    pub fn set_reuid(&self, ruid: u32, euid: u32) -> LxResult {
        let mut inner = self.inner.lock();
        let privileged = inner.credentials.euid == ROOT_UID;
        if !privileged {
            if ruid != NO_ID && !Self::allowed_uid(&inner.credentials, ruid) {
                return Err(LxError::EPERM);
            }
            if euid != NO_ID && !Self::allowed_uid(&inner.credentials, euid) {
                return Err(LxError::EPERM);
            }
        }
        let old_ruid = inner.credentials.ruid;
        if ruid != NO_ID {
            inner.credentials.ruid = ruid;
        }
        if euid != NO_ID {
            inner.credentials.euid = euid;
        }
        if privileged || ruid != NO_ID || (euid != NO_ID && euid != old_ruid) {
            inner.credentials.suid = inner.credentials.euid;
        }
        Ok(())
    }

    /// Set real/effective gid.
    pub fn set_regid(&self, rgid: u32, egid: u32) -> LxResult {
        let mut inner = self.inner.lock();
        let privileged = inner.credentials.euid == ROOT_UID;
        if !privileged {
            if rgid != NO_ID && !Self::allowed_gid(&inner.credentials, rgid) {
                return Err(LxError::EPERM);
            }
            if egid != NO_ID && !Self::allowed_gid(&inner.credentials, egid) {
                return Err(LxError::EPERM);
            }
        }
        let old_rgid = inner.credentials.rgid;
        if rgid != NO_ID {
            inner.credentials.rgid = rgid;
        }
        if egid != NO_ID {
            inner.credentials.egid = egid;
        }
        if privileged || rgid != NO_ID || (egid != NO_ID && egid != old_rgid) {
            inner.credentials.sgid = inner.credentials.egid;
        }
        Ok(())
    }

    /// Set real/effective/saved uid.
    pub fn set_resuid(&self, ruid: u32, euid: u32, suid: u32) -> LxResult {
        let mut inner = self.inner.lock();
        let privileged = inner.credentials.euid == ROOT_UID;
        if !privileged {
            for uid in [ruid, euid, suid] {
                if uid != NO_ID && !Self::allowed_uid(&inner.credentials, uid) {
                    return Err(LxError::EPERM);
                }
            }
        }
        if ruid != NO_ID {
            inner.credentials.ruid = ruid;
        }
        if euid != NO_ID {
            inner.credentials.euid = euid;
        }
        if suid != NO_ID {
            inner.credentials.suid = suid;
        }
        Ok(())
    }

    /// Set real/effective/saved gid.
    pub fn set_resgid(&self, rgid: u32, egid: u32, sgid: u32) -> LxResult {
        let mut inner = self.inner.lock();
        let privileged = inner.credentials.euid == ROOT_UID;
        if !privileged {
            for gid in [rgid, egid, sgid] {
                if gid != NO_ID && !Self::allowed_gid(&inner.credentials, gid) {
                    return Err(LxError::EPERM);
                }
            }
        }
        if rgid != NO_ID {
            inner.credentials.rgid = rgid;
        }
        if egid != NO_ID {
            inner.credentials.egid = egid;
        }
        if sgid != NO_ID {
            inner.credentials.sgid = sgid;
        }
        Ok(())
    }

    /// Get parent process.
    pub fn parent(&self) -> Option<Arc<Process>> {
        self.parent.upgrade()
    }

    /// Get current working directory.
    pub fn current_working_directory(&self) -> String {
        String::from("/") + &self.inner.lock().current_working_directory
    }

    /// Get absolute path from dirfd and relative path.
    pub fn get_absolute_path(&self, dirfd: FileDesc, path: &str) -> LxResult<String> {
        if path.is_empty() {
            return Ok(String::from("/"));
        }
        let base_path = if path.starts_with('/') {
            String::new()
        } else if dirfd == FileDesc::CWD {
            self.inner.lock().current_working_directory.clone()
        } else {
            let file = self.get_file(dirfd)?;
            let file_path = file.path().clone();
            if let Some(stripped) = file_path.strip_prefix('/') {
                String::from(stripped)
            } else {
                file_path
            }
        };
        let mut cwd_vec: Vec<_> = base_path.split('/').filter(|x| !x.is_empty()).collect();
        for seg in path.split('/') {
            match seg {
                ".." => {
                    cwd_vec.pop();
                }
                "." | "" => {}
                _ => cwd_vec.push(seg),
            }
        }
        Ok(String::from("/") + &cwd_vec.join("/"))
    }

    /// Change working directory.
    pub fn change_directory(&self, path: &str) {
        if path.is_empty() {
            return;
        }
        let mut inner = self.inner.lock();
        let cwd = match path.as_bytes()[0] {
            b'/' => String::new(),
            _ => inner.current_working_directory.clone(),
        };
        let mut cwd_vec: Vec<_> = cwd.split('/').filter(|x| !x.is_empty()).collect();
        for seg in path.split('/') {
            match seg {
                ".." => {
                    cwd_vec.pop();
                }
                "." | "" => {} // nothing to do here.
                _ => cwd_vec.push(seg),
            }
        }
        inner.current_working_directory = cwd_vec.join("/");
    }

    /// Get execute path.
    pub fn execute_path(&self) -> String {
        self.inner.lock().execute_path.clone()
    }

    /// Set execute path.
    ///
    /// Re-exec via `/proc/self/exe` or `/proc/<own_pid>/exe` must not clobber a
    /// previously recorded real path. Own pid is taken from the current thread
    /// when available; `LinuxProcess` only holds a `Weak` to its *parent*
    /// process, so there is no self-Process back-pointer to upgrade here
    /// (doing so used to `unwrap` a default `Weak` and panic on init/shell).
    pub fn set_execute_path(&self, path: &str) {
        let mut inner = self.inner.lock();
        if !inner.execute_path.is_empty() && is_proc_exe_magic(path) {
            return;
        }
        inner.execute_path = String::from(path);
    }

    /// The process's system-call personality (Linux or FreeBSD).
    pub fn abi(&self) -> Abi {
        self.inner.lock().abi
    }

    /// Set the system-call personality. The loader calls this after detecting
    /// the ABI of the ELF it just mapped (at initial load and at `execve`).
    pub fn set_abi(&self, abi: Abi) {
        self.inner.lock().abi = abi;
    }

    /// Set argv for `/proc/<pid>/cmdline`.
    pub fn set_cmdline(&self, args: Vec<String>) {
        self.inner.lock().cmdline = args;
    }

    /// Get argv.
    pub fn cmdline(&self) -> Vec<String> {
        self.inner.lock().cmdline.clone()
    }

    /// Set the environment for `/proc/<pid>/environ` (captured at `execve`).
    pub fn set_environ(&self, envs: Vec<String>) {
        self.inner.lock().environ = envs;
    }

    /// Get the environment as captured at the last `execve`.
    pub fn environ(&self) -> Vec<String> {
        self.inner.lock().environ.clone()
    }

    /// Get the current program break (top of heap).
    pub fn brk(&self) -> usize {
        self.inner.lock().brk
    }

    /// Set the current program break.
    pub fn set_brk(&self, brk: usize) {
        self.inner.lock().brk = brk;
    }

    /// Get the heap address actually mapped by `sys_brk` (>= the user-visible
    /// `brk` whenever the heap has been grown in chunks). Zero before the
    /// first `sys_brk`.
    pub fn mapped_brk(&self) -> usize {
        self.inner.lock().mapped_brk
    }

    /// Record the new upper bound of the heap's actual mapping.
    pub fn set_mapped_brk(&self, mapped_brk: usize) {
        self.inner.lock().mapped_brk = mapped_brk;
    }

    /// Get signal action.
    pub fn signal_action(&self, signal: LinuxSignal) -> SignalAction {
        self.inner.lock().signal_actions.table[signal as u8 as usize]
    }

    /// Set signal action.
    pub fn set_signal_action(&self, signal: LinuxSignal, action: SignalAction) {
        self.inner.lock().signal_actions.table[signal as u8 as usize] = action;
    }

    /// Reset signal dispositions across `execve`, as POSIX requires: every
    /// signal that was being *caught* (a custom handler) is restored to
    /// `SIG_DFL`; signals set to `SIG_IGN` or already `SIG_DFL` are left
    /// untouched. Without this, a `fork`+`exec`'d child keeps the parent
    /// shell's handler addresses; since busybox is one static binary, those
    /// addresses are still valid code in the new image, so a delivered signal
    /// (e.g. SIGINT) jumps into the shell's handler with the new applet's
    /// uninitialised globals (`ptr_to_globals == NULL`) and crashes.
    pub fn reset_signal_actions_for_exec(&self) {
        use crate::signal::{SIG_DFL, SIG_IGN};
        let mut inner = self.inner.lock();
        for action in inner.signal_actions.table.iter_mut() {
            if action.handler != SIG_DFL && action.handler != SIG_IGN {
                *action = SignalAction::default();
            }
        }
    }

    /// Close file that FD_CLOEXEC is set
    pub fn remove_cloexec_files(&self) {
        // Remove under the lock, DROP outside it — see `close_file` for the
        // re-entrancy deadlock this avoids.
        let (removed, exec_path): (Vec<(FileDesc, Arc<dyn FileLike>)>, String) = {
            let mut inner = self.inner.lock();
            // Per-fd state is authoritative — NOT the flag inside the (possibly
            // fork-shared) `File` objects, which is only a creation-time record.
            let close_fds = inner.cloexec_fds.drain().collect::<Vec<_>>();
            let removed = close_fds
                .into_iter()
                .filter_map(|fd| inner.files.remove(&fd).map(|f| (fd, f)))
                .collect();
            (removed, inner.execute_path.clone())
        };
        for (fd, f) in removed {
            // DRM diagnostics: removal of a DRM/dmabuf fd — see
            // fs::drm_fd_desc. debug level: this is NORMAL CLOEXEC behavior
            // (it fires on every execve of a process holding DRM fds); it
            // earned its keep during the stale-fd hunts and stays available
            // under LOG=debug without spamming ordinary boots.
            if let Some(desc) = crate::fs::drm_fd_desc(&f) {
                debug!(
                    "[drm] fd {:?} ({}) closed by execve CLOEXEC sweep",
                    fd, desc
                );
            }
            // Pipe diagnostics: a pipe end swept at exec is exactly the
            // dbus-launch --print-address failure shape (child inherits a
            // pipe fd across exec and the daemon writes the bus address
            // into it). Any hit here names the culprit immediately.
            // debug level: sweeping a CLOEXEC pipe end at exec is normal
            // (every shell pipeline does it); the dbus-launch bug it was
            // added for is fixed, and under LOG=debug the trace remains.
            if let Some(file) = f.downcast_ref::<crate::fs::File>() {
                let p = file.path();
                if p.starts_with("pipe_") {
                    debug!(
                        "[cloexec] {} fd={:?} ({}) closed by execve CLOEXEC sweep",
                        exec_path, fd, p
                    );
                }
            }
        }
    }

    /// Insert a `SemArray` and return its ID
    pub fn semaphores_add(&self, array: Arc<SemArray>) -> usize {
        self.inner.lock().semaphores.add(array)
    }

    /// Get an semaphore set by `id`
    pub fn semaphores_get(&self, id: usize) -> Option<Arc<SemArray>> {
        self.inner.lock().semaphores.get(id)
    }

    /// Add an undo operation
    pub fn semaphores_add_undo(&self, id: usize, num: u16, op: i16) {
        self.inner.lock().semaphores.add_undo(id, num, op)
    }

    /// Remove an `SemArray` by ID
    pub fn semaphores_remove(&self, id: usize) {
        self.inner.lock().semaphores.remove(id)
    }

    /// get ShmId from Virtual Addr
    pub fn shm_get_id(&self, id: usize) -> Option<usize> {
        self.inner.lock().shm_identifiers.get_id(id)
    }

    /// get the ShmIdentifier from shm_identifiers
    pub fn shm_get(&self, id: usize) -> Option<ShmIdentifier> {
        self.inner.lock().shm_identifiers.get(id)
    }

    /// Delete the ShmIdentifier from shm_identifiers
    pub fn shm_pop(&self, id: usize) {
        self.inner.lock().shm_identifiers.pop(id)
    }

    /// Insert the `SharedGuard` and return its ID
    pub fn shm_add(&self, shared_guard: Arc<Mutex<ShmGuard>>) -> usize {
        self.inner.lock().shm_identifiers.add(shared_guard)
    }

    /// Set Virtual Addr for shared memory
    pub fn shm_set(&self, id: usize, shm_id: ShmIdentifier) {
        self.inner.lock().shm_identifiers.set(id, shm_id)
    }
}

impl LinuxProcessInner {
    /// Fold a reaped child's CPU usage into the RUSAGE_CHILDREN totals.
    fn add_children_cpu(&mut self, cpu: ChildCpu) {
        self.children_utime_ns += cpu.utime_ns;
        self.children_stime_ns += cpu.stime_ns;
    }

    fn get_free_fd(&self) -> FileDesc {
        self.get_free_fd_from(0)
    }

    fn get_free_fd_from(&self, start: usize) -> FileDesc {
        (start..)
            .map(|i| i.into())
            .find(|fd| !self.files.contains_key(fd))
            .unwrap()
    }
}
/// Deliver SIGINT to the foreground terminal process group (job control).
pub fn deliver_sigint_to_foreground() {
    let pgid = crate::fs::stdio::get_foreground_pgrp();
    if pgid > 0 {
        let _ = send_signal_to_pgrp(pgid as usize, LinuxSignal::SIGINT);
        return;
    }
    if let Some(arc) = kernel_hal::thread::get_current_thread() {
        if let Ok(thread) = arc.downcast::<Thread>() {
            let _ = send_signal_to_process(thread.proc().id() as usize, LinuxSignal::SIGINT);
        }
    }
}

/// This process's effective process-group id: its raw pgid, or its own pid when
/// the raw value is unset (`0`).
fn effective_pgid(proc: &Arc<Process>) -> KoID {
    // try_linux: called while walking all_live_processes() (e.g. send_signal_to_pgrp
    // on Ctrl-C), so `proc` may be tearing down concurrently under SMP churn.
    // Fall back to the pid when the extension can no longer be resolved.
    let raw = proc.try_linux().map(|lp| lp.pgid_raw()).unwrap_or(0);
    if raw == 0 {
        proc.id()
    } else {
        raw
    }
}

/// Deliver `signal` to every live process in process group `pgid`. This is the
/// POSIX behaviour for terminal-generated signals (Ctrl-C → SIGINT, Ctrl-\\ →
/// SIGQUIT, Ctrl-Z → SIGTSTP): the whole foreground group is signalled, not
/// just the group leader, so a shell's foreground child (e.g. `ping`) actually
/// receives it. `ESRCH` if the group has no members.
pub fn send_signal_to_pgrp(pgid: usize, signal: LinuxSignal) -> LxResult<()> {
    let pgid = pgid as KoID;
    let mut any = false;
    for proc in all_live_processes() {
        if effective_pgid(&proc) == pgid
            && send_signal_to_process(proc.id() as usize, signal).is_ok()
        {
            any = true;
        }
    }
    if any {
        Ok(())
    } else {
        Err(LxError::ESRCH)
    }
}

/// `setpgid`: set process `pid`'s group to `pgid`. Permissive (no session/leader
/// checks): enough for a shell to put a job into its own group.
pub fn set_process_pgid(pid: KoID, pgid: KoID) -> LxResult<()> {
    let proc = all_live_processes()
        .into_iter()
        .find(|p| p.id() == pid)
        .ok_or(LxError::ESRCH)?;
    proc.try_linux().ok_or(LxError::ESRCH)?.set_pgid_raw(pgid);
    Ok(())
}

/// `getpgid`: the effective process-group id of process `pid`.
pub fn get_process_pgid(pid: KoID) -> LxResult<KoID> {
    let proc = all_live_processes()
        .into_iter()
        .find(|p| p.id() == pid)
        .ok_or(LxError::ESRCH)?;
    Ok(effective_pgid(&proc))
}

/// This process's effective session id: its raw sid, or its own pid when the
/// raw value is unset (`0`).
pub fn effective_sid(proc: &Arc<Process>) -> KoID {
    // try_linux: same teardown-race tolerance as `effective_pgid`.
    let raw = proc.try_linux().map(|lp| lp.sid_raw()).unwrap_or(0);
    if raw == 0 {
        proc.id()
    } else {
        raw
    }
}

/// `getsid`: the effective session id of process `pid`.
pub fn get_process_sid(pid: KoID) -> LxResult<KoID> {
    let proc = all_live_processes()
        .into_iter()
        .find(|p| p.id() == pid)
        .ok_or(LxError::ESRCH)?;
    Ok(effective_sid(&proc))
}

pub fn check_and_deliver_tty_interrupt() -> LxResult<()> {
    if crate::fs::stdio::ctrl_c_pending_take() {
        deliver_sigint_to_foreground();
        return Err(LxError::EINTR);
    }
    check_signals()
}

fn collect_live_processes(job: &Arc<Job>, out: &mut Vec<Arc<Process>>) {
    for id in job.process_ids() {
        if let Some(proc) = job.find_process(id) {
            if !matches!(proc.status(), Status::Exited(_)) {
                out.push(proc);
            }
        }
    }
    for child_id in job.children_ids() {
        if let Ok(child) = job.get_child(child_id) {
            if let Ok(child_job) = child.downcast_arc::<Job>() {
                collect_live_processes(&child_job, out);
            }
        }
    }
}

/// All non-exited processes in the root job tree.
pub fn all_live_processes() -> Vec<Arc<Process>> {
    let mut processes = Vec::new();
    collect_live_processes(&ROOT_JOB, &mut processes);
    processes
}

/// Linux PID of the `init` process (the base program). The system's reaper of
/// last resort for orphaned children.
const INIT_PID: KoID = 1;

/// Live INIT (PID 1) process, or `None` if there is no running init.
/// True for the Linux magic exe links that must not replace a real
/// `execute_path`. Prefer matching `/proc/self/exe` or `/proc/<own_pid>/exe`
/// when the calling thread is known; otherwise accept any `/proc/<digits>/exe`
/// so early `spawn` (no current thread yet) still preserves a prior path.
fn is_proc_exe_magic(path: &str) -> bool {
    if path == "/proc/self/exe" {
        return true;
    }
    let Some(rest) = path.strip_prefix("/proc/") else {
        return false;
    };
    let Some(digits) = rest.strip_suffix("/exe") else {
        return false;
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let Ok(link_pid) = digits.parse::<u64>() else {
        return false;
    };
    match kernel_hal::thread::get_current_thread()
        .and_then(|t| t.downcast::<Thread>().ok())
        .map(|t| t.proc().id())
    {
        Some(own_pid) => link_pid == own_pid,
        // Loader spawn has no "current" thread yet; treat numeric /proc/N/exe
        // as magic only when execute_path is already set (caller checks that).
        None => true,
    }
}

fn live_init() -> Option<Arc<Process>> {
    let init = ROOT_JOB.find_process(INIT_PID)?;
    if matches!(init.status(), Status::Exited(_)) {
        None
    } else {
        Some(init)
    }
}

/// Nearest live ancestor of `proc` that volunteered as a child subreaper via
/// `prctl(PR_SET_CHILD_SUBREAPER)` — prctl(2): orphans are reparented to it
/// instead of init (how session managers like tmux or `systemd --user` adopt
/// their descendants). The walk is capped so a corrupted parent chain can
/// never wedge the teardown path.
fn nearest_live_subreaper(proc: &Arc<Process>) -> Option<Arc<Process>> {
    let mut cur = proc.try_linux()?.parent();
    for _ in 0..64 {
        let p = cur?;
        if matches!(p.status(), Status::Exited(_)) {
            cur = p.try_linux()?.parent();
            continue;
        }
        match p.try_linux() {
            Some(lp) if lp.is_child_subreaper() => return Some(p),
            Some(lp) => cur = lp.parent(),
            None => return None,
        }
    }
    None
}

/// Choose who reaps a terminating child: its real parent while that parent is
/// still alive, otherwise the nearest live subreaper ancestor, otherwise INIT
/// (PID 1) — orphan reparenting. Returns `None` when nobody can take it (no
/// living parent, subreaper or init), so the exit status is dropped instead of
/// being leaked onto a dead process that will never `wait` for it.
fn reaper_for(parent: &Arc<Process>) -> Option<Arc<Process>> {
    if !matches!(parent.status(), Status::Exited(_)) {
        return Some(parent.clone());
    }
    if let Some(subreaper) = nearest_live_subreaper(parent) {
        return Some(subreaper);
    }
    // Parent already gone: hand the child to PID 1 (init), unless the parent
    // *is* init (it is exiting → the system is going down anyway).
    let init = live_init()?;
    if init.id() == parent.id() {
        None
    } else {
        Some(init)
    }
}

/// Reparent a terminating process's children so they are not stranded on a
/// dead parent that will never `wait` for them. The adopter is the nearest
/// live subreaper ancestor (`prctl(PR_SET_CHILD_SUBREAPER)`, prctl(2)) when
/// one exists, otherwise INIT (PID 1). Both still-live children and any
/// already-collected (zombie) exit statuses the dying process never reaped are
/// moved over, and the adopter is woken so a blocked `wait(-1)` observes the
/// adopted zombies at once. Live children that requested a parent-death signal
/// (`prctl(PR_SET_PDEATHSIG)`) receive it here — this is the moment their
/// parent dies. No-op when the dying process is INIT itself or nobody can
/// adopt (the orphans' exits then auto-reap via [`reaper_for`]).
fn reparent_live_children_to_init(dying: &Arc<Process>) {
    if dying.id() == INIT_PID {
        return;
    }
    let adopter = match nearest_live_subreaper(dying).or_else(live_init) {
        Some(adopter) => adopter,
        None => return,
    };
    let dying_linux = match dying.try_linux() {
        Some(lp) => lp,
        None => return,
    };
    let (orphans, zombies): (Vec<Arc<Process>>, Vec<ReapedChild>) = {
        let mut inner = dying_linux.inner.lock();
        let live_ids: Vec<KoID> = inner
            .children
            .iter()
            .filter(|(_, c)| !matches!(c.status(), Status::Exited(_)))
            .map(|(&id, _)| id)
            .collect();
        let orphans = live_ids
            .iter()
            .filter_map(|id| inner.children.remove(id))
            .collect();
        let zombies = inner.reaped_children.drain().collect();
        (orphans, zombies)
    };
    // Parent-death signals go out before the handover: the child asked to be
    // told when *this* parent dies, whoever adopts it afterwards.
    for orphan in &orphans {
        let sig = match orphan.try_linux() {
            Some(lp) => lp.pdeathsig(),
            None => 0,
        };
        if sig != 0 {
            if let Ok(signal) = LinuxSignal::try_from(sig) {
                let _ = send_signal_to_process(orphan.id() as usize, signal);
            }
        }
    }
    if orphans.is_empty() && zombies.is_empty() {
        return;
    }
    {
        let adopter_linux = match adopter.try_linux() {
            Some(lp) => lp,
            None => return,
        };
        let mut adopter_inner = adopter_linux.inner.lock();
        for orphan in orphans {
            adopter_inner.children.insert(orphan.id(), orphan);
        }
        for (pid, entry) in zombies {
            adopter_inner.reaped_children.insert(pid, entry);
        }
    }
    adopter.signal_set(Signal::SIGCHLD);
}

/// Insert `signal` into one unmasked thread of each live process under `ROOT_JOB`.
pub fn send_signal_to_all_processes(signal: LinuxSignal) -> LxResult<()> {
    let processes = all_live_processes();
    let mut any = false;
    for proc in processes {
        if send_signal_to_process(proc.id() as usize, signal).is_ok() {
            any = true;
        }
    }
    if any {
        Ok(())
    } else {
        Err(LxError::ESRCH)
    }
}

/// Whether a pending signal actually interrupts a blocking syscall.
///
/// Linux only interrupts a syscall for a signal that will run a handler or
/// terminate the process. A signal whose disposition is *ignore* — `SIG_IGN`,
/// or `SIG_DFL` for a signal whose default action is ignore/stop (SIGCHLD,
/// SIGURG, SIGWINCH, SIGCONT, and the job-control stops this kernel maps to
/// ignore in `handle_signal`) — is discarded and must NOT wake a blocking
/// syscall with `EINTR`.
///
/// Returning `EINTR` for these was the bug behind a compositor's libinput
/// dispatch failing with "Interrupted system call": every time an autostart
/// child exited (or crashed) and raised SIGCHLD, the parent's blocking
/// `ppoll`/`epoll_pwait` returned `EINTR`, which wlroots treats as a fatal
/// dispatch error. The disposition list here mirrors the default-ignore set in
/// `handle_signal` (loader/src/linux.rs) so the two agree on what is a no-op.
fn signal_interrupts_syscall(proc_linux: &LinuxProcess, sig: LinuxSignal) -> bool {
    use crate::signal::{SIG_DFL, SIG_IGN};
    let handler = proc_linux.signal_action(sig).handler;
    if handler == SIG_IGN {
        return false;
    }
    if handler == SIG_DFL {
        return !matches!(
            sig,
            LinuxSignal::SIGCHLD
                | LinuxSignal::SIGURG
                | LinuxSignal::SIGWINCH
                | LinuxSignal::SIGCONT
                | LinuxSignal::SIGSTOP
                | LinuxSignal::SIGTSTP
                | LinuxSignal::SIGTTIN
                | LinuxSignal::SIGTTOU
        );
    }
    // A caught signal (custom handler) interrupts the syscall.
    true
}

/// Check for pending signals and return EINTR if any *deliverable* signal
/// would interrupt the syscall. Signals whose disposition is ignore (SIG_IGN /
/// default-ignore, e.g. SIGCHLD) are skipped — matching Linux, where an ignored
/// signal is discarded and never returns EINTR from a blocking syscall.
pub fn check_signals() -> LxResult<()> {
    if let Some(arc) = kernel_hal::thread::get_current_thread() {
        if let Ok(thread) = arc.downcast::<Thread>() {
            use crate::thread::ThreadExt;
            use zircon_object::task::ThreadState;
            if thread.state() == ThreadState::Dying {
                return Err(LxError::EINTR);
            }
            if matches!(thread.proc().status(), Status::Exited(_)) {
                return Err(LxError::EINTR);
            }
            // Snapshot the deliverable (unblocked) pending signals, then drop the
            // per-thread lock before consulting the per-process disposition table
            // (a different lock) to avoid nesting the two.
            //
            // try_lock_linux (not lock_linux): the current thread may lack a
            // LinuxThread extension — most notably PID 1 / init, which running X
            // reparents exited grandchildren onto, so it takes their SIGCHLD and
            // ends up here. A thread with no Linux ext has no Linux signal state,
            // so nothing is pending and nothing can interrupt: return Ok rather
            // than unwrap-panicking ("init has no LinuxThread ext").
            let pending = match thread.try_lock_linux() {
                Some(linux_thread) => linux_thread.signals.mask_with(&linux_thread.signal_mask),
                None => return Ok(()),
            };
            if pending.is_not_empty() {
                let proc = thread.proc();
                // try_linux (not linux), mirroring the try_lock_linux above.
                // DEFENCE IN DEPTH ONLY, NOT a fix: every Linux process provably
                // HAS the ext -- process.rs:109 and :212 are the only creators
                // and both pass a LinuxProcess by value, `ext` is written once
                // in the constructor and never replaced, and Process has no
                // Drop. So a None here still means the ext fat pointer was
                // CORRUPTED and must be investigated (see the dump in
                // `Process::linux()`). But a process with no resolvable Linux
                // extension has no signal disposition table, so nothing is
                // deliverable and nothing can interrupt: answer "no pending
                // signal" rather than panicking the whole kernel from an
                // arbitrary blocking syscall at session teardown.
                let proc_linux = match proc.try_linux() {
                    Some(lp) => lp,
                    None => return Ok(()),
                };
                let mut rest = pending;
                while let Some(sig) = rest.find_first_signal() {
                    rest.remove(sig);
                    if signal_interrupts_syscall(proc_linux, sig) {
                        return Err(LxError::EINTR);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Send a signal to a process by its KoID.
pub fn send_signal_to_process(pid: usize, signal: LinuxSignal) -> LxResult<()> {
    use crate::thread::ThreadExt;
    if let Some(process) = ROOT_JOB.find_process(pid as KoID) {
        let tids = process.thread_ids();
        // Prefer a thread that has the signal *unblocked* — it can act on it
        // right away — and deliver there.
        let mut first: Option<Arc<Thread>> = None;
        for tid in tids {
            if let Ok(thread_obj) = process.get_child(tid) {
                if let Ok(thread) = thread_obj.downcast_arc::<Thread>() {
                    // try_lock_linux: this walks another process's threads, which
                    // may be tearing down concurrently under SMP churn. Skip any
                    // thread whose extension can no longer be resolved.
                    let blocked = match thread.try_lock_linux() {
                        Some(lt) => lt.signal_mask.contains(signal),
                        None => continue,
                    };
                    if !blocked {
                        if let Some(mut lt) = thread.try_lock_linux() {
                            lt.signals.insert(signal);
                            return Ok(());
                        }
                        continue;
                    }
                    if first.is_none() {
                        first = Some(thread);
                    }
                }
            }
        }
        // Every thread has the signal blocked: it must still become *pending*
        // (POSIX — masking only delays delivery, it does not discard the
        // signal), so it is delivered once unblocked or consumed via signalfd.
        // The old code dropped it here, which is why a Wayland compositor that
        // blocks SIGINT for its signalfd never saw Ctrl-C.
        if let Some(thread) = first {
            if let Some(mut lt) = thread.try_lock_linux() {
                lt.signals.insert(signal);
            }
        }
        Ok(())
    } else {
        Err(LxError::ESRCH)
    }
}
