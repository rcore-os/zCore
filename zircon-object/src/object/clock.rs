use {
    super::*,
    crate::vm::{VmObject, PAGE_SIZE},
    alloc::sync::Arc,
    core::sync::atomic::{AtomicI64, Ordering},
};

/// A userspace-adjustable clock backed by the platform monotonic clock.
pub struct Clock {
    base: KObjectBase,
    reference_offset: AtomicI64,
    synthetic_offset: AtomicI64,
    mapped_vmo: Option<Arc<VmObject>>,
}

impl_kobject!(Clock);

impl Clock {
    /// Create a clock whose initial value is `backstop`.
    pub fn new(backstop: i64, mappable: bool) -> Arc<Self> {
        let mapped_vmo = mappable.then(|| {
            let vmo = VmObject::new_paged(1);
            // This is the error_bound field in Fuchsia's mapped clock ABI.
            // The rest of the zero-filled page, including all padding, must
            // not disclose kernel data.
            vmo.write(0, &u64::MAX.to_ne_bytes()).unwrap();
            vmo
        });
        Arc::new(Self {
            base: KObjectBase::default(),
            reference_offset: AtomicI64::new(kernel_hal::timer::timer_now().as_nanos() as i64),
            synthetic_offset: AtomicI64::new(backstop),
            mapped_vmo,
        })
    }

    /// Read the current synthetic time.
    pub fn read(&self) -> i64 {
        let now = kernel_hal::timer::timer_now().as_nanos() as i64;
        let reference = self.reference_offset.load(Ordering::Relaxed);
        let synthetic = self.synthetic_offset.load(Ordering::Relaxed);
        synthetic.saturating_add(now.saturating_sub(reference))
    }

    /// Set a reference/synthetic correspondence for subsequent reads.
    pub fn update(&self, reference: i64, synthetic: i64) {
        self.reference_offset.store(reference, Ordering::Relaxed);
        self.synthetic_offset.store(synthetic, Ordering::Relaxed);
    }

    pub fn mapped_vmo(&self) -> Option<Arc<VmObject>> {
        self.mapped_vmo.clone()
    }

    pub const fn mapped_size(&self) -> usize {
        PAGE_SIZE
    }
}
