use super::*;
use zircon_object::dev::*;

impl Syscall<'_> {
    /// Write debug info to the serial port.
    pub fn sys_debug_write(&self, buf: UserInPtr<u8>, len: usize) -> ZxResult {
        info!("debug.write: buf=({:?}; {:#x})", buf, len);
        let data = buf.read_array(len)?;
        kernel_hal::console::console_write(core::str::from_utf8(&data).unwrap());
        Ok(())
    }

    /// Read debug info from the serial port.
    pub async fn sys_debug_read(
        &self,
        handle: HandleValue,
        mut buf: UserOutPtr<u8>,
        buf_size: u32,
        mut actual: UserOutPtr<u32>,
    ) -> ZxResult {
        info!(
            "debug.read: handle={:#x}, buf=({:?}; {:#x})",
            handle, buf, buf_size
        );
        let proc = self.thread.proc();
        proc.get_object::<Resource>(handle)?
            .validate(ResourceKind::ROOT)?;
        let mut vec = vec![0u8; buf_size as usize];
        let len = kernel_hal::console::console_read(&mut vec).await;
        buf.write_array(&vec[..len])?;
        actual.write(len as u32)?;
        Ok(())
    }
}
