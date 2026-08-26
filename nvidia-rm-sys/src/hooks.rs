//! Registration point for kernel-specific operations the vendored NVIDIA RM
//! core needs that this crate cannot implement on its own: PCI config space
//! access, MMIO mapping, and legacy I/O port access. `nvidia-rm-sys` must
//! not depend back on `drivers` (which depends on it), so `drivers`
//! registers real implementations via `register_hooks()` during GPU driver
//! init instead. Until registered, every hook returns a safe "unsupported"
//! default so an unwired build still links and runs (as the smoke test
//! already does).
use lock::Mutex;

pub trait KernelHooks: Sync {
    /// Read `len` (1/2/4) bytes at PCI config offset `offset` for the
    /// device identified by `pci_handle` (whatever `pci_init_handle`
    /// returned, passed back opaquely -- see os_interface::os_pci_init_handle).
    fn pci_config_read(&self, pci_handle: usize, offset: u32, len: u32) -> u32;
    fn pci_config_write(&self, pci_handle: usize, offset: u32, len: u32, value: u32);
    /// Map `size` bytes of physical memory starting at `phys` into kernel
    /// address space, returning the virtual address (0 on failure).
    fn map_kernel_space(&self, phys: u64, size: u64) -> u64;
    fn unmap_kernel_space(&self, virt: u64, size: u64);
    fn io_read(&self, port: u32, len: u32) -> u32;
    fn io_write(&self, port: u32, len: u32, value: u32);
    /// Monotonic time since some fixed point, in nanoseconds.
    fn monotonic_time_ns(&self) -> u64;
    /// Busy-wait for approximately `us` microseconds.
    fn delay_us(&self, us: u32);
}

static HOOKS: Mutex<Option<&'static dyn KernelHooks>> = Mutex::new(None);

/// Called once by `drivers` during NVIDIA GPU init with real
/// implementations backed by Eclipse's PCI/MMIO/timer primitives.
pub fn register_hooks(hooks: &'static dyn KernelHooks) {
    *HOOKS.lock() = Some(hooks);
}

pub(crate) fn with_hooks<R>(default: R, f: impl FnOnce(&dyn KernelHooks) -> R) -> R {
    // Bind and drop the guard *before* calling `f` -- matching on
    // `*HOOKS.lock()` directly would keep the lock held for the entire
    // match arm (including `f(h)`), which would self-deadlock solid on
    // any call path that re-enters `with_hooks` from inside `f` (no
    // current caller does, but the lock gives no warning if one ever
    // does -- this costs nothing and removes the landmine).
    let hooks = *HOOKS.lock();
    match hooks {
        Some(h) => f(h),
        None => default,
    }
}
