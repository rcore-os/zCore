use super::*;
use zircon_object::dev::*;

impl Syscall<'_> {
    pub fn sys_debug_write(&self, buf: UserInPtr<u8>, len: usize) -> ZxResult {
        info!("debug.write: buf=({:?}; {:#x})", buf, len);
        let data = buf.read_array(len)?;
        kernel_hal::serial_write(core::str::from_utf8(&data).unwrap());
        Ok(())
    }

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
        // FIXME: To make 'console' work, now debug_read is a blocking call.
        //        But it should be non-blocking.
        // let mut vec = vec![0u8; buf_size as usize];
        // let len = kernel_hal::serial_read(&mut vec);
        // buf.write_array(&vec[..len])?;
        // actual.write(len as u32)?;
        let c = kernel_hal::serial_getchar().await;
        buf.write_array(&[c])?;
        actual.write(1)?;
        Ok(())
    }
}
