//! Linux Thread

use crate::error::SysResult;
use crate::process::ProcessExt;
use crate::signal::{SigInfo, Signal, SignalStack, SignalUserContext, Sigset};
use alloc::string::String;
use alloc::sync::Arc;
use kernel_hal::context::{UserContext, UserContextField};
use kernel_hal::user::{Out, UserInPtr, UserOutPtr, UserPtr};
use lock::{Mutex, MutexGuard};
use zircon_object::object::KernelObject;
use zircon_object::task::{CurrentThread, Process, Thread};
use zircon_object::ZxResult;

/// Thread extension for linux
pub trait ThreadExt {
    /// create linux thread
    fn create_linux(proc: &Arc<Process>) -> ZxResult<Arc<Self>>;
    /// lock and get Linux thread
    fn lock_linux(&self) -> MutexGuard<'_, LinuxThread>;
    /// Like [`lock_linux`](Self::lock_linux) but returns `None` instead of
    /// panicking when the extension is not a `Mutex<LinuxThread>`. Use this when
    /// walking another process's threads (signal delivery, enumeration): a
    /// thread observed mid-teardown during SMP churn must be skipped, not bring
    /// down the kernel.
    fn try_lock_linux(&self) -> Option<MutexGuard<'_, LinuxThread>>;
    /// Set pointer to thread ID.
    fn set_tid_address(&self, tidptr: UserOutPtr<i32>);
    /// Get robust list.
    fn get_robust_list(
        &self,
        _head_ptr: UserOutPtr<UserOutPtr<RobustList>>,
        _len_ptr: UserOutPtr<usize>,
    ) -> SysResult;
    /// Set robust list.
    fn set_robust_list(&self, head: UserInPtr<RobustList>, len: usize);
}

/// CurrentThread extension for linux
pub trait CurrentThreadExt {
    /// exit linux thread
    fn exit_linux(&self, exit_code: i32);
}

impl ThreadExt for Thread {
    fn create_linux(proc: &Arc<Process>) -> ZxResult<Arc<Self>> {
        let linux_thread = Mutex::new(LinuxThread {
            clear_child_tid: 0.into(),
            signals: Sigset::default(),
            signal_mask: Sigset::default(),
            saved_sigmask: None,
            signal_alternate_stack: SignalStack::default(),
            robust_list: 0.into(),
            robust_list_len: 0,
            handling_signal: None,
            comm: String::new(),
            timerslack_ns: 0,
        });
        // The thread-group leader (the process's first/main thread) must have a
        // TID equal to the process PID, just like Linux. Userspace relies on
        // this: e.g. winit's `is_main_thread()` panics unless gettid()==getpid()
        // on the main thread, and tgkill(getpid(), gettid()) must reach the
        // leader. Without it, every KObject (process, thread, VMO, ...) draws a
        // distinct id from one global counter, so the leader's TID never matched
        // its PID. Subsequent threads (pthread_create) keep getting fresh,
        // unique ids — only the leader reuses the PID, which is allocated to no
        // other object.
        let leader_id = if proc.thread_ids().is_empty() {
            Some(proc.id())
        } else {
            None
        };
        Thread::create_with_ext_id(proc, "", linux_thread, leader_id)
    }

    fn lock_linux(&self) -> MutexGuard<'_, LinuxThread> {
        // See Process::linux(): a failed downcast means a non-Linux thread
        // leaked into a Linux-only path or the ext Box was corrupted. Identify
        // the thread/process so the panic names the culprit.
        self.ext()
            .downcast_ref::<Mutex<LinuxThread>>()
            .unwrap_or_else(|| {
                // Same evidence as Process::linux(): the fat pointer now, the
                // fat pointer at construction, and the guards either side of
                // the field. Which words moved says whether this is an 8-byte
                // store, a whole-fat-pointer assignment, or no write at all.
                let (data, vtable) = self.ext_fat();
                let (born_data, born_vtable) = self.ext_born();
                let vt = zircon_object::task::vtable_info(vtable);
                // See Process::linux(): `ext` is immutable, so a downcast that
                // fails and then immediately succeeds saw an inconsistent read,
                // not a different type. Carry on with the value we can now see
                // is correct rather than killing the kernel, and log it.
                if let Some(m) = self.ext().downcast_ref::<Mutex<LinuxThread>>() {
                    error!(
                        "[ext-glitch] Thread::lock_linux(): tid={} pid={} name={:?} downcast \
                         failed then SUCCEEDED on retry -- ext read inconsistently. \
                         fat data={:#x} vtable={:#x}, at birth data={:#x} vtable={:#x}",
                        self.id(),
                        self.proc().id(),
                        self.proc().name(),
                        data,
                        vtable,
                        born_data,
                        born_vtable,
                    );
                    return m;
                }
                panic!(
                    "Thread::lock_linux(): tid={} proc pid={} name={:?} has no \
                     LinuxThread ext (ext fat pointer: data={:#x} vtable={:#x} \
                     -> {:x?} (drop, size, align), Mutex<LinuxThread> would be \
                     size={} align={}; \
                     at birth: data={:#x} vtable={:#x} -> {}; canaries {}) -- \
                     non-Linux thread in a Linux path, or corrupted ext",
                    self.id(),
                    self.proc().id(),
                    self.proc().name(),
                    data,
                    vtable,
                    vt,
                    core::mem::size_of::<Mutex<LinuxThread>>(),
                    core::mem::align_of::<Mutex<LinuxThread>>(),
                    born_data,
                    born_vtable,
                    match (born_data == data, born_vtable == vtable) {
                        (true, true) => "UNCHANGED: the ext was never a LinuxThread",
                        (true, false) => "VTABLE ONLY: one 8-byte store, data untouched",
                        (false, true) => "DATA ONLY: one 8-byte store, vtable untouched",
                        (false, false) => "BOTH words replaced",
                    },
                    match self.ext_canaries() {
                        (true, true) => "both INTACT: a precise write to ext alone",
                        (false, true) => "LOW broken: overrun growing upward from below",
                        (true, false) => "HIGH broken: overrun growing downward from above",
                        (false, false) => "BOTH broken: wide overrun across the field",
                    },
                )
            })
            .lock()
    }

    fn try_lock_linux(&self) -> Option<MutexGuard<'_, LinuxThread>> {
        Some(self.ext().downcast_ref::<Mutex<LinuxThread>>()?.lock())
    }

    /// Set pointer to thread ID.
    fn set_tid_address(&self, tidptr: UserPtr<i32, Out>) {
        self.lock_linux().clear_child_tid = tidptr;
    }

    fn get_robust_list(
        &self,
        mut _head_ptr: UserOutPtr<UserOutPtr<RobustList>>,
        mut _len_ptr: UserOutPtr<usize>,
    ) -> SysResult {
        _head_ptr = (self.lock_linux().robust_list.as_addr() as *mut RobustList as usize).into();
        _len_ptr = (&self.lock_linux().robust_list_len as *const usize as usize).into();
        Ok(0)
    }

    fn set_robust_list(&self, head: UserInPtr<RobustList>, len: usize) {
        self.lock_linux().robust_list = head;
        self.lock_linux().robust_list_len = len;
    }
}

impl CurrentThreadExt for CurrentThread {
    /// Exit current thread for Linux.
    fn exit_linux(&self, _exit_code: i32) {
        let mut linux_thread = self.lock_linux();
        let clear_child_tid = &mut linux_thread.clear_child_tid;
        // perform futex wake 1
        // ref: http://man7.org/linux/man-pages/man2/set_tid_address.2.html
        if !clear_child_tid.is_null() {
            info!("exit: do futex {:?} wake 1", clear_child_tid);
            #[cfg(target_os = "none")]
            {
                let vaddr = clear_child_tid.as_addr();
                let vmar = self.proc().vmar();
                if vmar.contains(vaddr) {
                    // The page may be lazily allocated or CoW (mapped
                    // read-only after fork): fault it in writable first.
                    // Skipping the clear+wake here would leave pthread_join
                    // (and musl's __tl_sync) waiting forever.
                    let writable = matches!(
                        vmar.get_vaddr_flags(vaddr),
                        Ok(flags) if flags.contains(kernel_hal::MMUFlags::WRITE)
                    );
                    let mapped = writable
                        || vmar
                            .handle_page_fault(
                                vaddr,
                                kernel_hal::MMUFlags::WRITE | kernel_hal::MMUFlags::USER,
                            )
                            .is_ok();
                    if mapped && clear_child_tid.write(0).is_ok() {
                        if let Some(futex) = self.proc().linux().get_futex(vaddr) {
                            futex.wake(1);
                        }
                    }
                }
            }
            #[cfg(not(target_os = "none"))]
            {
                clear_child_tid.write(0).unwrap();
                let uaddr = clear_child_tid.as_addr();
                if let Some(futex) = self.proc().linux().get_futex(uaddr) {
                    futex.wake(1);
                }
            }
        }
        self.exit();
    }
}

/// robust_list
#[derive(Default)]
pub struct RobustList {
    /// head
    pub head: usize,
    /// off
    pub off: isize,
    /// pending
    pub pending: usize,
}

/// Linux specific thread information.
pub struct LinuxThread {
    /// Kernel performs futex wake when thread exits.
    /// Ref: <http://man7.org/linux/man-pages/man2/set_tid_address.2.html>
    clear_child_tid: UserOutPtr<i32>,
    /// Linux signals
    pub signals: Sigset,
    /// Signal mask
    pub signal_mask: Sigset,
    /// Signal mask to restore once the currently-awaited signal handler
    /// returns. Set by `rt_sigsuspend` so that the original mask is restored
    /// after the temporarily-unblocked signal is delivered.
    pub saved_sigmask: Option<Sigset>,
    /// signal alternate stack
    pub signal_alternate_stack: SignalStack,
    /// robust_list
    robust_list: UserInPtr<RobustList>,
    robust_list_len: usize,
    /// handling signals
    pub handling_signal: Option<u32>,
    /// Thread name (`prctl(PR_SET_NAME)` / `/proc/<pid>/comm`), at most
    /// [`TASK_COMM_LEN`]` - 1` bytes. Empty = never set: readers fall back to
    /// the executable's basename, so a fresh thread reports its program name.
    pub comm: String,
    /// Timer slack in nanoseconds (`prctl(PR_SET_TIMERSLACK)`). `0` = never
    /// set → reads as the Linux default of 50 µs. Recorded and read back;
    /// timers here do not apply slack coalescing.
    pub timerslack_ns: u64,
}

/// Size of the kernel's per-task `comm` buffer, including the trailing NUL
/// (`TASK_COMM_LEN` in `include/linux/sched.h`): names are truncated to 15
/// bytes.
pub const TASK_COMM_LEN: usize = 16;

fn unmodified_check(siginfo: &SigInfo, user_ctx: &SignalUserContext) -> usize {
    let mut check = 0usize;
    let default_info = SigInfo::default();
    let mut default_ctx = SignalUserContext::default();
    default_ctx.context.set_pc(user_ctx.context.get_pc());
    check |= (*siginfo != default_info) as usize;
    check |= ((user_ctx.flags != default_ctx.flags) as usize) << 1;
    check |= ((user_ctx.link != default_ctx.link) as usize) << 2;
    check |= ((user_ctx.stack != default_ctx.stack) as usize) << 3;
    check |= ((user_ctx._pad != default_ctx._pad) as usize) << 4;
    check |= ((user_ctx.context != default_ctx.context) as usize) << 5;
    #[cfg(target_arch = "x86_64")]
    {
        check |= ((user_ctx.fpregs_mem != default_ctx.fpregs_mem) as usize) << 6;
    }
    check
}

#[allow(unsafe_code)]
impl LinuxThread {
    /// Restore the information after the signal handler returns
    pub fn restore_after_handle_signal(
        &mut self,
        ctx: &mut UserContext,
        old_ctx: &UserContext,
        siginfo_ptr: usize,
        uctx_ptr: usize,
    ) {
        let siginfo = unsafe { &*(siginfo_ptr as *const SigInfo) };
        let user_ctx = unsafe { &*(uctx_ptr as *const SignalUserContext) };
        let check = unmodified_check(siginfo, user_ctx);
        if check != 0 {
            error!("unsupported signal fields : {:b}", check);
            trace!("uctx = {:x?}", *user_ctx);
            // Be tolerant: userland may legally modify parts of ucontext/siginfo.
            // We restore the saved context and only honor the restored PC/mask below.
        }
        *ctx = *old_ctx;
        ctx.set_field(UserContextField::InstrPointer, user_ctx.context.get_pc());
        self.signal_mask = Sigset::new(user_ctx.sig_mask.val());
        self.handling_signal = None;
    }

    /// Get signal info
    pub fn get_signal_info(&self) -> (Sigset, Sigset, Option<u32>) {
        (self.signals, self.signal_mask, self.handling_signal)
    }

    /// Address registered via `set_tid_address`/`CLONE_CHILD_CLEARTID`, for
    /// `prctl(PR_GET_TID_ADDRESS)`. `0` when never set.
    pub fn tid_address(&self) -> usize {
        self.clear_child_tid.as_addr()
    }

    /// Handle signal
    pub fn handle_signal(&mut self) -> Option<(Signal, Sigset)> {
        if self.handling_signal.is_none() {
            let signal = self
                .signals
                .mask_with(&self.signal_mask)
                .find_first_signal();
            if let Some(signal) = signal {
                self.handling_signal = Some(signal as u32);
                self.signals.remove(signal);
                // If a `rt_sigsuspend` (or similar) saved a mask to restore once
                // the handler returns, hand that mask to the signal frame so it
                // is reinstated on `sigreturn`. Otherwise keep the current mask.
                let restore_mask = self.saved_sigmask.take().unwrap_or(self.signal_mask);
                return Some((signal, restore_mask));
            }
        }
        None
    }
}
