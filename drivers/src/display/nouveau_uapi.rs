//! Nouveau-compatible DRM driver-specific ioctl surface, for `NvidiaGpu`.
//!
//! # Why this exists
//!
//! Mesa's OpenGL/Vulkan acceleration for NVIDIA hardware (`nouveau_dri.so`,
//! NVK) is not something Eclipse can vendor: it is ordinary open-source
//! Linux userspace, already built for x86_64 by Alpine, and it already runs
//! unmodified under Eclipse's syscall layer -- same as Xorg/labwc/busybox.
//! What it needs from the KERNEL side is `nouveau.ko`'s ioctl protocol
//! (`include/uapi/drm/nouveau_drm.h`), which is a stable, public, documented
//! contract -- unlike NVIDIA's own closed userspace driver, which speaks an
//! undocumented private protocol to `nvidia.ko`. This module reimplements
//! that public contract against `NvidiaGpu`, the same way
//! `linux-object/src/fs/devfs/drm.rs` reimplements the generic DRM/KMS UAPI
//! (see `docs/README-drm.md`).
//!
//! # Scope of THIS milestone -- read before extending
//!
//! This was written in a sandbox with no NVIDIA GPU, no `/dev/kvm`, and no
//! QEMU at all -- nothing here has run against real hardware. To keep that
//! honest, every operation either:
//!   (a) reuses an already hardware-exercised entry point verbatim
//!       (`nvidia_rm_sys::rm_init::step16`/`step17`, the same calls
//!       `/proc/gpustep16`/`gpustep17` make), or
//!   (b) is pure bookkeeping with no hardware/register access at all
//!       (the GEM handle table, the VRAM bitmap allocator -- which already
//!       existed as dead code, see `NvidiaVramAllocator`), or
//!   (c) is explicitly refused with `EOPNOTSUPP`/`ENOSYS` and a log line,
//!       never silently faked.
//!
//! Concretely implemented: GETPARAM, CHANNEL_ALLOC/FREE (exactly one
//! channel, backed by the existing step16+step17 ladder), GEM_NEW/INFO/
//! CPU_PREP/CPU_FINI (VRAM domain only), VM_INIT. Deliberately refused with
//! `EOPNOTSUPP`: VM_BIND, EXEC -- submitting an arbitrary, Mesa-built
//! command buffer needs a new general-purpose submission path in
//! `nvidia-rm-sys` (today's `step18`/`step19` submit one hardcoded,
//! hand-authored kernel each, not arbitrary content) and a real GPU-VA
//! binding path. That is real, scoped follow-up work, not something to fake
//! here. See `docs/README-nouveau-uapi.md` for the full ioctl-by-ioctl
//! status table and what to test first on real hardware.
//!
//! Entirely opt-in via `nvidia.nouveau_uapi` on the kernel cmdline
//! (`set_enabled`, called from `zCore/src/main.rs`). Disabled by default:
//! `NvidiaGpu::ioctl` behaves exactly as before, byte for byte.

use core::mem::size_of;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Called from `zCore`'s cmdline parsing when `nvidia.nouveau_uapi` is
/// present. See the module doc for why this defaults to off.
pub fn set_enabled(v: bool) {
    ENABLED.store(v, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

static FENCE_PAYLOAD_COUNTER: AtomicU32 = AtomicU32::new(1);

/// A fresh, never-zero value to write into `eclipse_rm_exec_submit_signaled`'s
/// kernel-owned fence semaphore each call, so a stale value left over from a
/// previous submission (or the zero the landing zone is cleared to) can never
/// be mistaken for this call's own completion.
pub(super) fn next_fence_payload() -> u32 {
    0x8000_0000 | (FENCE_PAYLOAD_COUNTER.fetch_add(1, Ordering::Relaxed) & 0x7FFF_FFFF)
}

// --- Linux errno values used below (matches linux-object's translation) ---
pub(super) const ENOENT: i32 = 2;
pub(super) const EIO: i32 = 5;
pub(super) const ENOMEM: i32 = 12;
pub(super) const EBUSY: i32 = 16;
pub(super) const ENODEV: i32 = 19;
pub(super) const EINVAL: i32 = 22;
pub(super) const ENOSYS: i32 = 38;
pub(super) const EOPNOTSUPP: i32 = 95;

// --- Linux ioctl encoding (`_IOC`/`_IOWR`, include/uapi/asm-generic/ioctl.h) ---
// Verified against this file's own already-hardcoded DRM_IOCTL_MODE_CREATE_DUMB
// (0xC02064B2): dir=3(RW) type='d'(0x64) nr=0xB2 size=32 ->
// (3<<30)|(0x64<<8)|0xB2|(32<<16) == 0xC02064B2. Same formula here.
const fn drm_ioc(dir: u32, nr: u32, size: usize) -> u32 {
    (dir << 30) | (0x64u32 << 8) | (nr & 0xff) | (((size as u32) & 0x3fff) << 16)
}
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;
const fn drm_iow(nr: u32, size: usize) -> u32 {
    drm_ioc(IOC_WRITE, nr, size)
}
const fn drm_iowr(nr: u32, size: usize) -> u32 {
    drm_ioc(IOC_WRITE | IOC_READ, nr, size)
}

/// `DRM_COMMAND_BASE` (drm.h) -- start of the driver-private ioctl range.
const DRM_COMMAND_BASE: u32 = 0x40;

// --- DRM_NOUVEAU_* command offsets (nouveau_drm.h) ---
const DRM_NOUVEAU_GETPARAM: u32 = 0x00;
const DRM_NOUVEAU_CHANNEL_ALLOC: u32 = 0x02;
const DRM_NOUVEAU_CHANNEL_FREE: u32 = 0x03;
const DRM_NOUVEAU_VM_INIT: u32 = 0x10;
const DRM_NOUVEAU_VM_BIND: u32 = 0x11;
const DRM_NOUVEAU_EXEC: u32 = 0x12;
const DRM_NOUVEAU_GEM_NEW: u32 = 0x40;
const DRM_NOUVEAU_GEM_CPU_PREP: u32 = 0x42;
const DRM_NOUVEAU_GEM_CPU_FINI: u32 = 0x43;
const DRM_NOUVEAU_GEM_INFO: u32 = 0x44;

// --- Full ioctl numbers, as Mesa's libdrm_nouveau actually issues them ---
pub(super) const DRM_IOCTL_NOUVEAU_GETPARAM: u32 = drm_iowr(
    DRM_COMMAND_BASE + DRM_NOUVEAU_GETPARAM,
    size_of::<DrmNouveauGetparam>(),
);
pub(super) const DRM_IOCTL_NOUVEAU_CHANNEL_ALLOC: u32 = drm_iowr(
    DRM_COMMAND_BASE + DRM_NOUVEAU_CHANNEL_ALLOC,
    size_of::<DrmNouveauChannelAlloc>(),
);
pub(super) const DRM_IOCTL_NOUVEAU_CHANNEL_FREE: u32 = drm_iow(
    DRM_COMMAND_BASE + DRM_NOUVEAU_CHANNEL_FREE,
    size_of::<DrmNouveauChannelFree>(),
);
pub(super) const DRM_IOCTL_NOUVEAU_VM_INIT: u32 = drm_iowr(
    DRM_COMMAND_BASE + DRM_NOUVEAU_VM_INIT,
    size_of::<DrmNouveauVmInit>(),
);
pub(super) const DRM_IOCTL_NOUVEAU_VM_BIND: u32 = drm_iowr(
    DRM_COMMAND_BASE + DRM_NOUVEAU_VM_BIND,
    size_of::<DrmNouveauVmBind>(),
);
pub(super) const DRM_IOCTL_NOUVEAU_EXEC: u32 = drm_iowr(
    DRM_COMMAND_BASE + DRM_NOUVEAU_EXEC,
    size_of::<DrmNouveauExec>(),
);
pub(super) const DRM_IOCTL_NOUVEAU_GEM_NEW: u32 = drm_iowr(
    DRM_COMMAND_BASE + DRM_NOUVEAU_GEM_NEW,
    size_of::<DrmNouveauGemNew>(),
);
pub(super) const DRM_IOCTL_NOUVEAU_GEM_CPU_PREP: u32 = drm_iow(
    DRM_COMMAND_BASE + DRM_NOUVEAU_GEM_CPU_PREP,
    size_of::<DrmNouveauGemCpuPrep>(),
);
pub(super) const DRM_IOCTL_NOUVEAU_GEM_CPU_FINI: u32 = drm_iow(
    DRM_COMMAND_BASE + DRM_NOUVEAU_GEM_CPU_FINI,
    size_of::<DrmNouveauGemCpuFini>(),
);
pub(super) const DRM_IOCTL_NOUVEAU_GEM_INFO: u32 = drm_iowr(
    DRM_COMMAND_BASE + DRM_NOUVEAU_GEM_INFO,
    size_of::<DrmNouveauGemInfo>(),
);

// --- NOUVEAU_GETPARAM_* selectors ---
pub(super) const NOUVEAU_GETPARAM_PCI_VENDOR: u64 = 3;
pub(super) const NOUVEAU_GETPARAM_PCI_DEVICE: u64 = 4;
pub(super) const NOUVEAU_GETPARAM_BUS_TYPE: u64 = 5;
pub(super) const NOUVEAU_GETPARAM_FB_SIZE: u64 = 8;
pub(super) const NOUVEAU_GETPARAM_AGP_SIZE: u64 = 9;
pub(super) const NOUVEAU_GETPARAM_PTIMER_TIME: u64 = 10;
pub(super) const NOUVEAU_GETPARAM_CHIPSET_ID: u64 = 11;
/// GPC/TPC/MP topology, chip-specific. Real value must come from the RM (it
/// knows the floorswept config); faking it wrong under-sizes the shader TLS
/// buffer and faults the GPU, so this milestone returns EINVAL and lets the
/// boot log flag that Mesa wanted it.
pub(super) const NOUVEAU_GETPARAM_GRAPH_UNITS: u64 = 13;
/// Max pushbuffers per EXEC ioctl (new submission uAPI). This driver caps EXEC
/// at 64 pushes, so it answers exactly that.
pub(super) const NOUVEAU_GETPARAM_EXEC_PUSH_MAX: u64 = 17;
pub(super) const NOUVEAU_GETPARAM_HAS_BO_USAGE: u64 = 15;
pub(super) const NOUVEAU_GETPARAM_HAS_PAGEFLIP: u64 = 16;
pub(super) const NOUVEAU_GETPARAM_VRAM_BAR_SIZE: u64 = 18;
pub(super) const NOUVEAU_GETPARAM_VRAM_USED: u64 = 19;
pub(super) const NOUVEAU_GETPARAM_HAS_VMA_TILEMODE: u64 = 20;

// --- NOUVEAU_GEM_DOMAIN_* flags ---
pub(super) const NOUVEAU_GEM_DOMAIN_VRAM: u32 = 1 << 1;

// --- DRM_NOUVEAU_VM_BIND_OP_* ---
pub(super) const VM_BIND_OP_MAP: u32 = 0x0;
pub(super) const VM_BIND_OP_UNMAP: u32 = 0x1;

// --- DRM_NOUVEAU_SYNC_* (drm_nouveau_sync.flags) ---
pub(super) const SYNC_TIMELINE_SYNCOBJ: u32 = 0x1;
pub(super) const SYNC_TYPE_MASK: u32 = 0xf;

// --- Structs, field-for-field identical to nouveau_drm.h (natural C layout) ---

#[repr(C)]
pub(super) struct DrmNouveauGetparam {
    pub param: u64,
    pub value: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct DrmNouveauChannelAllocSubchan {
    pub handle: u32,
    pub grclass: u32,
}

#[repr(C)]
pub(super) struct DrmNouveauChannelAlloc {
    pub fb_ctxdma_handle: u32,
    pub tt_ctxdma_handle: u32,
    pub channel: i32,
    pub pushbuf_domains: u32,
    pub notifier_handle: u32,
    pub subchan: [DrmNouveauChannelAllocSubchan; 8],
    pub nr_subchan: u32,
}

#[repr(C)]
pub(super) struct DrmNouveauChannelFree {
    pub channel: i32,
}

#[repr(C)]
pub(super) struct DrmNouveauGemInfo {
    pub handle: u32,
    pub domain: u32,
    pub size: u64,
    pub offset: u64,
    pub map_handle: u64,
    pub tile_mode: u32,
    pub tile_flags: u32,
}

#[repr(C)]
pub(super) struct DrmNouveauGemNew {
    pub info: DrmNouveauGemInfo,
    pub channel_hint: u32,
    pub align: u32,
}

#[repr(C)]
pub(super) struct DrmNouveauGemCpuPrep {
    pub handle: u32,
    pub flags: u32,
}

#[repr(C)]
pub(super) struct DrmNouveauGemCpuFini {
    pub handle: u32,
}

#[repr(C)]
pub(super) struct DrmNouveauVmInit {
    pub kernel_managed_addr: u64,
    pub kernel_managed_size: u64,
}

#[repr(C)]
#[allow(dead_code)] // fields read for validation only in this milestone (VM_BIND itself is EOPNOTSUPP)
pub(super) struct DrmNouveauVmBindOp {
    pub op: u32,
    pub flags: u32,
    pub handle: u32,
    pub pad: u32,
    pub addr: u64,
    pub bo_offset: u64,
    pub range: u64,
}

#[repr(C)]
#[allow(dead_code)]
pub(super) struct DrmNouveauVmBind {
    pub op_count: u32,
    pub flags: u32,
    pub wait_count: u32,
    pub sig_count: u32,
    pub wait_ptr: u64,
    pub sig_ptr: u64,
    pub op_ptr: u64,
}

#[repr(C)]
pub(super) struct DrmNouveauSync {
    pub flags: u32,
    pub handle: u32,
    pub timeline_value: u64,
}

#[repr(C)]
#[allow(dead_code)]
pub(super) struct DrmNouveauExecPush {
    pub va: u64,
    pub va_len: u32,
    pub flags: u32,
}

#[repr(C)]
#[allow(dead_code)]
pub(super) struct DrmNouveauExec {
    pub channel: u32,
    pub push_count: u32,
    pub wait_count: u32,
    pub sig_count: u32,
    pub wait_ptr: u64,
    pub sig_ptr: u64,
    pub push_ptr: u64,
}

// --- Driver-side bookkeeping (Eclipse-internal, not part of the UAPI wire format) ---

/// The single channel this milestone supports, cached after `CHANNEL_ALLOC`
/// reuses the existing step16+step17 bring-up ladder. `h_vas` and
/// `notifier_handle` are real RM object handles from that ladder; nothing
/// here is invented.
pub(super) struct NouveauChannelState {
    pub h_vas: u32,
    pub notifier_handle: u32,
    /// pid that issued `CHANNEL_ALLOC`, pushed down from `linux-object`'s
    /// ioctl dispatch (this crate can't learn it itself -- see
    /// `DrmScheme::ioctl_owned`'s doc). Used by `nouveau_release_process`
    /// to reclaim the channel (and everything bound in it) if this
    /// process exits without an explicit `CHANNEL_FREE`. 0 (no known
    /// caller, e.g. `ioctl` instead of `ioctl_owned`) never matches a
    /// real exiting pid, so such a channel is simply never auto-reclaimed.
    pub owner_pid: u64,
}

/// A GEM object allocated through `GEM_NEW`. Backed by a real RM memory
/// object (`nvidia_rm_sys::rm_init::gem_alloc_vram`, `NV01_MEMORY_LOCAL_USER`
/// -- the RM's own VRAM heap, same class `step17` uses for USERD), not a
/// separate allocator carving up the same physical range out-of-band.
pub(super) struct NouveauGemObject {
    /// Handle returned to userspace (nouveau-uAPI wire format). Distinct
    /// from `h_memory`: this is Eclipse's own counter, not an RM handle.
    /// Starts at `0x8000_0001` (see `NvidiaGpu::new`'s
    /// `nouveau_gem_next_handle` init) so it can never collide with
    /// `linux-object`'s own `DRM_STATE` handle ids -- both are decoded
    /// from the same fake-mmap-offset space by `DrmDev::get_vmo`.
    pub handle: u32,
    /// The real RM memory object handle backing this allocation.
    pub h_memory: u32,
    pub size: u64,
    /// BAR1-relative CPU physical address (`gem_map_cpu`'s `AT_CPU`
    /// offset), if resolving one succeeded at `GEM_NEW` time. `None` means
    /// this object is real (VM_BIND/EXEC both still work) but not
    /// CPU-mmap-able -- see the `GEM_NEW` gap note in
    /// docs/README-nouveau-uapi.md.
    pub phys_addr: Option<u64>,
}

/// A GPU-VA mapping created by `VM_BIND` (`MAP` op), tracked so `UNMAP` can
/// find the RM virtual-range handle to tear down.
pub(super) struct NouveauVmMapping {
    /// Which `NouveauGemObject::handle` this binds.
    pub gem_handle: u32,
    /// RM's virtual-range object handle (`vm_bind_map`'s `h_virt`) -- needed
    /// to `Unmap`+`Free` it later.
    pub h_virt: u32,
    pub va: u64,
    pub size: u64,
}
