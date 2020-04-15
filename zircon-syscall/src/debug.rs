use super::*;

impl Syscall<'_> {
    pub fn sys_debug_write(&self, buf: UserInPtr<u8>, len: usize) -> ZxResult {
        info!("debug.write: buf=({:?}; {:#x})", buf, len);
        let data = buf.read_array(len)?;
        kernel_hal::serial_write(core::str::from_utf8(&data).unwrap());
        kernel_hal::serial_write("\n");
        Ok(())
    }

    pub fn sys_debug_read(
        &self,
        handle: HandleValue,
        buf: UserOutPtr<u8>,
        buf_size: u32,
        mut actual: UserOutPtr<u32>,
    ) -> ZxResult {
        // FIXME this read operation should bind to serial-read
        // now just returen nothing read
        info!(
            "debug.read: handle={:#x}, buf=({:?}; {:#x})",
            handle, buf, buf_size
        );
        actual.write(0)?;
        Ok(())
    }
}
