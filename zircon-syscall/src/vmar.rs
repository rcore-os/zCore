use {super::*, bitflags::bitflags, zircon_object::vm::*};

fn amount_of_alignments(options: u32) -> ZxResult<usize> {
    let mut align_pow2 = (options >> 24) as usize;
    if align_pow2 == 0 {
        align_pow2 = PAGE_SIZE_LOG2;
    }
    if !(PAGE_SIZE_LOG2..=32).contains(&align_pow2) {
        Err(ZxError::INVALID_ARGS)
    } else {
        Ok(1 << align_pow2)
    }
}

impl Syscall<'_> {
    /// Allocate a new subregion.
    ///
    /// Creates a new VMAR within the one specified by `parent_vmar`.
    pub fn sys_vmar_allocate(
        &self,
        parent_vmar: HandleValue,
        options: u32,
        offset: u64,
        size: u64,
        mut out_child_vmar: UserOutPtr<HandleValue>,
        mut out_child_addr: UserOutPtr<usize>,
    ) -> ZxResult {
        let vm_options = VmOptions::from_bits(options).ok_or(ZxError::INVALID_ARGS)?;
        info!(
            "vmar.allocate: parent={:#x?}, options={:#x?}, offset={:#x?}, size={:#x?}",
            parent_vmar, options, offset, size,
        );
        // try to get parent_vmar
        let perm_rights = vm_options.to_rights();
        let proc = self.thread.proc();
        let parent = proc.get_object_with_rights::<VmAddressRegion>(parent_vmar, perm_rights)?;

        if vm_options.intersects(VmOptions::PERM_RXW | VmOptions::MAP_RANGE) {
            return Err(ZxError::INVALID_ARGS);
        }
        // get vmar_flags
        let vmar_flags = vm_options.to_flags();
        if vmar_flags.intersects(
            !(VmarFlags::SPECIFIC
                | VmarFlags::CAN_MAP_SPECIFIC
                | VmarFlags::COMPACT
                | VmarFlags::CAN_MAP_RXW),
        ) {
            return Err(ZxError::INVALID_ARGS);
        }

        // get align
        let align = amount_of_alignments(options)?;

        // get offest with options
        let offset = if vm_options.contains(VmOptions::SPECIFIC) {
            Some(offset as usize)
        } else if vm_options.contains(VmOptions::SPECIFIC_OVERWRITE) {
            unimplemented!()
        } else {
            if offset != 0 {
                return Err(ZxError::INVALID_ARGS);
            }
            None
        };

        let size = roundup_pages(size as usize);
        // check `size`
        if size == 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let child = parent.allocate(offset, size, vmar_flags, align)?;
        let child_addr = child.addr();
        let child_handle = proc.add_handle(Handle::new(child, Rights::DEFAULT_VMAR | perm_rights));
        info!("vmar.allocate: at {:#x?}", child_addr);
        out_child_vmar.write(child_handle)?;
        out_child_addr.write(child_addr)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    /// Add a memory mapping.
    ///
    /// Maps the given VMO into the given virtual memory address region.
    pub fn sys_vmar_map(
        &self,
        vmar_handle: HandleValue,
        options: u32,
        vmar_offset: usize,
        vmo_handle: HandleValue,
        vmo_offset: usize,
        len: usize,
        mut mapped_addr: UserOutPtr<VirtAddr>,
    ) -> ZxResult {
        info!(
            "vmar.map: vmar_handle={:#x?}, options={:#x?}, vmar_offset={:#x?}, vmo_handle={:#x?}, vmo_offset={:#x?}, len={:#x?}",
            vmar_handle, options, vmar_offset, vmo_handle, vmo_offset, len
        );
        let options = VmOptions::from_bits(options).ok_or(ZxError::INVALID_ARGS)?;
        let proc = self.thread.proc();
        let (vmar, vmar_rights) = proc.get_object_and_rights::<VmAddressRegion>(vmar_handle)?;
        let (vmo, vmo_rights) = proc.get_object_and_rights::<VmObject>(vmo_handle)?;
        if !vmo_rights.contains(Rights::MAP) {
            return Err(ZxError::ACCESS_DENIED);
        };
        if options
            .intersects(VmOptions::CAN_MAP_RXW | VmOptions::CAN_MAP_SPECIFIC | VmOptions::COMPACT)
        {
            return Err(ZxError::INVALID_ARGS);
        }
        if options.contains(VmOptions::REQUIRE_NON_RESIZABLE) && vmo.is_resizable() {
            return Err(ZxError::NOT_SUPPORTED);
        }
        // check SPECIFIC options with offset
        let is_specific = options.contains(VmOptions::SPECIFIC)
            || options.contains(VmOptions::SPECIFIC_OVERWRITE);
        if !is_specific && vmar_offset != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        if !vmar_rights.contains(options.to_required_rights()) {
            return Err(ZxError::ACCESS_DENIED);
        }
        let mut permissions = MMUFlags::empty();
        permissions.set(MMUFlags::READ, vmo_rights.contains(Rights::READ));
        permissions.set(MMUFlags::WRITE, vmo_rights.contains(Rights::WRITE));
        permissions.set(MMUFlags::EXECUTE, vmo_rights.contains(Rights::EXECUTE));
        let mut mapping_flags = MMUFlags::USER;
        mapping_flags.set(MMUFlags::READ, options.contains(VmOptions::PERM_READ));
        mapping_flags.set(MMUFlags::WRITE, options.contains(VmOptions::PERM_WRITE));
        mapping_flags.set(MMUFlags::EXECUTE, options.contains(VmOptions::PERM_EXECUTE));
        let overwrite = options.contains(VmOptions::SPECIFIC_OVERWRITE);
        #[cfg(feature = "deny-page-fault")]
        let map_range = true;
        #[cfg(not(feature = "deny-page-fault"))]
        let map_range = options.contains(VmOptions::MAP_RANGE);

        info!(
            "mmuflags: {:?}, is_specific {:?}, overwrite {:?}, map_range {:?}",
            mapping_flags, is_specific, overwrite, map_range
        );
        if map_range && overwrite {
            return Err(ZxError::INVALID_ARGS);
        }
        // Note: we should reject non-page-aligned length here,
        // but since zCore use different memory layout from zircon,
        // we should not reject them and round up them instead
        // TODO: reject non-page-aligned length after we have the same memory layout with zircon
        let len = roundup_pages(len);
        if len == 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let vmar_offset = if is_specific { Some(vmar_offset) } else { None };
        let vaddr = vmar.map_ext(
            vmar_offset,
            vmo,
            vmo_offset,
            len,
            permissions,
            mapping_flags,
            overwrite,
            map_range,
        )?;
        info!("vmar.map: at {:#x?}", vaddr);
        mapped_addr.write(vaddr)?;
        Ok(())
    }

    /// Destroy a virtual memory address region.
    ///
    /// Unmaps all mappings within the given region, and destroys all sub-regions of the region.
    /// > This operation is logically recursive.
    pub fn sys_vmar_destroy(&self, handle_value: HandleValue) -> ZxResult {
        info!("vmar.destroy: handle={:#x?}", handle_value);
        let proc = self.thread.proc();
        let vmar = proc.get_object::<VmAddressRegion>(handle_value)?;
        vmar.destroy()?;
        Ok(())
    }

    /// Set protection of virtual memory pages.
    pub fn sys_vmar_protect(
        &self,
        handle_value: HandleValue,
        options: u32,
        addr: u64,
        len: u64,
    ) -> ZxResult {
        let options = VmOptions::from_bits(options).ok_or(ZxError::INVALID_ARGS)?;
        let rights = options.to_required_rights();
        info!(
            "vmar.protect: handle={:#x}, options={:#x}, addr={:#x}, len={:#x}",
            handle_value, options, addr, len
        );
        let proc = self.thread.proc();
        let vmar = proc.get_object_with_rights::<VmAddressRegion>(handle_value, rights)?;
        if options.intersects(!VmOptions::PERM_RXW) {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut mapping_flags = MMUFlags::empty();
        mapping_flags.set(MMUFlags::READ, options.contains(VmOptions::PERM_READ));
        mapping_flags.set(MMUFlags::WRITE, options.contains(VmOptions::PERM_WRITE));
        mapping_flags.set(MMUFlags::EXECUTE, options.contains(VmOptions::PERM_EXECUTE));
        info!("mmuflags: {:?}", mapping_flags);
        let len = roundup_pages(len as usize);
        if len == 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        vmar.protect(addr as usize, len, mapping_flags)?;
        Ok(())
    }

    /// Unmap virtual memory pages.
    pub fn sys_vmar_unmap(&self, handle_value: HandleValue, addr: usize, len: usize) -> ZxResult {
        info!(
            "vmar.unmap: handle_value={:#x}, addr={:#x}, len={:#x}",
            handle_value, addr, len
        );
        let proc = self.thread.proc();
        let vmar = proc.get_object::<VmAddressRegion>(handle_value)?;
        vmar.unmap(addr, pages(len) * PAGE_SIZE)?;
        Ok(())
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
        const CAN_MAP_RXW           = Self::CAN_MAP_READ.bits | Self::CAN_MAP_EXECUTE.bits | Self::CAN_MAP_WRITE.bits;
        const PERM_RXW           = Self::PERM_READ.bits | Self::PERM_WRITE.bits | Self::PERM_EXECUTE.bits;
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

    fn to_required_rights(self) -> Rights {
        let mut rights = Rights::empty();
        if self.contains(VmOptions::PERM_READ) {
            rights.insert(Rights::READ);
        }
        if self.contains(VmOptions::PERM_WRITE) {
            rights.insert(Rights::WRITE);
        }
        if self.contains(VmOptions::PERM_EXECUTE) {
            rights.insert(Rights::EXECUTE);
        }
        rights
    }

    fn to_flags(self) -> VmarFlags {
        let mut flags = VmarFlags::empty();
        if self.contains(VmOptions::COMPACT) {
            flags.insert(VmarFlags::COMPACT);
        }
        if self.contains(VmOptions::SPECIFIC) {
            flags.insert(VmarFlags::SPECIFIC);
        }
        if self.contains(VmOptions::SPECIFIC_OVERWRITE) {
            flags.insert(VmarFlags::SPECIFIC_OVERWRITE);
        }
        if self.contains(VmOptions::CAN_MAP_SPECIFIC) {
            flags.insert(VmarFlags::CAN_MAP_SPECIFIC);
        }
        if self.contains(VmOptions::CAN_MAP_READ) {
            flags.insert(VmarFlags::CAN_MAP_READ);
        }
        if self.contains(VmOptions::CAN_MAP_WRITE) {
            flags.insert(VmarFlags::CAN_MAP_WRITE);
        }
        if self.contains(VmOptions::CAN_MAP_EXECUTE) {
            flags.insert(VmarFlags::CAN_MAP_EXECUTE);
        }
        if self.contains(VmOptions::REQUIRE_NON_RESIZABLE) {
            flags.insert(VmarFlags::REQUIRE_NON_RESIZABLE);
        }
        if self.contains(VmOptions::ALLOW_FAULTS) {
            flags.insert(VmarFlags::ALLOW_FAULTS);
        }
        flags
    }
}
