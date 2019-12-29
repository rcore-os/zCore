use super::*;

impl Syscall {
    pub fn sys_debug_write(&self, buf: UserInPtr<u8>, len: usize) -> ZxResult<usize> {
        let data = buf.read_array(len)?;
        let s = core::str::from_utf8(&data).map_err(|_| ZxError::INVALID_ARGS)?;
        info!("debug_write: {:?}", s);
        Ok(0)
    }
}
