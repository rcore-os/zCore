use {
    super::*,
    bitflags::bitflags,
    zircon_object::{resource::*, task::PolicyCondition, vm::*},
};

impl Syscall<'_> {
    pub fn sys_vmo_create(
        &self,
        size: u64,
        options: u32,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "vmo.create: size={:#x?}, options={:#x?}, out={:#x?}",
            size, options, out
        );
        if options & !2u32 != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let resizable = if options == 0 { false } else { true };
        let proc = self.thread.proc();
        let vmo = VmObject::new_paged_with_resizable(resizable, pages(size as usize));
        let handle_value = proc.add_handle(Handle::new(vmo, Rights::DEFAULT_VMO));
        out.write(handle_value)?;
        Ok(())
    }

    pub fn sys_vmo_read(
        &self,
        handle_value: HandleValue,
        mut buf: UserOutPtr<u8>,
        offset: u64,
        buf_size: usize,
    ) -> ZxResult {
        info!(
            "vmo.read: handle={:#x?}, offset={:#x?}, buf=({:#x?}; {:#x?})",
            handle_value, offset, buf, buf_size,
        );
        let proc = self.thread.proc();
        let vmo = proc.get_object_with_rights::<VmObject>(handle_value, Rights::READ)?;
        // TODO: optimize
        let mut buffer = vec![0u8; buf_size];
        vmo.read(offset as usize, &mut buffer);
        buf.write_array(&buffer)?;
        Ok(())
    }

    pub fn sys_vmo_write(
        &self,
        handle_value: HandleValue,
        buf: UserInPtr<u8>,
        offset: u64,
        buf_size: usize,
    ) -> ZxResult {
        info!(
            "vmo.write: handle={:#x?}, offset={:#x?}, buf=({:#x?}; {:#x?})",
            handle_value, offset, buf, buf_size,
        );
        let proc = self.thread.proc();
        let vmo = proc.get_object_with_rights::<VmObject>(handle_value, Rights::WRITE)?;
        vmo.write(offset as usize, &buf.read_array(buf_size)?);
        Ok(())
    }

    pub fn sys_vmo_replace_as_executable(
        &self,
        handle: HandleValue,
        vmex: HandleValue,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "vmo.replace_as_executable: handle={:#x?}, vmex={:#x?}",
            handle, vmex
        );
        let proc = self.thread.proc();
        if vmex != INVALID_HANDLE {
            proc.validate_resource(vmex, ResourceKind::VMEX)?;
        } else {
            proc.check_policy(PolicyCondition::AmbientMarkVMOExec)?;
        }
        let _ = proc.get_object_and_rights::<VmObject>(handle)?;
        let new_handle = proc.dup_handle_operating_rights(handle, |handle_rights| {
            Ok(handle_rights | Rights::EXECUTE)
        })?;
        out.write(new_handle)?;
        Ok(())
    }

    pub fn sys_vmo_get_size(&self, handle: HandleValue, mut size: UserOutPtr<usize>) -> ZxResult {
        info!("vmo.get_size: handle={:?}", handle);
        let proc = self.thread.proc();
        let vmo = proc.get_object::<VmObject>(handle)?;
        size.write(vmo.len())?;
        Ok(())
    }

    pub fn sys_vmo_create_child(
        &self,
        handle_value: HandleValue,
        options: u32,
        offset: usize,
        size: usize,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        let options = VmoCloneFlags::from_bits(options).ok_or(ZxError::INVALID_ARGS)?;
        info!(
            "vmo_create_child: handle={:#x}, options={:?}, offset={:#x}, size={:#x}",
            handle_value, options, offset, size
        );
        if !options.contains(VmoCloneFlags::SNAPSHOT_AT_LEAST_ON_WRITE) {
            return Err(ZxError::NOT_SUPPORTED);
        }
        let proc = self.thread.proc();

        let (vmo, parent_rights) = proc.get_object_and_rights::<VmObject>(handle_value)?;
        if !parent_rights.contains(Rights::DUPLICATE | Rights::READ) {
            return Err(ZxError::ACCESS_DENIED);
        }
        let mut child_rights = parent_rights;
        child_rights.insert(Rights::GET_PROPERTY | Rights::SET_PROPERTY);
        if options.contains(VmoCloneFlags::NO_WRITE) {
            child_rights.remove(Rights::WRITE);
        } else if options.contains(VmoCloneFlags::SNAPSHOT)
            || options.contains(VmoCloneFlags::SNAPSHOT_AT_LEAST_ON_WRITE)
        {
            child_rights.remove(Rights::EXECUTE);
            child_rights.insert(Rights::WRITE);
        };
        info!(
            "parent_rights: {:?} child_rights: {:?}",
            parent_rights, child_rights
        );
        let resizable = options.contains(VmoCloneFlags::RESIZABLE);

        let child_size = roundup_pages(size);
        info!("size of child vmo: {:#x}", child_size);
        let child_vmo = vmo.create_child(resizable, offset as usize, child_size);
        out.write(proc.add_handle(Handle::new(child_vmo, child_rights)))?;
        Ok(())
    }

    pub fn sys_vmo_set_size(&self, handle_value: HandleValue, size: usize) -> ZxResult {
        let proc = self.thread.proc();
        let vmo = proc.get_object_with_rights::<VmObject>(handle_value, Rights::WRITE)?;
        info!(
            "vmo.set_size: handle={:#x}, size={:#x}, current_size={:#x}",
            handle_value,
            size,
            vmo.len(),
        );
        vmo.set_len(size)
    }

    pub fn sys_vmo_op_range(
        &self,
        handle_value: HandleValue,
        op: u32,
        offset: usize,
        len: usize,
        _buffer: UserOutPtr<u8>,
        _buffer_size: usize,
    ) -> ZxResult {
        info!(
            "vmo.op_range: handle={:#x}, op={:#X}, offset={:#x}, len={:#x}, buffer_size={:#x}",
            handle_value, op, offset, len, _buffer_size,
        );
        let proc = self.thread.proc();
        let (vmo, rights) = proc.get_object_and_rights::<VmObject>(handle_value)?;
        if !page_aligned(offset) || !page_aligned(len) {
            return Err(ZxError::INVALID_ARGS);
        }
        match op {
            VMO_OP_COMMIT => {
                if !rights.contains(Rights::WRITE) {
                    return Err(ZxError::ACCESS_DENIED);
                }
                vmo.commit(offset, len);
                Ok(())
            }
            VMO_OP_DECOMMIT => {
                if !rights.contains(Rights::WRITE) {
                    return Err(ZxError::ACCESS_DENIED);
                }
                vmo.decommit(offset, len)
            }
            _ => unimplemented!(),
        }
    }
}

bitflags! {
    struct VmoCloneFlags: u32 {
        #[allow(clippy::identity_op)]
        const SNAPSHOT                   = 1 << 0;
        const RESIZABLE                  = 1 << 2;
        const SLICE                      = 1 << 3;
        const SNAPSHOT_AT_LEAST_ON_WRITE = 1 << 4;
        const NO_WRITE                   = 1 << 5;
    }
}

/// VMO Opcodes (for vmo_op_range)
const VMO_OP_COMMIT: u32 = 1;
const VMO_OP_DECOMMIT: u32 = 2;
