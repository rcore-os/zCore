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

    pub fn sys_debuglog_write(
        &self,
        _handle_value: HandleValue,
        _options: u32,
        buf: UserInPtr<u8>,
        len: usize,
    ) -> ZxResult<usize> {
        self.sys_debug_write(buf, len)
    }
}
