use {super::*, core::convert::TryFrom, zircon_object::dev::*};

impl Syscall<'_> {
    #[allow(clippy::too_many_arguments)]
    /// Create a resource object for use with other DDK syscalls.  
    pub fn sys_resource_create(
        &self,
        parent_rsrc: HandleValue,
        options: u32,
        base: u64,
        size: u64,
        name: UserInPtr<u8>,
        name_size: u64,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "resource.create: parent={:#x}, options={:#x}, base={:#X}, size={:#x}",
            parent_rsrc, options, base, size
        );
        let name = name.read_string(name_size as usize)?;
        info!("name={:?}", name);
        let proc = self.thread.proc();
        let parent_rsrc = proc.get_object_with_rights::<Resource>(parent_rsrc, Rights::WRITE)?;
        let kind = ResourceKind::try_from(options & 0xFFFF).map_err(|_| ZxError::INVALID_ARGS)?;
        let flags = ResourceFlags::from_bits(options & 0xFFFF_0000).ok_or(ZxError::INVALID_ARGS)?;
        parent_rsrc.validate_ranged_resource(kind, base as usize, size as usize)?;
        parent_rsrc.check_exclusive(flags)?;
        let rsrc = Resource::create(&name, kind, base as usize, size as usize, flags);
        let handle = proc.add_handle(Handle::new(rsrc, Rights::DEFAULT_RESOURCE));
        out.write(handle)?;
        Ok(())
    }
}
