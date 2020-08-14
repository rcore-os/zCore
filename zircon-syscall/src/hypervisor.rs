use {
    super::*,
    core::mem::size_of,
    zircon_object::{
        dev::{Resource, ResourceKind},
        hypervisor::{Guest, Vcpu, VcpuIo, VcpuReadWriteKind, VcpuState},
        signal::{Port, PortPacket},
        vm::VmarFlags,
    },
};

impl Syscall<'_> {
    /// Creates a guest virtual machine.  
    ///
    /// The guest is a virtual machine that can be run within the hypervisor, with `vmar_handle` used to represent the physical address space of the guest.
    pub fn sys_guest_create(
        &self,
        resource: HandleValue,
        options: u32,
        mut guest_handle: UserOutPtr<HandleValue>,
        mut vmar_handle: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "hypervisor.guest_create: resource={:#x?}, options={:?}",
            resource, options
        );
        if options != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        proc.get_object::<Resource>(resource)?
            .validate(ResourceKind::HYPERVISOR)?;

        let guest = Guest::new()?;
        let vmar = guest.vmar();
        let guest_handle_value = proc.add_handle(Handle::new(guest, Rights::DEFAULT_GUEST));
        guest_handle.write(guest_handle_value)?;

        let vmar_flags = vmar.get_flags();
        let mut vmar_rights = Rights::DEFAULT_VMAR;
        if vmar_flags.contains(VmarFlags::CAN_MAP_READ) {
            vmar_rights.insert(Rights::READ);
        }
        if vmar_flags.contains(VmarFlags::CAN_MAP_WRITE) {
            vmar_rights.insert(Rights::WRITE);
        }
        if vmar_flags.contains(VmarFlags::CAN_MAP_EXECUTE) {
            vmar_rights.insert(Rights::EXECUTE);
        }
        let vmar_handle_value = proc.add_handle(Handle::new(vmar, vmar_rights));
        vmar_handle.write(vmar_handle_value)?;
        Ok(())
    }

    /// Set a trap within a guest.  
    pub fn sys_guest_set_trap(
        &self,
        handle: HandleValue,
        kind: u32,
        addr: u64,
        size: u64,
        port_handle: HandleValue,
        key: u64,
    ) -> ZxResult {
        info!(
            "hypervisor.guest_set_trap: handle={:#x?}, kind={:#x?}, addr={:#x?}, size={:#x?}, port_handle={:#x?}, key={:#x?}",
            handle, kind, addr, size, port_handle, key
        );
        let proc = self.thread.proc();
        let guest = proc.get_object_with_rights::<Guest>(handle, Rights::WRITE)?;
        let port = if port_handle != INVALID_HANDLE {
            Some(Arc::downgrade(&proc.get_object_with_rights::<Port>(
                port_handle,
                Rights::WRITE,
            )?))
        } else {
            None
        };
        guest.set_trap(kind, addr as usize, size as usize, port, key)
    }

    /// Create a VCPU within a guest.  
    ///
    /// The VCPU allows for execution within the virtual machine.
    pub fn sys_vcpu_create(
        &self,
        guest_handle: HandleValue,
        options: u32,
        entry: u64,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "hypervisor.vcpu_create: guest_handle={:#x?}, options={:?}, entry={:#x?}",
            guest_handle, options, entry
        );
        if options != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        let guest = proc.get_object_with_rights::<Guest>(guest_handle, Rights::MANAGE_PROCESS)?;
        let vcpu = Vcpu::new(guest, entry, (*self.thread).clone())?;
        let handle_value = proc.add_handle(Handle::new(vcpu, Rights::DEFAULT_VCPU));
        out.write(handle_value)?;
        Ok(())
    }

    /// Resume execution of a VCPU.  
    pub fn sys_vcpu_resume(
        &self,
        handle: HandleValue,
        mut user_packet: UserOutPtr<PortPacket>,
    ) -> ZxResult {
        info!("hypervisor.vcpu_resume: handle={:#x?}", handle);
        let proc = self.thread.proc();
        let vcpu = proc.get_object_with_rights::<Vcpu>(handle, Rights::EXECUTE)?;
        if !vcpu.same_thread(&self.thread) {
            return Err(ZxError::BAD_STATE);
        }
        let packet = vcpu.resume()?;
        user_packet.write(packet)?;
        Ok(())
    }

    /// Raise an interrupt on a VCPU and may be called from any thread.  
    pub fn sys_vcpu_interrupt(&self, handle: HandleValue, vector: u32) -> ZxResult {
        info!(
            "hypervisor.vcpu_interrupt: handle={:#x?}, vector={:?}",
            handle, vector
        );
        let proc = self.thread.proc();
        let vcpu = proc.get_object_with_rights::<Vcpu>(handle, Rights::SIGNAL)?;
        vcpu.virtual_interrupt(vector)?;
        Ok(())
    }

    /// Read the state of a VCPU.  
    pub fn sys_vcpu_read_state(
        &self,
        handle: HandleValue,
        kind: u32,
        mut user_buffer: UserOutPtr<VcpuState>,
        buffer_size: usize,
    ) -> ZxResult {
        info!(
            "hypervisor.vcpu_read_state: handle={:#x?}, kind={:?}, buffer_size={:?}",
            handle, kind, buffer_size
        );
        if kind != VcpuReadWriteKind::VcpuState as u32 || buffer_size != size_of::<VcpuState>() {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        let vcpu = proc.get_object_with_rights::<Vcpu>(handle, Rights::READ)?;
        if !vcpu.same_thread(&self.thread) {
            return Err(ZxError::BAD_STATE);
        }
        let state = vcpu.read_state()?;
        user_buffer.write(state)?;
        Ok(())
    }

    /// Write the state of a VCPU.  
    ///
    /// > It is only valid to write the state of handle when execution has been paused.
    pub fn sys_vcpu_write_state(
        &self,
        handle: HandleValue,
        kind: u32,
        user_buffer: usize,
        buffer_size: usize,
    ) -> ZxResult {
        info!(
            "hypervisor.vcpu_write_state: handle={:#x?}, kind={:?}, user_buffer={:#x?}, buffer_size={:?}",
            handle, kind, user_buffer, buffer_size
        );
        let proc = self.thread.proc();
        let vcpu = proc.get_object_with_rights::<Vcpu>(handle, Rights::WRITE)?;
        if !vcpu.same_thread(&self.thread) {
            return Err(ZxError::BAD_STATE);
        }

        match VcpuReadWriteKind::try_from(kind) {
            Ok(VcpuReadWriteKind::VcpuState) => {
                if buffer_size != size_of::<VcpuState>() {
                    return Err(ZxError::INVALID_ARGS);
                }
                let state: UserInPtr<VcpuState> = user_buffer.into();
                vcpu.write_state(&state.read()?)?;
            }
            Ok(VcpuReadWriteKind::VcpuIo) => {
                if buffer_size != size_of::<VcpuIo>() {
                    return Err(ZxError::INVALID_ARGS);
                }
                let state: UserInPtr<VcpuIo> = user_buffer.into();
                vcpu.write_io_state(&state.read()?)?;
            }
            Err(_) => return Err(ZxError::INVALID_ARGS),
        }
        Ok(())
    }
}
