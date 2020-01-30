use super::*;

impl Syscall {
    pub fn sys_debug_write(&self, buf: UserInPtr<u8>, len: usize) -> ZxResult<usize> {
        let data = buf.read_array(len)?;
        kernel_hal::serial_write(core::str::from_utf8(&data).unwrap());
        kernel_hal::serial_write("\n");
        Ok(0)
    }
}
