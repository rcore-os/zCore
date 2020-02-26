use {
    super::*,
    zircon_object::{resource::*, vm::*},
};

impl Syscall {
    pub fn sys_vmo_create(
        &self,
        size: u64,
        options: u32,
        mut out: UserOutPtr<HandleValue>,
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
        mut buf: UserOutPtr<u8>,
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

    pub fn sys_vmo_write(
        &self,
        handle_value: HandleValue,
        buf: UserInPtr<u8>,
        offset: u64,
        buf_size: usize,
    ) -> ZxResult<usize> {
        info!(
            "vmo.write: handle={:?}, offset={:?}, buf=({:?}; {:?})",
            handle_value, offset, buf, buf_size,
        );
        let proc = self.thread.proc();
        let vmo = proc.get_vmo_with_rights(handle_value, Rights::READ)?;
        vmo.write(offset as usize, &buf.read_array(buf_size)?);
        Ok(0)
    }

    pub fn sys_vmo_replace_as_executable(
        &self,
        handle: HandleValue,
        vmex: HandleValue,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        let proc = self.thread.proc();
        if vmex != INVALID_HANDLE {
            proc.validate_resource(vmex, ResourceKind::VMEX)?;
        } else {
            unimplemented!()
        }
        let _ = proc.get_vmo_and_rights(handle)?;
        let new_handle = proc.dup_handle_operating_rights(handle, |handle_rights| {
            Ok(handle_rights | Rights::EXECUTE)
        })?;
        out.write(new_handle)?;
        Ok(0)
    }

    pub fn sys_vmo_get_size(
        &self,
        handle: HandleValue,
        mut size: UserOutPtr<usize>,
    ) -> ZxResult<usize> {
        let vmo = self.thread.proc().get_vmo_and_rights(handle)?.0;
        size.write(vmo.len())?;
        Ok(0)
    }
}
