use crate::types::*;
use crate::error::*;
use crate::object::*;
use crate::sys;
use crate::thread::Tcb;
use crate::asid;
use crate::cap;
use core::mem::{self, ManuallyDrop};
use trapframe::{TrapFrame, UserContext, GeneralRegs};
use alloc::sync::Arc;
use alloc::boxed::Box;

const USER_CSPACE_NUM_ENTRIES_BITS: u8 = 1; // 2 entries

type UserVspace = Object<UserVspaceBacking>;
type UserCspace = Object<UserCspaceBacking>;

#[derive(Copy, Clone, Debug)]
pub enum KernelEntryReason {
    NotStarted,
    UnknownSyscall,
    VMFault,
    Unknown,
}

/// The context of a user process.
pub struct UserProcess {
    vspace: UserVspace,
    fault_channel: UserFaultChannel,
}

/// The context of a user thread.
pub struct UserThread {
    process: Arc<UserProcess>,
    cspace: ManuallyDrop<UserCspace>,
    tcb: ManuallyDrop<Tcb>,
    kernel_entry_reason: KernelEntryReason,
}

impl UserProcess {
    pub fn new() -> KernelResult<Arc<UserProcess>> {
        // Check consistency early.
        L4UserContext::check_consistency();

        let vspace = UserVspace::new()?;
        let fault_channel = UserFaultChannel::new()?;

        asid::assign(vspace.object())?;

        Ok(Arc::new(UserProcess {
            vspace,
            fault_channel,
        }))
    }

    pub fn create_thread(self: &Arc<Self>) -> KernelResult<Box<UserThread>> {
        let cspace = UserCspace::new()?;
        let tcb = Tcb::new()?;
        let ut = Box::new(UserThread {
            process: self.clone(),
            cspace: ManuallyDrop::new(cspace),
            tcb: ManuallyDrop::new(tcb),
            kernel_entry_reason: KernelEntryReason::NotStarted,
        });
        let ut_addr = &*ut as *const UserThread as usize;

        if unsafe {
            sys::l4bridge_badge_endpoint_to_user_thread_ts(
                self.fault_channel.object(),
                ut.cspace.object(),
                CPtr(1),
                USER_CSPACE_NUM_ENTRIES_BITS as _,
                ut_addr
            )
        } != 0 {
            panic!("UserProcess::create_thread: cannot copy fault endpoint");
        }

        if unsafe { sys::locked(|| sys::l4bridge_configure_tcb(
            ut.tcb.object(),
            CPtr(1 << (64 - USER_CSPACE_NUM_ENTRIES_BITS)),
            ut.cspace.object(), self.vspace.object(),
            0, CPtr(0),
        )) } != 0 {
            // should never fail
            panic!("UserProcess::create_thread: cannot configure tcb");
        }
        ut.tcb.set_priority(user_thread_priority()).expect("UserProcess::create_thread: cannot set priority");

        Ok(ut)
    }
}

impl UserThread {
    pub fn run(self: Box<Self>, uctx: &mut UserContext) -> (KernelEntryReason, Box<Self>) {
        let mut l4uctx = L4UserContext::read_user_context(uctx);
        let mut sender_badge: usize = 0;
        let fault_num = match self.kernel_entry_reason {
            KernelEntryReason::NotStarted => {
                if unsafe {
                    sys::l4bridge_write_all_registers_ts(
                        self.tcb.object(),
                        &l4uctx,
                        1
                    )
                } != 0 {
                    panic!("UserProcess::run: cannot write registers");
                }
                unsafe {
                    sys::l4bridge_fault_ipc_first_return_ts(self.process.fault_channel.object(), &mut l4uctx, &mut sender_badge)
                }
            }
            KernelEntryReason::UnknownSyscall => {
                unsafe {
                    sys::l4bridge_fault_ipc_return_unknown_syscall_ts(
                        self.process.fault_channel.object(),
                        &mut l4uctx,
                        &mut sender_badge
                    )
                }
            }
            KernelEntryReason::VMFault => {
                if unsafe {
                    sys::l4bridge_write_all_registers_ts(
                        self.tcb.object(),
                        &l4uctx,
                        0
                    )
                } != 0 {
                    panic!("UserProcess::run: cannot write registers");
                }
                unsafe {
                    sys::l4bridge_fault_ipc_return_generic_ts(
                        self.process.fault_channel.object(),
                        &mut l4uctx,
                        &mut sender_badge
                    )
                }
            }
            KernelEntryReason::Unknown => {
                panic!("UserProcess::run: bad entry reason");
            }
        };

        // Now we've got the newly entering thread and the ownership of `self` is "implicitly" passed to
        // the kernel scheduler. Forget it.
        mem::forget(self);
        let mut t: Box<Self> = unsafe {
            Box::from_raw(sender_badge as *mut Self)
        };

        let fault_num = fault_num as usize;
        let registers_preloaded;
        t.kernel_entry_reason = if fault_num == unsafe { sys::L4BRIDGE_FAULT_UNKNOWN_SYSCALL } {
            registers_preloaded = true;
            KernelEntryReason::UnknownSyscall
        } else if fault_num == unsafe { sys::L4BRIDGE_FAULT_VM } {
            registers_preloaded = false;
            KernelEntryReason::VMFault
        } else {
            registers_preloaded = false;
            KernelEntryReason::Unknown
        };
        if !registers_preloaded {
            if unsafe {
                sys::l4bridge_read_all_registers_ts(
                    t.tcb.object(),
                    &mut l4uctx,
                    0
                )
            } != 0 {
                panic!("UserProcess::run: cannot read registers");
            }
        }
        l4uctx.write_user_context(uctx);
        let entry_reason = t.kernel_entry_reason;
        (entry_reason, t)
    }
}

impl Drop for UserThread {
    fn drop(&mut self) {
        unsafe {
            // `tcb` uses `cspace` so drop it first
            ManuallyDrop::drop(&mut self.tcb);
            ManuallyDrop::drop(&mut self.cspace);
        }
    }
}

struct UserVspaceBacking;
unsafe impl ObjectBacking for UserVspaceBacking {
    fn bits() -> u8 {
        unsafe {
            sys::L4BRIDGE_VSPACE_BITS as u8
        }
    }

    unsafe fn retype(untyped: CPtr, out: CPtr) -> KernelResult<()> {
        if sys::locked(|| sys::l4bridge_retype_vspace(untyped, out)) != 0 {
            Err(KernelError::RetypeFailed)
        } else {
            Ok(())
        }
    }
}

struct UserCspaceBacking;
unsafe impl ObjectBacking for UserCspaceBacking {
    fn bits() -> u8 {
        unsafe {
            sys::L4BRIDGE_CNODE_SLOT_BITS as u8 + USER_CSPACE_NUM_ENTRIES_BITS
        }
    }

    unsafe fn retype(untyped: CPtr, out: CPtr) -> KernelResult<()> {
        if sys::locked(|| sys::l4bridge_retype_cnode(untyped, out, USER_CSPACE_NUM_ENTRIES_BITS as _)) != 0 {
            Err(KernelError::RetypeFailed)
        } else {
            Ok(())
        }
    }
}

struct UserFaultChannelBacking;
unsafe impl ObjectBacking for UserFaultChannelBacking {
    fn bits() -> u8 {
        unsafe {
            sys::L4BRIDGE_ENDPOINT_BITS as u8
        }
    }

    unsafe fn retype(untyped: CPtr, out: CPtr) -> KernelResult<()> {
        if sys::locked(|| sys::l4bridge_retype_endpoint(untyped, out)) != 0 {
            Err(KernelError::RetypeFailed)
        } else {
            Ok(())
        }
    }
}
type UserFaultChannel = Object<UserFaultChannelBacking>;

#[repr(C)]
pub struct L4UserContext {
    rip: usize,
    rsp: usize,
    rflags: usize,
    rax: usize,
    rbx: usize,
    rcx: usize,
    rdx: usize,
    rsi: usize,
    rdi: usize,
    rbp: usize,
    r8: usize,
    r9: usize,
    r10: usize,
    r11: usize,
    r12: usize,
    r13: usize,
    r14: usize,
    r15: usize,
    fs_base: usize,
    gs_base: usize,
}

impl L4UserContext {
    fn check_consistency() {
        if unsafe { sys::L4BRIDGE_NUM_REGISTERS } != mem::size_of::<Self>() / mem::size_of::<usize>() {
            panic!("L4UserContext::check_consistency: inconsistent layout with loader");
        }
    }

    fn read_user_context(uctx: &UserContext) -> Self {
        Self {
            rax: uctx.general.rax,
            rbx: uctx.general.rbx,
            rcx: uctx.general.rcx,
            rdx: uctx.general.rdx,
            rsi: uctx.general.rsi,
            rdi: uctx.general.rdi,
            rbp: uctx.general.rbp,
            rsp: uctx.general.rsp,
            r8: uctx.general.r8,
            r9: uctx.general.r9,
            r10: uctx.general.r10,
            r11: uctx.general.r11,
            r12: uctx.general.r12,
            r13: uctx.general.r13,
            r14: uctx.general.r14,
            r15: uctx.general.r15,
            rip: uctx.general.rip,
            rflags: uctx.general.rflags,
            fs_base: uctx.general.fsbase,
            gs_base: uctx.general.gsbase,
        }
    }

    fn write_user_context(&self, uctx: &mut UserContext) {
        uctx.general = GeneralRegs {
            rax: self.rax,
            rbx: self.rbx,
            rcx: self.rcx,
            rdx: self.rdx,
            rsi: self.rsi,
            rdi: self.rdi,
            rbp: self.rbp,
            rsp: self.rsp,
            r8: self.r8,
            r9: self.r9,
            r10: self.r10,
            r11: self.r11,
            r12: self.r12,
            r13: self.r13,
            r14: self.r14,
            r15: self.r15,
            rip: self.rip,
            rflags: self.rflags,
            fsbase: self.fs_base,
            gsbase: self.gs_base,
        };
    }
}

fn user_thread_priority() -> u8 {
    unsafe {
        sys::L4BRIDGE_MAX_PRIO as u8 - 1
    }
}

#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    unreachable!("trap_handler")
}