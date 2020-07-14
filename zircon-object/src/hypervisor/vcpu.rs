use {
    crate::{hypervisor::Guest, object::*},
    alloc::sync::Arc,
    rvm::Vcpu as VcpuInner,
    spin::Mutex,
};

pub struct Vcpu {
    base: KObjectBase,
    _counter: CountHelper,
    inner: Mutex<VcpuInner>,
}

impl_kobject!(Vcpu);
define_count_helper!(Vcpu);

impl Vcpu {
    pub fn new(guest: Arc<Guest>, entry: u64) -> ZxResult<Arc<Self>> {
        Ok(Arc::new(Vcpu {
            base: KObjectBase::new(),
            _counter: CountHelper::new(),
            inner: Mutex::new(VcpuInner::new(entry, guest.rvm_geust())?),
        }))
    }

    pub fn virtual_interrupt(&self, vector: u32) -> ZxResult {
        self.inner
            .lock()
            .virtual_interrupt(vector)
            .map_err(From::from)
    }
}
