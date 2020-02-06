use {super::*, bitflags::bitflags, zircon_object::vm::*};

impl Syscall {
    pub fn sys_vmar_allocate(
        &self,
        parent_vmar: HandleValue,
        options: u32,
        offset: u64,
        size: u64,
        mut out_child_vmar: UserOutPtr<HandleValue>,
        mut out_child_addr: UserOutPtr<usize>,
    ) -> ZxResult<usize> {
        let options = VmOptions::from_bits(options).ok_or(ZxError::INVALID_ARGS)?;
        info!(
            "vmar.allocate: parent={:?}, options={:?}, offset={:?}, size={:?}",
            parent_vmar, options, offset, size,
        );
        let offset = if options.contains(VmOptions::SPECIFIC) {
            Some(offset as usize)
        } else if offset == 0 {
            None
        } else {
            return Err(ZxError::INVALID_ARGS);
        };
        // TODO: process options
        let perm_rights = options.to_rights();
        let proc = self.thread.proc();
        let parent = proc.get_object_with_rights::<VmAddressRegion>(parent_vmar, perm_rights)?;
        let child = parent.create_child(offset, size as usize)?;
        let child_addr = child.addr();
        let child_handle = proc.add_handle(Handle::new(child, Rights::DEFAULT_VMAR | perm_rights));
        out_child_vmar.write(child_handle)?;
        out_child_addr.write(child_addr)?;
        Ok(0)
    }
}

bitflags! {
    struct VmOptions: u32 {
        #[allow(clippy::identity_op)]
        const PERM_READ             = 1 << 0;
        const PERM_WRITE            = 1 << 1;
        const PERM_EXECUTE          = 1 << 2;
        const COMPACT               = 1 << 3;
        const SPECIFIC              = 1 << 4;
        const SPECIFIC_OVERWRITE    = 1 << 5;
        const CAN_MAP_SPECIFIC      = 1 << 6;
        const CAN_MAP_READ          = 1 << 7;
        const CAN_MAP_WRITE         = 1 << 8;
        const CAN_MAP_EXECUTE       = 1 << 9;
        const MAP_RANGE             = 1 << 10;
        const REQUIRE_NON_RESIZABLE = 1 << 11;
        const ALLOW_FAULTS          = 1 << 12;
    }
}

impl VmOptions {
    fn to_rights(self) -> Rights {
        let mut rights = Rights::empty();
        if self.contains(VmOptions::CAN_MAP_READ) {
            rights.insert(Rights::READ);
        }
        if self.contains(VmOptions::CAN_MAP_WRITE) {
            rights.insert(Rights::WRITE);
        }
        if self.contains(VmOptions::CAN_MAP_EXECUTE) {
            rights.insert(Rights::EXECUTE);
        }
        rights
    }
}
