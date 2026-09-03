use crate::{ZxError, ZxResult};
#[cfg(target_arch = "riscv64")]
use kernel_hal::context::GeneralRegs;
use kernel_hal::context::UserContext;
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
use kernel_hal::context::UserContextField;
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
                #[cfg(target_arch = "aarch64")]
                {
                    let mut context = *self;
                    let regs = context.general();
                    let state = Aarch64GeneralRegs {
                        r: [
                            regs.x0, regs.x1, regs.x2, regs.x3, regs.x4, regs.x5, regs.x6, regs.x7,
                            regs.x8, regs.x9, regs.x10, regs.x11, regs.x12, regs.x13, regs.x14,
                            regs.x15, regs.x16, regs.x17, regs.x18, regs.x19, regs.x20, regs.x21,
                            regs.x22, regs.x23, regs.x24, regs.x25, regs.x26, regs.x27, regs.x28,
                            regs.x29,
                        ],
                        lr: regs.x30,
                        sp: context.get_field(UserContextField::StackPointer),
                        pc: context.get_field(UserContextField::InstrPointer),
                        cpsr: context.status_register(),
                        tpidr: context.get_field(UserContextField::ThreadPointer),
                    };
                    buf.write_struct(&state)
                }
                #[cfg(target_arch = "riscv64")]
                {
                    let mut context = *self;
                    let mut regs = *context.general();
                    // The Zircon ABI puts PC in slot zero, where trapframe keeps
                    // an unused placeholder for the architectural x0 register.
                    regs.zero = context.get_field(UserContextField::InstrPointer);
                    buf.write_struct(&regs)
                }
                #[cfg(target_arch = "x86_64")]
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
                #[cfg(target_arch = "aarch64")]
                {
                    let state: Aarch64GeneralRegs = buf.read_struct()?;
                    let regs = self.general_mut();
                    regs.x0 = state.r[0];
                    regs.x1 = state.r[1];
                    regs.x2 = state.r[2];
                    regs.x3 = state.r[3];
                    regs.x4 = state.r[4];
                    regs.x5 = state.r[5];
                    regs.x6 = state.r[6];
                    regs.x7 = state.r[7];
                    regs.x8 = state.r[8];
                    regs.x9 = state.r[9];
                    regs.x10 = state.r[10];
                    regs.x11 = state.r[11];
                    regs.x12 = state.r[12];
                    regs.x13 = state.r[13];
                    regs.x14 = state.r[14];
                    regs.x15 = state.r[15];
                    regs.x16 = state.r[16];
                    regs.x17 = state.r[17];
                    regs.x18 = state.r[18];
                    regs.x19 = state.r[19];
                    regs.x20 = state.r[20];
                    regs.x21 = state.r[21];
                    regs.x22 = state.r[22];
                    regs.x23 = state.r[23];
                    regs.x24 = state.r[24];
                    regs.x25 = state.r[25];
                    regs.x26 = state.r[26];
                    regs.x27 = state.r[27];
                    regs.x28 = state.r[28];
                    regs.x29 = state.r[29];
                    regs.x30 = state.lr;
                    self.set_field(UserContextField::StackPointer, state.sp);
                    self.set_field(UserContextField::InstrPointer, state.pc);
                    self.set_field(UserContextField::ThreadPointer, state.tpidr);
                    self.set_status_register(state.cpsr);
                }
                #[cfg(target_arch = "riscv64")]
                {
                    let mut regs: GeneralRegs = buf.read_struct()?;
                    let pc = regs.zero;
                    regs.zero = 0;
                    *self.general_mut() = regs;
                    self.set_field(UserContextField::InstrPointer, pc);
                }
                #[cfg(target_arch = "x86_64")]
                {
                    *self.general_mut() = buf.read_struct()?;
                }
            }
            _ => return Err(ZxError::NOT_SUPPORTED),
        }
        Ok(())
    }
}

#[cfg(target_arch = "aarch64")]
#[repr(C)]
#[derive(Copy, Clone)]
struct Aarch64GeneralRegs {
    r: [usize; 30],
    lr: usize,
    sp: usize,
    pc: usize,
    cpsr: usize,
    tpidr: usize,
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
