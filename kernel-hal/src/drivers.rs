//! Device drivers.

use alloc::{sync::Arc, vec::Vec};
use core::convert::From;

use lock::{RwLock, RwLockReadGuard};

use zcore_drivers::scheme::{
    AudioScheme, BlockScheme, DisplayScheme, DrmScheme, InputScheme, IrqScheme, NetScheme, Scheme,
    UartScheme,
};
use zcore_drivers::{Device, DeviceError};

/// Re-exported modules from crate [`zcore_drivers`].
pub use zcore_drivers::{prelude, scheme, utils};

/// A wrapper of a device array with the same [`Scheme`].
pub struct DeviceList<T: Scheme + ?Sized>(RwLock<Vec<Arc<T>>>);

impl<T: Scheme + ?Sized> DeviceList<T> {
    fn add(&self, dev: Arc<T>) {
        self.0.write().push(dev);
    }

    /// Convert self into a vector.
    pub fn as_vec(&self) -> RwLockReadGuard<'_, Vec<Arc<T>>> {
        self.0.read()
    }

    /// Returns the device at given position, or `None` if out of bounds.
    pub fn try_get(&self, idx: usize) -> Option<Arc<T>> {
        self.0.read().get(idx).cloned()
    }

    /// Returns the device with the given name, or `None` if not found.
    pub fn find(&self, name: &str) -> Option<Arc<T>> {
        self.0.read().iter().find(|d| d.name() == name).cloned()
    }

    /// Returns the first device of this device array, or `None` if it is empty.
    pub fn first(&self) -> Option<Arc<T>> {
        self.try_get(0)
    }

    /// Returns the first device of this device array.
    ///
    /// # Panic
    ///
    /// Panics if the array is empty.
    pub fn first_unwrap(&self) -> Arc<T> {
        self.first()
            .unwrap_or_else(|| panic!("device not initialized: {}", core::any::type_name::<T>()))
    }
}

impl<T: Scheme + ?Sized> Default for DeviceList<T> {
    fn default() -> Self {
        Self(RwLock::new(Vec::new()))
    }
}

#[derive(Default)]
struct AllDeviceList {
    block: DeviceList<dyn BlockScheme>,
    display: DeviceList<dyn DisplayScheme>,
    input: DeviceList<dyn InputScheme>,
    irq: DeviceList<dyn IrqScheme>,
    net: DeviceList<dyn NetScheme>,
    uart: DeviceList<dyn UartScheme>,
    drm: DeviceList<dyn DrmScheme>,
    audio: DeviceList<dyn AudioScheme>,
}

impl AllDeviceList {
    pub fn add_device(&self, dev: Device) {
        match dev {
            Device::Block(d) => self.block.add(d),
            Device::Display(d) => self.display.add(d),
            Device::Input(d) => self.input.add(d),
            Device::Irq(d) => self.irq.add(d),
            Device::Net(d) => self.net.add(d),
            Device::Uart(d) => self.uart.add(d),
            Device::Drm(d) => self.drm.add(d),
            Device::Audio(d) => self.audio.add(d),
        }
    }
}

lazy_static! {
    static ref DEVICES: AllDeviceList = AllDeviceList::default();
}

pub(crate) fn add_device(dev: Device) {
    DEVICES.add_device(dev)
}

/// Returns all devices which implement the [`BlockScheme`].
pub fn all_block() -> &'static DeviceList<dyn BlockScheme> {
    &DEVICES.block
}

/// Returns all devices which implement the [`DisplayScheme`].
pub fn all_display() -> &'static DeviceList<dyn DisplayScheme> {
    &DEVICES.display
}

/// Returns all devices which implement the [`InputScheme`].
pub fn all_input() -> &'static DeviceList<dyn InputScheme> {
    &DEVICES.input
}

/// Returns all devices which implement the [`IrqScheme`].
pub fn all_irq() -> &'static DeviceList<dyn IrqScheme> {
    &DEVICES.irq
}

/// Cached `&'static dyn IrqScheme` for the primary (boot-time) IRQ
/// controller — i.e. what `all_irq().first_unwrap()` would resolve to. The
/// IRQ-dispatch hot path runs on every interrupt (timer × 250 Hz × N CPUs,
/// plus every device IRQ), and going through the regular accessor each time
/// would acquire an `RwLock` and clone an `Arc` per interrupt. The primary
/// controller is registered at boot and never replaced, so we can stash it
/// once and hand out a borrowed reference.
pub fn primary_irq() -> &'static (dyn IrqScheme + Send + Sync + 'static) {
    static PRIMARY_IRQ: spin::Once<Arc<dyn IrqScheme>> = spin::Once::new();
    let arc = PRIMARY_IRQ.call_once(|| all_irq().first_unwrap());
    &**arc
}

/// Returns all devices which implement the [`NetScheme`].
pub fn all_net() -> &'static DeviceList<dyn NetScheme> {
    &DEVICES.net
}

/// Returns all devices which implement the [`UartScheme`].
pub fn all_uart() -> &'static DeviceList<dyn UartScheme> {
    &DEVICES.uart
}

/// Returns all devices which implement the [`DrmScheme`].
pub fn all_drm() -> &'static DeviceList<dyn DrmScheme> {
    &DEVICES.drm
}

/// Returns all devices which implement the [`AudioScheme`].
pub fn all_audio() -> &'static DeviceList<dyn AudioScheme> {
    &DEVICES.audio
}

/// Enables the nouveau-compatible driver-specific ioctl surface on the
/// NVIDIA DRM driver (see `zcore_drivers::display::nouveau_uapi` and
/// `docs/README-nouveau-uapi.md`). No-op where the NVIDIA driver doesn't
/// exist (non-x86_64 builds).
#[cfg(target_arch = "x86_64")]
pub fn set_nouveau_uapi_enabled(v: bool) {
    zcore_drivers::display::set_nouveau_uapi_enabled(v);
}

/// Hands the NVIDIA RM a provider of real per-thread identity (see
/// `zcore_drivers::display::set_rm_thread_id_provider`). No-op off x86_64.
#[cfg(target_arch = "x86_64")]
pub fn set_rm_thread_id_provider(f: fn() -> u64) {
    zcore_drivers::display::set_rm_thread_id_provider(f);
}
#[cfg(not(target_arch = "x86_64"))]
pub fn set_rm_thread_id_provider(_f: fn() -> u64) {}
#[cfg(not(target_arch = "x86_64"))]
pub fn set_nouveau_uapi_enabled(_v: bool) {}

/// Whether the nouveau-compatible ioctl surface is currently enabled --
/// the read side of [`set_nouveau_uapi_enabled`], needed by
/// `linux-syscall` (which, unlike `linux-object`, has no direct
/// `zcore-drivers` dependency on the real kernel target -- only under the
/// `libos` feature) to gate `SYNCOBJ_HANDLE_TO_FD`/`FD_TO_HANDLE`.
#[cfg(target_arch = "x86_64")]
pub fn nouveau_uapi_enabled() -> bool {
    zcore_drivers::display::nouveau_uapi_enabled()
}
#[cfg(not(target_arch = "x86_64"))]
pub fn nouveau_uapi_enabled() -> bool {
    false
}

/// Writes a summary of registered graphics devices to the kernel log (dmesg only).
///
/// `active_console` describes which output path is driving the graphical console, if any.
pub fn klog_graphics_device_summary(active_console: Option<&str>) {
    #[cfg(not(feature = "graphic"))]
    {
        crate::klog_info!(
            "graphics: kernel built without `graphic` feature — framebuffer console disabled"
        );
    }

    let displays = all_display().as_vec();
    let drms = all_drm().as_vec();
    let nd = displays.len();
    let nr = drms.len();

    if nd == 0 && nr == 0 {
        crate::klog_info!("graphics: no framebuffer (Display) or DRM devices registered");
    } else {
        crate::klog_info!(
            "graphics: {} framebuffer device(s), {} DRM / GPU device(s)",
            nd,
            nr
        );

        for (i, d) in displays.iter().enumerate() {
            let info = d.info();
            let fb_kib = info.fb_size.saturating_add(1023) / 1024;
            crate::klog_info!(
                "graphics: display[{}] driver={} {}x{} {:?} pitch={} bpp={} fb≈{} KiB",
                i,
                d.name(),
                info.width,
                info.height,
                info.format,
                info.pitch(),
                info.format.depth(),
                fb_kib,
            );
        }

        for (i, d) in drms.iter().enumerate() {
            let c = d.get_caps();
            crate::klog_info!(
                "graphics: drm[{}] driver={} max_mode={}x{} 3d={} cursor={}",
                i,
                d.name(),
                c.max_width,
                c.max_height,
                c.has_3d,
                c.has_cursor,
            );
        }
    }

    match active_console {
        Some(note) if !note.is_empty() => {
            crate::klog_info!("graphics: active framebuffer console: {}", note);
        }
        _ => {
            #[cfg(feature = "graphic")]
            crate::klog_info!("graphics: active framebuffer console: (none — serial/text only)");
        }
    }
}

impl From<DeviceError> for crate::HalError {
    fn from(err: DeviceError) -> Self {
        warn!("{:?}", err);
        Self
    }
}

/// Ring quarantine + poison-trap for freed DMA blocks, shared by
/// `virtio_dma_dealloc` and `drivers_dma_dealloc`.
///
/// Freed DMA memory used to go straight back to the frame pool, so a device
/// descriptor or a userspace `VmObject::new_physical` mapping still pointing at
/// a just-freed block could write it AFTER `frame_alloc` had already handed the
/// recycled frames to a fresh coroutine stack — the SMP `[null-exec]` zero-smash
/// ("all-zeros usable region, no guard hit"). This holds each freed small block
/// out of circulation for a window (`SLOTS` frees) before returning it, closing
/// the reuse race, and poisons the first word of every page: nothing
/// legitimately touches a quarantined (freed) block, so if the poison is gone at
/// eviction the only thing that could have written it is a stale device/mapping
/// — caught and named as a one-shot `[dma-uaf]` report instead of a silent
/// corruption. Oversized blocks skip the ring (returned now) to bound held RAM.
#[cfg(not(feature = "libos"))]
fn dma_quarantine_dealloc(paddr: crate::PhysAddr, pages: usize) {
    use core::sync::atomic::{AtomicUsize, Ordering};
    const SLOTS: usize = 128;
    const MAX_PAGES: usize = 32;
    const POISON: u64 = 0xDEAD_D3AD_F0F0_F0F0;
    static QUAR_BASE: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
    static QUAR_PAGES: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
    static QUAR_IDX: AtomicUsize = AtomicUsize::new(0);

    let free_now = |base: usize, n: usize| {
        for i in 0..n {
            crate::KHANDLER.frame_dealloc(base + i * crate::PAGE_SIZE);
        }
    };
    if pages == 0 || pages > MAX_PAGES {
        free_now(paddr, pages);
        return;
    }
    let phys_to_va = |pa: usize| pa + crate::KCONFIG.phys_to_virt_offset;
    // Poison the first word of every page of the block we are now quarantining.
    unsafe {
        for i in 0..pages {
            core::ptr::write_volatile(
                phys_to_va(paddr + i * crate::PAGE_SIZE) as *mut u64,
                POISON,
            );
        }
    }
    // Enqueue this block; the block that was in this slot `SLOTS` frees ago is
    // now old enough to actually return to the pool.
    let idx = QUAR_IDX.fetch_add(1, Ordering::Relaxed) % SLOTS;
    let old_pages = QUAR_PAGES[idx].swap(pages, Ordering::AcqRel);
    let old_base = QUAR_BASE[idx].swap(paddr, Ordering::AcqRel);
    if old_base == 0 || old_pages == 0 {
        return; // slot was empty (still filling the ring)
    }
    // Verify the evicted block's poison. Flush the sentinel line first so a
    // (cache-coherent) device DMA that overwrote it is not masked by a stale
    // cache line still holding the poison this CPU wrote.
    let mut smashed_page = usize::MAX;
    let mut smashed_val = 0u64;
    unsafe {
        for i in 0..old_pages {
            let va = phys_to_va(old_base + i * crate::PAGE_SIZE);
            #[cfg(target_arch = "x86_64")]
            core::arch::x86_64::_mm_clflush(va as *const u8);
            let w = core::ptr::read_volatile(va as *const u64);
            if w != POISON {
                smashed_page = i;
                smashed_val = w;
                break;
            }
        }
    }
    if smashed_page != usize::MAX {
        crate::console::serial_write_fmt_spin(format_args!(
            "\n[dma-uaf] STALE WRITE into a freed DMA block while quarantined: \
             paddr={:#x} pages={} page={} word0={:#x} (expected poison {:#x}) — a device \
             descriptor or a userspace mapping wrote memory the driver already freed; this is \
             the SMP null-exec corruptor.\n",
            old_base, old_pages, smashed_page, smashed_val, POISON,
        ));
    }
    free_now(old_base, old_pages);
}

#[cfg(not(feature = "libos"))]
mod virtio_drivers_ffi {
    use crate::{PhysAddr, VirtAddr, KCONFIG, KHANDLER};

    #[no_mangle]
    extern "C" fn virtio_dma_alloc(pages: usize) -> PhysAddr {
        let paddr = KHANDLER.frame_alloc_contiguous(pages, 0).unwrap_or_else(|| {
            panic!(
                "virtio_dma_alloc: no hay {} páginas físicas contiguas (RAM insuficiente o fragmentación)",
                pages
            );
        });
        trace!("alloc DMA: paddr={:#x}, pages={}", paddr, pages);
        paddr
    }

    #[no_mangle]
    extern "C" fn virtio_dma_dealloc(paddr: PhysAddr, pages: usize) -> i32 {
        // See `drivers_dma_dealloc`: record the freed block for the fault
        // path's device/mapping-UAF detector (virtio-gpu/blk rings go here).
        crate::stack_guard::dma_free_note(paddr, pages);
        // Quarantine before the frames re-enter the pool. A device descriptor
        // (NIC RX ring, virtio-gpu/blk, GPU pushbuffer/GEM, NVMe completion) or a
        // userspace `VmObject::new_physical` mapping (the nouveau GEM CPU-mmap
        // path) can still reference a just-freed block and write it after the
        // driver moved on; `frame_alloc` then hands the recycled memory to a
        // fresh coroutine stack and the stale write zeroes a live frame — the SMP
        // `[null-exec]` smash. Holding the block out of circulation for a window
        // closes that race; the poison-trap on eviction names the writer in one
        // run. (See `dma_quarantine_dealloc`.)
        super::dma_quarantine_dealloc(paddr, pages);
        trace!("dealloc DMA: paddr={:#x}, pages={}", paddr, pages);
        0
    }

    #[no_mangle]
    extern "C" fn virtio_phys_to_virt(paddr: PhysAddr) -> VirtAddr {
        paddr + KCONFIG.phys_to_virt_offset
    }

    #[no_mangle]
    extern "C" fn virtio_virt_to_phys(vaddr: VirtAddr) -> PhysAddr {
        #[cfg(any(target_arch = "x86_64", target_arch = "riscv64"))]
        {
            use crate::vm::{GenericPageTable, PageTable};
            let pt = PageTable::from_current();
            if let Ok((paddr, _, _)) = pt.query(vaddr) {
                return paddr;
            }
        }
        vaddr - KCONFIG.phys_to_virt_offset
    }
}

/// Minimal FFI shims for hosted (libos) builds: zcore_drivers' portable code
/// (e.g. the loopback NetScheme) still links against these symbols.
#[cfg(feature = "libos")]
mod drivers_ffi_libos {
    use crate::hal_fn::timer::timer_now;

    #[no_mangle]
    extern "C" fn drivers_timer_now_as_micros() -> u64 {
        timer_now().as_micros() as _
    }

    // The scheduler's `wait_for_interrupt` calls `hal_cpu_idle`. On bare metal
    // that halts the CPU; under libos we run in a host user process where `hlt`
    // would fault, so just spin briefly and let the host scheduler reclaim us.
    #[no_mangle]
    extern "C" fn hal_cpu_idle() {
        core::hint::spin_loop();
    }

    // `zcore_drivers` always links its kernel-log helper; provide the emitter in
    // libos too (forwards to the registered dmesg sink, or a no-op if none),
    // otherwise the hosted build fails to link with `undefined: drivers_klog_emit`.
    use crate::console::klog_emit;
    #[no_mangle]
    extern "C" fn drivers_klog_emit(priority: u8, msg: *const u8, len: usize) {
        if msg.is_null() || len == 0 {
            return;
        }
        let slice = unsafe { core::slice::from_raw_parts(msg, len) };
        if let Ok(s) = core::str::from_utf8(slice) {
            klog_emit(priority, s);
        }
    }
}

#[cfg(not(feature = "libos"))]
mod drivers_ffi {
    use crate::{PhysAddr, VirtAddr, KCONFIG, KHANDLER, PAGE_SIZE};

    #[no_mangle]
    extern "C" fn drivers_dma_alloc(pages: usize) -> PhysAddr {
        let paddr = KHANDLER.frame_alloc_contiguous(pages, 0).unwrap_or_else(|| {
            panic!(
                "drivers_dma_alloc: no hay {} páginas físicas contiguas (RAM insuficiente o fragmentación)",
                pages
            );
        });
        trace!("alloc DMA: paddr={:#x}, pages={}", paddr, pages);
        paddr
    }

    #[no_mangle]
    extern "C" fn drivers_dma_dealloc(paddr: PhysAddr, pages: usize) -> i32 {
        // Restore the kernel-physmap mapping of every page to the default
        // cacheable (WB) state BEFORE returning it to the general frame pool.
        // `drivers_dma_mark_uncached` flips these PTEs to UC for DMA buffers
        // (NIC rings, the NVIDIA RM's UNCACHED sysmem allocs); without this
        // restore, a freed page re-enters the pool with a poisoned UC kernel
        // alias. The next owner (a userspace VMO page, mapped WB in the user's
        // page table) then has two aliases with CONFLICTING memory types —
        // architecturally undefined on x86 — and every kernel access through
        // the physmap (e.g. `PhysFrame::zero()` on commit) goes uncached
        // underneath the user's cached view. Stale dirty lines evicting later
        // silently overwrite the new owner's data: random SIGSEGVs in fresh
        // processes while old ones stay healthy.
        {
            use crate::vm::{GenericPageTable, PageTable};
            use crate::{CachePolicy, MMUFlags};
            let vaddr_base = paddr + KCONFIG.phys_to_virt_offset;
            let mut pt = PageTable::from_current();
            for i in 0..pages {
                let va = vaddr_base + i * PAGE_SIZE;
                if let Ok((_, flags, _)) = pt.query(va) {
                    if flags.bits() & 3 != CachePolicy::Cached as usize {
                        // Flush any lines for this physical page first (lines are
                        // physically tagged, so flushing via this alias covers
                        // every mapping), then re-flag the PTE WB.
                        #[cfg(target_arch = "x86_64")]
                        unsafe {
                            for line in (0..PAGE_SIZE).step_by(64) {
                                core::arch::x86_64::_mm_clflush((va + line) as *const u8);
                            }
                        }
                        let _ = pt.update(va, None, Some(MMUFlags::READ | MMUFlags::WRITE));
                    }
                }
            }
            core::mem::forget(pt);
        }
        // Record the block BEFORE returning it to the pool, so the
        // null-execute/soft-smash fault path can later recognise a corrupted
        // stack frame that was a DMA buffer freed while a device descriptor or
        // a userspace `VmObject::new_physical` mapping still referenced it (the
        // device/mapping UAF the physmap guard cannot see). Diagnostic only.
        crate::stack_guard::dma_free_note(paddr, pages);
        // Quarantine before the frames re-enter the pool. A device descriptor
        // (NIC RX ring, virtio-gpu/blk, GPU pushbuffer/GEM, NVMe completion) or a
        // userspace `VmObject::new_physical` mapping (the nouveau GEM CPU-mmap
        // path) can still reference a just-freed block and write it after the
        // driver moved on; `frame_alloc` then hands the recycled memory to a
        // fresh coroutine stack and the stale write zeroes a live frame — the SMP
        // `[null-exec]` smash. Holding the block out of circulation for a window
        // closes that race; the poison-trap on eviction names the writer in one
        // run. (See `dma_quarantine_dealloc`.)
        super::dma_quarantine_dealloc(paddr, pages);
        trace!("dealloc DMA: paddr={:#x}, pages={}", paddr, pages);
        0
    }

    /// Remap contiguous DMA pages as uncacheable (UC) in the kernel page tables.
    /// Descriptor rings and NIC DMA buffers on bare metal must not use WB without snooping.
    #[no_mangle]
    extern "C" fn drivers_dma_mark_uncached(paddr: PhysAddr, pages: usize) -> i32 {
        use crate::hal_fn::vm::flush_tlb;
        use crate::vm::{GenericPageTable, PageTable};
        use crate::{CachePolicy, MMUFlags, PAGE_SIZE};

        if paddr == 0 || pages == 0 {
            return -1;
        }
        let vaddr = paddr + KCONFIG.phys_to_virt_offset;
        let flags = MMUFlags::READ
            | MMUFlags::WRITE
            | MMUFlags::from_bits_truncate(CachePolicy::Uncached as usize);
        let mut pt = PageTable::from_current();
        for i in 0..pages {
            let va = vaddr + i * PAGE_SIZE;
            match pt.query(va) {
                Ok((_, _, size)) => {
                    // Refuse to re-flag an entry bigger than the page we were
                    // asked about: `update` writes the flags of WHATEVER entry
                    // covers `va`, so on a huge-mapped region it would turn the
                    // whole 2M/1G window UC — poisoning frames owned by other
                    // subsystems/processes. (The x86_64 physmap is 4K-mapped by
                    // rboot, so this only guards future/other-arch layouts.)
                    if size as usize != PAGE_SIZE {
                        trace!(
                            "drivers_dma_mark_uncached: {:#x} covered by a {:?} entry; refusing",
                            va,
                            size
                        );
                        return -1;
                    }
                    // Never pass a paddr here: `update` would `set_addr` the
                    // entry, and `query` returns the offset-adjusted physical
                    // address — harmlessly redundant for a 4K entry, but a
                    // catastrophic repoint should a huge entry ever slip
                    // through. Flags-only is all this function means.
                    if let Err(e) = pt.update(va, None, Some(flags)) {
                        trace!("drivers_dma_mark_uncached: update {:#x} failed {:?}", va, e);
                        return -1;
                    }
                    // WB -> UC transition: flush any cached lines for this page
                    // (physically tagged, so this alias covers all mappings) so
                    // no stale dirty line can later evict on top of device DMA.
                    #[cfg(target_arch = "x86_64")]
                    unsafe {
                        for line in (0..PAGE_SIZE).step_by(64) {
                            core::arch::x86_64::_mm_clflush((va + line) as *const u8);
                        }
                    }
                }
                Err(_) => {
                    if let Err(e) = pt.map_cont(va, PAGE_SIZE, paddr + i * PAGE_SIZE, flags) {
                        trace!("drivers_dma_mark_uncached: map {:#x} failed {:?}", va, e);
                        return -1;
                    }
                }
            }
        }
        flush_tlb(None);
        core::mem::forget(pt);
        0
    }

    /// Verify contiguous DMA pages are mapped uncacheable (PAT/PCD/PWT set in PTE).
    #[no_mangle]
    extern "C" fn drivers_dma_verify_uncached(paddr: PhysAddr, pages: usize) -> i32 {
        use crate::vm::{GenericPageTable, PageTable};
        use crate::{CachePolicy, PAGE_SIZE};

        if paddr == 0 || pages == 0 {
            return -1;
        }
        let vaddr = paddr + KCONFIG.phys_to_virt_offset;
        let pt = PageTable::from_current();
        for i in 0..pages {
            let va = vaddr + i * PAGE_SIZE;
            let Ok((_, flags, _)) = pt.query(va) else {
                return -1;
            };
            let policy = flags.bits() & 3;
            if policy != CachePolicy::Uncached as usize
                && policy != CachePolicy::UncachedDevice as usize
            {
                return -1;
            }
        }
        core::mem::forget(pt);
        0
    }

    #[no_mangle]
    extern "C" fn drivers_phys_to_virt(paddr: PhysAddr) -> VirtAddr {
        paddr + KCONFIG.phys_to_virt_offset
    }

    #[no_mangle]
    extern "C" fn drivers_virt_to_phys(vaddr: VirtAddr) -> PhysAddr {
        #[cfg(any(target_arch = "x86_64", target_arch = "riscv64"))]
        {
            use crate::vm::{GenericPageTable, PageTable};
            let pt = PageTable::from_current();
            if let Ok((paddr, _, _)) = pt.query(vaddr) {
                return paddr;
            }
        }
        vaddr - KCONFIG.phys_to_virt_offset
    }

    use crate::hal_fn::timer::timer_now;
    #[no_mangle]
    extern "C" fn drivers_timer_now_as_micros() -> u64 {
        timer_now().as_micros() as _
    }

    use crate::hal_fn::interrupt::{intr_get, intr_off, intr_on};
    #[no_mangle]
    extern "C" fn drivers_intr_on() {
        intr_on();
    }
    #[no_mangle]
    extern "C" fn drivers_intr_off() {
        intr_off();
    }
    #[no_mangle]
    extern "C" fn drivers_intr_get() -> bool {
        intr_get()
    }

    use crate::console::klog_emit;
    #[no_mangle]
    extern "C" fn drivers_klog_emit(priority: u8, msg: *const u8, len: usize) {
        if msg.is_null() || len == 0 {
            return;
        }
        let slice = unsafe { core::slice::from_raw_parts(msg, len) };
        if let Ok(s) = core::str::from_utf8(slice) {
            klog_emit(priority, s);
        }
    }

    /// Wake tasks blocked on NIC RX (TCP/UDP recv, poll/epoll).
    #[no_mangle]
    extern "C" fn drivers_wake_net_rx_waiters() {
        crate::net::wake_net_rx_waiters();
    }
}
