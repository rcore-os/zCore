use super::*;

impl Syscall<'_> {
    pub fn sys_debug_write(&self, buf: UserInPtr<u8>, len: usize) -> ZxResult {
        let data = buf.read_array(len)?;
        kernel_hal::serial_write(core::str::from_utf8(&data).unwrap());
        kernel_hal::serial_write("\n");
        Ok(())
    }

    pub fn sys_debug_read(
        &self,
        handle: HandleValue,
        mut buf: UserOutPtr<u8>,
        buf_size: u32,
        mut actual: UserOutPtr<u32>,
    ) -> ZxResult {
        info!(
            "handle = {:#x}, buf_size={:#x}",
            handle, buf_size
        );
        buf.write_cstring("HelloWorld\n")?;
        actual.write(11)?;
        Ok(())
    }
}
