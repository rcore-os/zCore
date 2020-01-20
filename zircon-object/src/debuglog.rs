use {crate::object::*, alloc::sync::Arc};

#[allow(dead_code)]
#[repr(C)]
struct dlog_header {
    header: u32,
    datalen: u16,
    flags: u16,
    timestamp: u64,
    pid: u64,
    tid: u64,
}

pub struct DebugLog {
    base: KObjectBase,
    #[allow(dead_code)]
    flags: u32,
}

impl_kobject!(DebugLog);

impl DebugLog {
    pub fn create(flags: u32) -> ZxResult<Arc<Self>> {
        let dlog = Arc::new(DebugLog {
            base: KObjectBase::new(),
            flags,
        });
        Ok(dlog)
    }
}
