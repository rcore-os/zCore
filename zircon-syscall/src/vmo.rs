use {super::*, zircon_object::vm::*};

impl Syscall {
    pub fn sys_vmo_create(
        &self,
        size: u64,
        options: u32,
        out: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        info!(
            "vmo.create: size={:?}, options={:?}, out={:?}",
            size, options, out
        );
        // TODO: options
        let proc = self.thread.proc();
        let vmo = VMObjectPaged::new(pages(size as usize));
        let handle_value = proc.add_handle(Handle::new(vmo, Rights::DEFAULT_VMO));
        out.write(handle_value)?;
        Ok(0)
    }

    pub fn sys_vmo_read(
        &self,
        handle_value: HandleValue,
        buf: UserOutPtr<u8>,
        offset: u64,
        buf_size: usize,
    ) -> ZxResult<usize> {
        info!(
            "vmo.read: handle={:?}, offset={:?}, buf=({:?}; {:?})",
            handle_value, offset, buf, buf_size,
        );
        let proc = self.thread.proc();
        let vmo = proc.get_vmo_with_rights(handle_value, Rights::READ)?;
        // TODO: optimize
        let mut buffer = vec![0u8; buf_size];
        vmo.read(offset as usize, &mut buffer);
        buf.write_array(&buffer)?;
        Ok(0)
    }
}
