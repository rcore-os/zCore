use crate::{ZxError, ZxResult};
use kernel_hal::GeneralRegs;

#[repr(u32)]
#[derive(Debug, Copy, Clone)]
pub enum ThreadStateKind {
    General = 0,
    FloatPoint = 1,
    Vector = 2,
    Debug = 4,
    SingleStep = 5,
    #[cfg(target_arch = "x86_64")]
    FS = 6,
    #[cfg(target_arch = "x86_64")]
    GS = 7,
}

pub fn read_state(general: &GeneralRegs, kind: ThreadStateKind, buf: &mut [u8]) -> ZxResult<usize> {
    match kind {
        ThreadStateKind::General => buf.write_struct(general),
        #[cfg(target_arch = "x86_64")]
        ThreadStateKind::FS => buf.write_struct(&general.fsbase),
        #[cfg(target_arch = "x86_64")]
        ThreadStateKind::GS => buf.write_struct(&general.gsbase),
        _ => unimplemented!(),
    }
}

pub fn write_state(general: &mut GeneralRegs, kind: ThreadStateKind, buf: &[u8]) -> ZxResult<()> {
    match kind {
        ThreadStateKind::General => *general = buf.read_struct()?,
        #[cfg(target_arch = "x86_64")]
        ThreadStateKind::FS => general.fsbase = buf.read_struct()?,
        #[cfg(target_arch = "x86_64")]
        ThreadStateKind::GS => general.gsbase = buf.read_struct()?,
        _ => unimplemented!(),
    }
    Ok(())
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
