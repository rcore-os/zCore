use crate::{ZxError, ZxResult};
use kernel_hal::context::UserContext;
#[cfg(target_arch = "riscv64")]
use kernel_hal::context::{GeneralRegs, UserContextField};
use numeric_enum_macro::numeric_enum;

numeric_enum! {
    #[repr(u32)]
    /// Possible values for "kind" in zx_thread_read_state and zx_thread_write_state.
    #[allow(missing_docs)]
    #[derive(Debug, Copy, Clone)]
    pub enum ThreadStateKind {
        General = 0,
        FloatPoint = 1,
        Vector = 2,
        Debug = 4,
        SingleStep = 5,
    }
}

pub(super) trait ContextAccessState {
    fn read_state(&self, kind: ThreadStateKind, buf: &mut [u8]) -> ZxResult<usize>;
    fn write_state(&mut self, kind: ThreadStateKind, buf: &[u8]) -> ZxResult;
}

impl ContextAccessState for UserContext {
    fn read_state(&self, kind: ThreadStateKind, buf: &mut [u8]) -> ZxResult<usize> {
        match kind {
            ThreadStateKind::General => {
                #[cfg(target_arch = "riscv64")]
                {
                    let mut context = *self;
                    let mut regs = *context.general();
                    // The Zircon ABI puts PC in slot zero, where trapframe keeps
                    // an unused placeholder for the architectural x0 register.
                    regs.zero = context.get_field(UserContextField::InstrPointer);
                    buf.write_struct(&regs)
                }
                #[cfg(not(target_arch = "riscv64"))]
                {
                    buf.write_struct(self.general())
                }
            }
            _ => Err(ZxError::NOT_SUPPORTED),
        }
    }

    fn write_state(&mut self, kind: ThreadStateKind, buf: &[u8]) -> ZxResult {
        match kind {
            ThreadStateKind::General => {
                #[cfg(target_arch = "riscv64")]
                {
                    let mut regs: GeneralRegs = buf.read_struct()?;
                    let pc = regs.zero;
                    regs.zero = 0;
                    *self.general_mut() = regs;
                    self.set_field(UserContextField::InstrPointer, pc);
                }
                #[cfg(not(target_arch = "riscv64"))]
                {
                    *self.general_mut() = buf.read_struct()?;
                }
            }
            _ => return Err(ZxError::NOT_SUPPORTED),
        }
        Ok(())
    }
}

trait BufExt {
    fn read_struct<T>(&self) -> ZxResult<T>;
    fn write_struct<T: Copy>(&mut self, value: &T) -> ZxResult<usize>;
}

#[allow(unsafe_code)]
impl BufExt for [u8] {
    fn read_struct<T>(&self) -> ZxResult<T> {
        if self.len() < core::mem::size_of::<T>() {
            return Err(ZxError::BUFFER_TOO_SMALL);
        }
        Ok(unsafe { (self.as_ptr() as *const T).read() })
    }

    fn write_struct<T: Copy>(&mut self, value: &T) -> ZxResult<usize> {
        if self.len() < core::mem::size_of::<T>() {
            return Err(ZxError::BUFFER_TOO_SMALL);
        }
        unsafe {
            *(self.as_mut_ptr() as *mut T) = *value;
        }
        Ok(core::mem::size_of::<T>())
    }
}
