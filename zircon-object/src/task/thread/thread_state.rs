use crate::{ZxError, ZxResult};
use kernel_hal::context::UserContext;
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
        FS = 6,
        GS = 7,
    }
}

pub(super) trait ContextExt {
    fn read_state(&self, kind: ThreadStateKind, buf: &mut [u8]) -> ZxResult<usize>;
    fn write_state(&mut self, kind: ThreadStateKind, buf: &[u8]) -> ZxResult;
}

impl ContextExt for UserContext {
    fn read_state(&self, kind: ThreadStateKind, buf: &mut [u8]) -> ZxResult<usize> {
        match kind {
            ThreadStateKind::General => buf.write_struct(&self.general),
            #[cfg(target_arch = "x86_64")]
            ThreadStateKind::FS => buf.write_struct(&self.general.fsbase),
            #[cfg(target_arch = "x86_64")]
            ThreadStateKind::GS => buf.write_struct(&self.general.gsbase),
            _ => Err(ZxError::NOT_SUPPORTED),
        }
    }

    fn write_state(&mut self, kind: ThreadStateKind, buf: &[u8]) -> ZxResult {
        match kind {
            ThreadStateKind::General => self.general = buf.read_struct()?,
            #[cfg(target_arch = "x86_64")]
            ThreadStateKind::FS => self.general.fsbase = buf.read_struct()?,
            #[cfg(target_arch = "x86_64")]
            ThreadStateKind::GS => self.general.gsbase = buf.read_struct()?,
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
