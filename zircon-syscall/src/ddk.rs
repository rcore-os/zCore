use {
    super::*,
    zircon_object::{
        dev::{Bti, Iommu},
        resource::*,
    },
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
        proc.validate_resource(resource, ResourceKind::ROOT)?;
        if desc_size > IOMMU_MAX_DESC_LEN {
            return Err(ZxError::INVALID_ARGS);
        }
        if desc_size != IOMMU_DESC_SIZE {
            return Err(ZxError::INVALID_ARGS);
        }
        let copied_desc = desc.read_array(desc_size)?;
        let iommu = Iommu::create(type_, copied_desc, desc_size);
        let handle = proc.add_handle(Handle::new(iommu, Rights::DEFAULT_CHANNEL));
        warn!("iommu handle value {:#x}", handle);
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
}

const IOMMU_MAX_DESC_LEN: usize = 4096;
const IOMMU_DESC_SIZE: usize = 1;
