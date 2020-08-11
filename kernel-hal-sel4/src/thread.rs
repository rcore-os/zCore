use crate::sys;
use crate::pmem::{PMEM, PhysicalRegion};
use crate::cap;
use crate::types::*;
use crate::error::*;
use crate::object::*;

pub struct TcbBacking;
unsafe impl ObjectBacking for TcbBacking {
    fn bits() -> u8 {
        unsafe {
            sys::L4BRIDGE_TCB_BITS as u8
        }
    }

    unsafe fn retype(untyped: CPtr, out: CPtr) -> KernelResult<()> {
        if sys::locked(|| sys::l4bridge_retype_tcb(untyped, out)) != 0 {
            Err(KernelError::RetypeFailed)
        } else {
            Ok(())
        }
    }
}

pub type Tcb = Object<TcbBacking>;

impl Tcb {
    pub unsafe fn prepare_as_kernel_thread(
        &self,
        pc: usize,
        sp: usize,
        ipc_buffer: usize,
        ipc_buffer_cap: CPtr,
    ) -> KernelResult<()> {
        if sys::locked(|| sys::l4bridge_configure_tcb(
            self.object(),
            CPtr(0),
            CPtr(sys::L4BRIDGE_STATIC_CAP_CSPACE), CPtr(sys::L4BRIDGE_STATIC_CAP_VSPACE),
            ipc_buffer, ipc_buffer_cap,
        )) != 0 {
            return Err(KernelError::TcbFailure);
        }

        if sys::locked(|| sys::l4bridge_set_pc_sp(self.object(), pc, sp)) != 0 {
            return Err(KernelError::TcbFailure);
        }

        Ok(())
    }

    pub fn resume(&self) -> KernelResult<()> {
        if unsafe {
            sys::locked(|| sys::l4bridge_resume(self.object()))
        } != 0 {
            Err(KernelError::ResumeFailed)
        } else {
            Ok(())
        }
    }

    pub fn set_priority(&self, prio: u8) -> KernelResult<()> {
        if unsafe {
            sys::locked(|| sys::l4bridge_set_priority(self.object(), CPtr(sys::L4BRIDGE_STATIC_CAP_TCB), prio as _))
        } != 0 {
            Err(KernelError::PriorityFailure)
        } else {
            Ok(())
        }
    }
}

pub fn yield_now() {
    unsafe {
        sys::l4bridge_yield();
    }
}
