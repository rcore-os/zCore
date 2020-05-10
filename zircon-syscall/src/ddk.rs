use {
    super::*,
    bitflags::bitflags,
    kernel_hal::DevVAddr,
    zircon_object::vm::{page_aligned, VmObject},
    zircon_object::{dev::*, resource::*, signal::*, task::*},
};

impl Syscall<'_> {
    pub fn sys_iommu_create(
        &self,
        resource: HandleValue,
        type_: u32,
        desc: UserInPtr<u8>,
        desc_size: usize,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "iommu.create: resource={:#x}, type={:#x}, desc={:#x?} desc_size={:#x} out={:#x?}",
            resource, type_, desc, desc_size, out
        );
        let proc = self.thread.proc();
        proc.get_object::<Resource>(resource)?
            .validate(ResourceKind::ROOT)?;
        if desc_size > IOMMU_MAX_DESC_LEN {
            return Err(ZxError::INVALID_ARGS);
        }
        if desc_size != IOMMU_DESC_SIZE {
            return Err(ZxError::INVALID_ARGS);
        }
        let copied_desc = desc.read_array(desc_size)?;
        let iommu = Iommu::create(type_, copied_desc, desc_size);
        let handle = proc.add_handle(Handle::new(iommu, Rights::DEFAULT_CHANNEL));
        info!("iommu handle value {:#x}", handle);
        out.write(handle)?;
        Ok(())
    }

    pub fn sys_bti_create(
        &self,
        iommu: HandleValue,
        options: u32,
        bti_id: u64,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "bti.create: iommu={:#x}, options={:?}, bti_id={:#x?}",
            iommu, options, bti_id
        );
        let proc = self.thread.proc();
        if options != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let iommu = proc.get_object::<Iommu>(iommu)?;
        if !iommu.is_valid_bus_txn_id() {
            return Err(ZxError::INVALID_ARGS);
        }
        let bti = Bti::create(iommu, bti_id);
        let handle = proc.add_handle(Handle::new(bti, Rights::DEFAULT_BTI));
        out.write(handle)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sys_bti_pin(
        &self,
        bti: HandleValue,
        options: u32,
        vmo: HandleValue,
        offset: usize,
        size: usize,
        mut addrs: UserOutPtr<DevVAddr>,
        addrs_count: usize,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "bti.pin: bti={:#x}, options={:?}, vmo={:#x}, offset={:#x}, size={:#x}, addrs: {:#x?}, addrs_count: {:#x}",
            bti, options, vmo, offset, size, addrs, addrs_count
        );
        let proc = self.thread.proc();
        let bti = proc.get_object_with_rights::<Bti>(bti, Rights::MAP)?;
        if !page_aligned(offset) || !page_aligned(size) {
            return Err(ZxError::INVALID_ARGS);
        }
        let (vmo, rights) = proc.get_object_and_rights::<VmObject>(vmo)?;
        if !rights.contains(Rights::MAP) {
            return Err(ZxError::ACCESS_DENIED);
        }

        let mut iommu_perms = IommuPerms::empty();
        let options = BtiOptions::from_bits_truncate(options);
        if options.contains(BtiOptions::PERM_READ) {
            if !rights.contains(Rights::READ) {
                return Err(ZxError::ACCESS_DENIED);
            }
            iommu_perms.insert(IommuPerms::PERM_READ)
        }

        if options.contains(BtiOptions::PERM_WRITE) {
            if !rights.contains(Rights::WRITE) {
                return Err(ZxError::ACCESS_DENIED);
            }
            iommu_perms.insert(IommuPerms::PERM_WRITE);
        }

        if options.contains(BtiOptions::PERM_EXECUTE) {
            // NOTE: Check Rights::READ instead of Rights::EXECUTE,
            // because Rights::EXECUTE applies to the execution permission of the host CPU,
            // but ZX_BTI_PERM_EXECUTE applies to transactions initiated by the bus device.
            if !rights.contains(Rights::READ) {
                return Err(ZxError::ACCESS_DENIED);
            }
            iommu_perms.insert(IommuPerms::PERM_EXECUTE);
        }
        if options.contains(BtiOptions::CONTIGUOUS) && options.contains(BtiOptions::COMPRESS) {
            return Err(ZxError::INVALID_ARGS);
        }
        if options.contains(BtiOptions::CONTIGUOUS) && !vmo.is_contiguous() {
            return Err(ZxError::INVALID_ARGS);
        }
        let compress_results = options.contains(BtiOptions::COMPRESS);
        let contiguous = options.contains(BtiOptions::CONTIGUOUS);
        let pmt = bti.pin(vmo, offset, size, iommu_perms)?;
        let encoded_addrs = pmt.as_ref().encode_addrs(compress_results, contiguous)?;
        if encoded_addrs.len() != addrs_count {
            warn!(
                "bti.pin addrs_count = {}, but encoded_addrs.len = {}",
                addrs_count,
                encoded_addrs.len()
            );
            return Err(ZxError::INVALID_ARGS);
        }
        addrs.write_array(&encoded_addrs)?;
        let handle = proc.add_handle(Handle::new(pmt, Rights::INSPECT));
        out.write(handle)?;
        Ok(())
    }

    pub fn sys_pmt_unpin(&self, pmt: HandleValue) -> ZxResult {
        info!("pmt.unpin: pmt={:#x}", pmt);
        let proc = self.thread.proc();
        let pmt = proc.remove_object::<Pmt>(pmt)?;
        pmt.as_ref().unpin_and_remove()
    }

    pub fn sys_bti_release_quarantine(&self, bti: HandleValue) -> ZxResult {
        info!("bti.release_quarantine: bti = {:#x}", bti);
        let proc = self.thread.proc();
        let bti = proc.get_object_with_rights::<Bti>(bti, Rights::WRITE)?;
        bti.release_quarantine();
        Ok(())
    }

    pub fn sys_pc_firmware_tables(
        &self,
        resource: HandleValue,
        mut acpi_rsdp_ptr: UserOutPtr<u64>,
        mut smbios_ptr: UserOutPtr<u64>,
    ) -> ZxResult {
        info!("pc_firmware_tables: handle={:?}", resource);
        let proc = self.thread.proc();
        proc.get_object::<Resource>(resource)?
            .validate(ResourceKind::ROOT)?;
        let (acpi_rsdp, smbios) = kernel_hal::pc_firmware_tables();
        acpi_rsdp_ptr.write(acpi_rsdp)?;
        smbios_ptr.write(smbios)?;
        Ok(())
    }

    pub fn sys_interrupt_create(
        &self,
        resource: HandleValue,
        _src_num: u32,
        options: u32,
        mut out: UserOutPtr<HandleValue>
    ) -> ZxResult {
        info!("interrupt.create: handle={:?} options={:?}", resource, options);
        let proc = self.thread.proc();
        if (options & INTERRUPT_VIRTUAL) == 0 {
            // let resource = proc.get_object::<Resource>(resource)?;
            error!("unimplemented: interrupt_create: handle={:?} options={:?}", resource, options);
            return Err(ZxError::NOT_SUPPORTED);
        } else {
            let interrupt = Interrupt::new_virtual(options)?;
            let handle = proc.add_handle(Handle::new(interrupt, Rights::DEFAULT_INTERRUPT));
            out.write(handle)?;
        }
        Ok(())
    }

    pub fn sys_interrupt_bind(
        &self,
        interrupt: HandleValue,
        port: HandleValue,
        key: u64,
        options: u32,
    ) -> ZxResult {
        info!("interrupt.bind: interrupt={:?} port={:?} key={:?} options={:?}", interrupt, port, key, options);
        let proc = self.thread.proc();
        let interrupt = proc.get_object_with_rights::<Interrupt>(interrupt, Rights::READ)?;
        let port = proc.get_object_with_rights::<Port>(port, Rights::WRITE)?;
        if !port.can_bind_to_interrupt() {
            return Err(ZxError::WRONG_TYPE);
        }
        if options == InterruptOp::Bind as _ {
            interrupt.bind(port, key)
        } else if options == InterruptOp::Unbind as _ {
            interrupt.unbind(port)
        } else {
            Err(ZxError::INVALID_ARGS)
        }
    }

    pub fn sys_interrupt_trigger(&self, interrupt: HandleValue, options: u32, timestamp: i64) -> ZxResult {
        info!("interrupt.trigger: interrupt={:?} options={:?} timestamp={:?}", interrupt, options, timestamp);
        let interrupt = self.thread.proc().get_object_with_rights::<Interrupt>(interrupt, Rights::SIGNAL)?;
        interrupt.trigger(timestamp)
    }

    pub fn sys_interrupt_ack(&self, interrupt: HandleValue) -> ZxResult {
        info!("interupt.ack: interrupt={:?}", interrupt);
        let interrupt = self.thread.proc().get_object_with_rights::<Interrupt>(interrupt, Rights::WRITE)?;
        interrupt.ack()
    }

    pub fn sys_interrupt_destroy(&self, interrupt: HandleValue) -> ZxResult {
        info!("interupt.destory: interrupt={:?}", interrupt);
        let interrupt = self.thread.proc().get_object::<Interrupt>(interrupt)?;
        interrupt.destroy()
    }

    pub async fn sys_interrupt_wait(&self, interrupt: HandleValue, mut out: UserOutPtr<i64>) -> ZxResult {
        info!("interrupt.wait: handle={:?}", interrupt);
        assert_eq!(core::mem::size_of::<PortPacket>(), 48);
        let proc = self.thread.proc();
        let interrupt = proc.get_object_with_rights::<Interrupt>(interrupt, Rights::WAIT)?;
        let future = interrupt.wait();
        pin_mut!(future);
        let timestamp = self
            .thread
            .blocking_run(future, ThreadState::BlockedInterrupt, Deadline::forever().into())
            .await?;
        out.write_if_not_null(timestamp)?;
        Ok(())
    }
}

const IOMMU_MAX_DESC_LEN: usize = 4096;
const IOMMU_DESC_SIZE: usize = 1;

bitflags! {
    struct BtiOptions: u32 {
        #[allow(clippy::identity_op)]
        const PERM_READ             = 1 << 0;
        const PERM_WRITE            = 1 << 1;
        const PERM_EXECUTE          = 1 << 2;
        const COMPRESS              = 1 << 3;
        const CONTIGUOUS            = 1 << 4;
    }
}

enum InterruptOp {
    Bind = 0,
    Unbind = 1,
}
