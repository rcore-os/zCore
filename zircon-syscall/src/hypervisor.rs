use {
    super::*,
    zircon_object::{
        dev::{Resource, ResourceKind},
        hypervisor::Guest,
        vm::VmarFlags,
    },
};

impl Syscall<'_> {
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
}
