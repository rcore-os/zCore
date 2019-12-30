use super::*;

impl Syscall {
    pub fn sys_debuglog_create(
        &self,
        rsrc: usize,
        options: usize,
        target: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        target.write(1u32)?;
        Ok(ZxError::OK as usize)
    }
}
