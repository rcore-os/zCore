use {
    super::*,
    zircon_object::{dev::*, hypervisor::*},
};

impl Syscall<'_> {
    pub fn sys_guest_create(
        &self,
        resource: HandleValue,
        options: u32,
        mut _guest_handle: UserOutPtr<HandleValue>,
        mut _vmar_handle: UserOutPtr<HandleValue>,
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
        let _guest = Guest::new();
        Ok(())
    }
}
