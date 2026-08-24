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

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

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

/// Physical address of the RM channel's error notifier, cached at
/// CHANNEL_ALLOC time. The EXEC failure path reads the page through
/// `crate::bus::phys_to_virt` with NO RM calls and NO locks: acquiring the
/// RM's API lock from inside a failure storm is what wedged the machine on
/// real hardware (`DEADLOCK: spinlock(s) stuck >8s`), and before that, the
/// RM's own CPU-mapping of the same page took the kernel down with a page
/// fault. 0 = never captured.
static CHAN_NOTIFIER_PA: AtomicU64 = AtomicU64::new(0);

pub(super) fn set_chan_notifier_pa(pa: u64) {
    CHAN_NOTIFIER_PA.store(pa, Ordering::Relaxed);
}

pub(super) fn chan_notifier_pa_cached() -> Option<u64> {
    match CHAN_NOTIFIER_PA.load(Ordering::Relaxed) {
        0 => None,
        pa => Some(pa),
    }
}

/// NVIF subchannel objects that are backed by REAL RM class objects on a
/// process's RM channel, keyed by the NVIF object token
/// (`nvif_ioctl_new_v0.object`, the pointer-sized cookie userspace passes back
/// in DEL). Entries are `(nvif_object_token, rm_handle, owner_pid)`. The
/// `owner_pid` scopes crash cleanup: with per-process contexts each client's
/// class objects live on ITS OWN channel, so a client's exit must free only its
/// own objects -- draining globally (as this once did) would rip the
/// compositor's and other clients' 3D/copy classes out from under live
/// channels.
static CLASS_OBJECTS: lock::Mutex<alloc::vec::Vec<(u64, u32, u64)>> =
    lock::Mutex::new(alloc::vec::Vec::new());

pub(super) fn class_object_insert(token: u64, rm_handle: u32, owner_pid: u64) {
    CLASS_OBJECTS.lock().push((token, rm_handle, owner_pid));
}

/// Removes and returns the RM handle for `token`, if it was RM-backed.
pub(super) fn class_object_remove(token: u64) -> Option<u32> {
    let mut t = CLASS_OBJECTS.lock();
    t.iter()
        .position(|(k, _, _)| *k == token)
        .map(|i| t.remove(i).1)
}

/// Drains the class objects owned by `pid` (that process's exit / teardown),
/// returning `(token, rm_handle)` for each. Other processes' objects stay.
pub(super) fn class_objects_drain_pid(pid: u64) -> alloc::vec::Vec<(u64, u32)> {
    let mut t = CLASS_OBJECTS.lock();
    let mut out = alloc::vec::Vec::new();
    let mut i = 0;
    while i < t.len() {
        if t[i].2 == pid {
            let (token, h, _) = t.remove(i);
            out.push((token, h));
        } else {
            i += 1;
        }
    }
    out
}

/// Per-context "wedged" latch (bit `i` = context `i`). Set when a client
/// context's EXEC fence times out: its ring is jammed (GPGet frozen) and every
/// further submit would just re-poll a dead fence for the full timeout while
/// holding the RM gate -- which starves the compositor's own rendering and
/// froze the desktop/cursor when a hung GL client kept retrying. Once latched,
/// that context's submits fast-fail (no gate-holding poll) until the process
/// exits and its context is rebuilt fresh. Context 0 (the compositor) is never
/// latched. A lock-free bitmask: MAX_CTX (8) fits in the low bits.
static CTX_WEDGED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

pub(super) fn ctx_is_wedged(ctx_idx: u32) -> bool {
    ctx_idx < 32
        && (CTX_WEDGED.load(core::sync::atomic::Ordering::Relaxed) & (1u32 << ctx_idx)) != 0
}

pub(super) fn ctx_set_wedged(ctx_idx: u32) {
    if ctx_idx < 32 {
        CTX_WEDGED.fetch_or(1u32 << ctx_idx, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Clear the latch for a context slot -- on process exit (before the slot is
/// reused) and when a fresh context is assigned, so a re-launched client gets a
/// clean channel, never the previous tenant's wedged verdict.
pub(super) fn ctx_clear_wedged(ctx_idx: u32) {
    if ctx_idx < 32 {
        CTX_WEDGED.fetch_and(!(1u32 << ctx_idx), core::sync::atomic::Ordering::Relaxed);
    }
}

/// De-dup latch for EXEC submission-failure klogs, keyed by the failure's
/// status signature. Userspace controls the retry rate (labwc respawns and
/// resubmits forever), `klog` writes synchronously to the UART with no
/// filter, and a wedged ring makes EVERY retry fail identically -- on real
/// hardware that storm (hundreds of ~180-byte lines per second, ~15 ms of
/// UART time each) monopolised the console path badly enough to starve other
/// spinlock holders past the 8 s deadlock watchdog. Log only when the
/// signature CHANGES; the first line of a new failure mode is the whole
/// diagnostic, every repeat after it is noise.
static LAST_EXEC_FAILURE_SIG: AtomicU64 = AtomicU64::new(u64::MAX);

pub(super) fn exec_failure_changed(sig: u64) -> bool {
    LAST_EXEC_FAILURE_SIG.swap(sig, Ordering::Relaxed) != sig
}

/// Order-sensitive fold of failure statuses into a signature for
/// [`exec_failure_changed`].
pub(super) fn exec_failure_sig(tag: u64, statuses: &[u32]) -> u64 {
    statuses
        .iter()
        .fold(tag ^ 0x9e37_79b9_7f4a_7c15, |acc, &x| {
            acc.rotate_left(13) ^ x as u64
        })
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

/// `DRM_COMMAND_BASE` (drm.h) -- start of the driver-private ioctl range.
const DRM_COMMAND_BASE: u32 = 0x40;

// --- DRM_NOUVEAU_* command offsets (nouveau_drm.h) ---
const DRM_NOUVEAU_GETPARAM: u32 = 0x00;
const DRM_NOUVEAU_CHANNEL_ALLOC: u32 = 0x02;
const DRM_NOUVEAU_CHANNEL_FREE: u32 = 0x03;
const DRM_NOUVEAU_NVIF: u32 = 0x07;
const DRM_NOUVEAU_VM_INIT: u32 = 0x10;
const DRM_NOUVEAU_VM_BIND: u32 = 0x11;
const DRM_NOUVEAU_EXEC: u32 = 0x12;
const DRM_NOUVEAU_GET_ZCULL_INFO: u32 = 0x13;
const DRM_NOUVEAU_GEM_NEW: u32 = 0x40;
const DRM_NOUVEAU_GEM_PUSHBUF: u32 = 0x41;
const DRM_NOUVEAU_GEM_CPU_PREP: u32 = 0x42;
const DRM_NOUVEAU_GEM_CPU_FINI: u32 = 0x43;
const DRM_NOUVEAU_GEM_INFO: u32 = 0x44;

// NOTE: the canonical `DRM_IOWR(...)`-encoded request numbers used to live
// here, and dispatch matched on them. They are gone on purpose: userspace does
// NOT always encode an ioctl the way the header does -- mesa issues VM_INIT
// through `drmCommandWrite` (_IOW) while the header defines it _IOWR, and NVIF
// multiplexes five different sizes/directions onto one NR. Linux itself
// dispatches driver-private ioctls by NR alone (`_IOC_NR(cmd) -
// DRM_COMMAND_BASE`) and takes the size from its own table, so this driver now
// does the same; see the `NR_*` constants at the end of this file.

// --- Diagnostic: decode + name every nouveau ioctl, handled or not ---
//
// A first-hardware boot's single most useful artifact is the exact list of
// ioctls Mesa issues -- above all *which submission path* it picks: the legacy
// `GEM_PUSHBUF` (classic nvc0 Gallium GL) or the new `EXEC` uAPI (NVK). The
// The dispatch in `nvidia.rs` keys on NR (the low 8 bits -- the stable
// identity of an ioctl, independent of the caller's struct size and
// direction), so this trace decodes the same way and can name what Mesa asked
// for even when the payload size differs from ours.

/// Split a full DRM ioctl request number into `(dir, nr, size)`.
pub(super) fn decode_ioc(request: u32) -> (u32, u32, u32) {
    let dir = (request >> 30) & 0x3;
    let nr = request & 0xff;
    let size = (request >> 16) & 0x3fff;
    (dir, nr, size)
}

/// Human-readable name for a driver-private ioctl NR, covering the whole
/// `nouveau_drm.h` vocabulary -- including ioctls this driver does not
/// implement -- so a trace names what Mesa wanted instead of a bare number.
/// NR is `DRM_COMMAND_BASE + DRM_NOUVEAU_*`; returns `"unknown"` for anything
/// outside nouveau's private range.
pub(super) fn nouveau_ioctl_name(nr: u32) -> &'static str {
    match nr.wrapping_sub(DRM_COMMAND_BASE) {
        DRM_NOUVEAU_GETPARAM => "GETPARAM",
        0x01 => "SETPARAM(deprecated)",
        DRM_NOUVEAU_CHANNEL_ALLOC => "CHANNEL_ALLOC",
        DRM_NOUVEAU_CHANNEL_FREE => "CHANNEL_FREE",
        0x04 => "GROBJ_ALLOC(deprecated)",
        0x05 => "NOTIFIEROBJ_ALLOC(deprecated)",
        0x06 => "GPUOBJ_FREE(deprecated)",
        0x07 => "NVIF",
        0x08 => "SVM_INIT",
        0x09 => "SVM_BIND",
        DRM_NOUVEAU_VM_INIT => "VM_INIT",
        DRM_NOUVEAU_VM_BIND => "VM_BIND",
        DRM_NOUVEAU_EXEC => "EXEC",
        DRM_NOUVEAU_GET_ZCULL_INFO => "GET_ZCULL_INFO",
        DRM_NOUVEAU_GEM_NEW => "GEM_NEW",
        DRM_NOUVEAU_GEM_PUSHBUF => "GEM_PUSHBUF",
        DRM_NOUVEAU_GEM_CPU_PREP => "GEM_CPU_PREP",
        DRM_NOUVEAU_GEM_CPU_FINI => "GEM_CPU_FINI",
        DRM_NOUVEAU_GEM_INFO => "GEM_INFO",
        _ => "unknown",
    }
}

/// First-sight trace de-dup: one bit per ioctl NR. The whole nouveau uAPI is
/// opt-in and only enabled for the NVIDIA GL experiment, so tracing each
/// *distinct* ioctl the first time Mesa issues it gives the full vocabulary
/// (and the submission path) in ~a dozen bounded lines, without flooding the
/// serial console on every per-draw EXEC.
static NR_TRACED: [AtomicBool; 256] = {
    const F: AtomicBool = AtomicBool::new(false);
    [F; 256]
};

/// Log this ioctl by name the first time its NR is seen this boot.
pub(super) fn trace_first_sight(request: u32) {
    let (dir, nr, size) = decode_ioc(request);
    let idx = (nr & 0xff) as usize;
    if !NR_TRACED[idx].swap(true, Ordering::Relaxed) {
        // klog, not log::warn: real-hardware boots run at a log level that
        // drops warn, and this trace -- one bounded line per distinct ioctl --
        // IS the diagnostic. It is already de-duped per NR, so it cannot flood.
        crate::klog_warn!(
            "[nouveau-uapi] first {} : request={:#010x} dir={} nr={:#04x} size={}",
            nouveau_ioctl_name(nr),
            request,
            dir,
            nr,
            size
        );
    }
}

// --- NOUVEAU_GETPARAM_* selectors ---
pub(super) const NOUVEAU_GETPARAM_PCI_VENDOR: u64 = 3;
pub(super) const NOUVEAU_GETPARAM_PCI_DEVICE: u64 = 4;
pub(super) const NOUVEAU_GETPARAM_BUS_TYPE: u64 = 5;
pub(super) const NOUVEAU_GETPARAM_FB_SIZE: u64 = 8;
pub(super) const NOUVEAU_GETPARAM_AGP_SIZE: u64 = 9;
/// NOTE: 14, not 10. Verified against Linux `include/uapi/drm/nouveau_drm.h`
/// (`#define NOUVEAU_GETPARAM_PTIMER_TIME 14`). This was 10 for several
/// milestones, so Mesa's timestamp query hit the unknown-param EINVAL arm
/// while param 10 (unused upstream) answered with a timer value it does not
/// mean.
pub(super) const NOUVEAU_GETPARAM_PTIMER_TIME: u64 = 14;
pub(super) const NOUVEAU_GETPARAM_CHIPSET_ID: u64 = 11;
/// GPC/TPC topology, chip-specific. **Enumeration-fatal**: mesa's
/// `nouveau_ws_device_new` does `if (nouveau_ws_param(fd,
/// NOUVEAU_GETPARAM_GRAPH_UNITS, &value)) goto out_err;` and then unpacks
/// `gpc_count = value & 0xff; tpc_count = (value >> 8) & 0xffff`. Returning
/// EINVAL here silently kills the whole physical device (0 Vulkan GPUs).
/// Linux packs it in `gf100_gr_units()`: `cfg  = gr->gpc_nr;
/// cfg |= gr->tpc_total << 8; cfg |= (u64)gr->rop_nr << 32;`.
pub(super) const NOUVEAU_GETPARAM_GRAPH_UNITS: u64 = 13;
/// Max pushbuffers per EXEC ioctl (new submission uAPI). This driver caps EXEC
/// at 64 pushes, so it answers exactly that.
pub(super) const NOUVEAU_GETPARAM_EXEC_PUSH_MAX: u64 = 17;
pub(super) const NOUVEAU_GETPARAM_HAS_BO_USAGE: u64 = 15;
pub(super) const NOUVEAU_GETPARAM_HAS_PAGEFLIP: u64 = 16;
pub(super) const NOUVEAU_GETPARAM_VRAM_BAR_SIZE: u64 = 18;
pub(super) const NOUVEAU_GETPARAM_VRAM_USED: u64 = 19;
pub(super) const NOUVEAU_GETPARAM_HAS_VMA_TILEMODE: u64 = 20;

// --- NOUVEAU_GEM_DOMAIN_* flags (`nouveau_drm.h`) ---
pub(super) const NOUVEAU_GEM_DOMAIN_VRAM: u32 = 1 << 1;
/// Host system memory reachable by the GPU. NVK's
/// `nvkmd_nouveau_alloc_tiled_mem` picks exactly ONE domain per allocation
/// (`if GART ... else if VRAM ...`), so a GART request carries no VRAM bit --
/// GEM_NEW has to honour it on its own or every host-visible Vulkan
/// allocation fails.
pub(super) const NOUVEAU_GEM_DOMAIN_GART: u32 = 1 << 2;

// --- DRM_NOUVEAU_VM_BIND_OP_* ---
pub(super) const VM_BIND_OP_MAP: u32 = 0x0;
pub(super) const VM_BIND_OP_UNMAP: u32 = 0x1;
/// `DRM_NOUVEAU_VM_BIND_SPARSE` -- the op describes a SPARSE region: a VA
/// range with no GEM object behind it (`handle` is 0), whose pages read as
/// zero instead of faulting. NVK asks for these only for sparse Vulkan
/// resources (`nvk_image.c`/`nvk_buffer.c`), never during device creation.
pub(super) const VM_BIND_SPARSE: u32 = 1 << 8;
/// In a `VM_BIND` op, the low byte of `flags` is the PTE kind mesa wants the
/// mapping programmed with (`nouveau_ws_bo_bind` passes `pte_kind` straight
/// through as `flags`). 0 means plain linear.
pub(super) const VM_BIND_PTE_KIND_MASK: u32 = 0xff;

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

// --- Legacy GEM_PUSHBUF submission ABI (nouveau_drm.h) --------------------------
//
// This is the path the classic **nvc0 Gallium** driver (Mesa's OpenGL for
// Turing) uses — NOT the new VM_BIND/EXEC uAPI. Field-for-field identical to
// `nouveau_drm.h` so the ioctl request number (which bakes in the struct size)
// matches what Mesa's libdrm issues. Only parsed+logged for now (see the
// dispatch arm in `nvidia.rs`): real submission needs GART-domain GEM, the 3D
// class bound to the channel, and relocation handling — hardware-validated
// follow-up, never faked.

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub(super) struct DrmNouveauGemPushbufBoPresumed {
    pub valid: u32,
    pub domain: u32,
    pub offset: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub(super) struct DrmNouveauGemPushbufBo {
    pub user_priv: u64,
    pub handle: u32,
    pub read_domains: u32,
    pub write_domains: u32,
    pub valid_domains: u32,
    pub presumed: DrmNouveauGemPushbufBoPresumed,
}

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub(super) struct DrmNouveauGemPushbufReloc {
    pub reloc_bo_index: u32,
    pub reloc_bo_offset: u32,
    pub bo_index: u32,
    pub flags: u32,
    pub data: u32,
    pub vor: u32,
    pub tor: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub(super) struct DrmNouveauGemPushbufPush {
    pub bo_index: u32,
    pub pad: u32,
    pub offset: u64,
    pub length: u64,
}

#[repr(C)]
#[allow(dead_code)]
pub(super) struct DrmNouveauGemPushbuf {
    pub channel: u32,
    pub nr_buffers: u32,
    pub buffers: u64,
    pub nr_relocs: u32,
    pub nr_push: u32,
    pub relocs: u64,
    pub push: u64,
    pub suffix0: u32,
    pub suffix1: u32,
    pub vram_available: u64,
    pub gart_available: u64,
}

// --- Driver-side bookkeeping (Eclipse-internal, not part of the UAPI wire format) ---

/// The single channel this milestone supports, cached after `CHANNEL_ALLOC`
/// reuses the existing step16+step17 bring-up ladder. `h_vas` and
/// `notifier_handle` are real RM object handles from that ladder; nothing
/// here is invented.
pub(super) struct NouveauChannelState {
    /// Channel id handed back in `drm_nouveau_channel_alloc.channel`, and the
    /// value mesa echoes as the NVIF `token` when it enumerates classes or
    /// allocates subchannels on this channel. Signed to match the uAPI's
    /// `__s32 channel`.
    pub id: i32,
    pub h_vas: u32,
    pub notifier_handle: u32,
    /// Whether this channel is backed by a real RM GR channel (the
    /// `step16`+`step17` ladder) or is a *discovery* channel: NVK allocates a
    /// throwaway channel during `vkEnumeratePhysicalDevices` purely to ask
    /// `NVIF SCLASS` which engine classes exist, then frees it without ever
    /// submitting. Serving that from software lets the GPU enumerate on a
    /// kernel whose RM is not attached yet, while every path that genuinely
    /// needs hardware (GEM_NEW/VM_BIND/EXEC) still refuses with ENODEV.
    pub rm_backed: bool,
    /// Which per-process GPU context (independent VAS + GPFIFO channel) this
    /// channel belongs to. 0 = the compositor's singleton context (the
    /// `step16`+`step17` ladder), and also the value for discovery channels.
    /// >= 1 = a GL client's OWN context built by `nvidia_rm_sys::rm_init::
    /// ctx_alloc`, so a client's `VM_BIND`/`EXEC` route to its own VAS/channel
    /// instead of trampling the compositor's. `VM_BIND`/`EXEC` recover it from
    /// the calling pid via `ctx_idx_for_pid`.
    pub ctx_idx: u32,
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
/// object (`nvidia_rm_sys::rm_init::gem_alloc`, `NV01_MEMORY_LOCAL_USER`
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
    /// pid that allocated this object (`GEM_NEW`). Process-exit teardown frees
    /// ONLY the exiting process's objects -- freeing every object on any exit
    /// (the old single-process assumption) would pull the compositor's buffers
    /// out from under it the moment a GL client closed.
    pub owner_pid: u64,
    pub size: u64,
    /// BAR1-relative CPU physical address (`gem_map_cpu`'s `AT_CPU`
    /// offset), if resolving one succeeded at `GEM_NEW` time. `None` means
    /// this object is real (VM_BIND/EXEC both still work) but not
    /// CPU-mmap-able -- see the `GEM_NEW` gap note in
    /// docs/README-nouveau-uapi.md.
    pub phys_addr: Option<u64>,
    /// Tiling the client asked for at `GEM_NEW`, kept so `GEM_INFO` returns
    /// what was requested. `tile_flags`'s upper bits carry the PTE kind
    /// (`pte_kind << 8`); 0/0 is plain linear.
    pub tile_mode: u32,
    pub tile_flags: u32,
}

/// A GPU-VA mapping created by `VM_BIND` (`MAP` op), tracked so `UNMAP` can
/// find the RM virtual-range handle to tear down.
pub(super) struct NouveauVmMapping {
    /// Which `NouveauGemObject::handle` this binds.
    pub gem_handle: u32,
    /// RM's virtual-range object handle (`vm_bind_map`'s `h_virt`) -- needed
    /// to `Unmap`+`Free` it later.
    pub h_virt: u32,
    /// pid that created this mapping. With per-process GPU contexts each client
    /// has its OWN VA space, so a bind is meaningful ONLY within its owner's
    /// context: process-exit teardown and MAP/UNMAP overlap-replace must scope
    /// to `owner_pid`, or a client's exit (or a same-VA bind in a DIFFERENT
    /// context) would tear down the compositor's or another client's mapping.
    pub owner_pid: u64,
    pub va: u64,
    pub size: u64,
    /// Offset into the GEM object this mapping starts at (`bo_offset` from
    /// the VM_BIND op) -- needed to translate a GPU VA inside this mapping
    /// back to a CPU-readable physical address (gem phys + bo_offset + delta).
    pub bo_offset: u64,
}


// ===================== NVIF (`DRM_NOUVEAU_NVIF`, nr 0x47) =====================
//
// NVIF is nouveau's generic object-model ioctl, and mesa's NVK winsys leans on
// it during *physical device enumeration* -- long before any rendering. It is
// therefore mandatory: `nouveau_ws_device_new()` fails, and NVK reports zero
// Vulkan GPUs, if any of these calls fails.
//
// Every NVIF call shares nr 0x47 but carries a DIFFERENT payload size and
// direction (72B W, 136B WR, 160B WR, 56B W, 24B W), so it can only be
// dispatched by NR -- never by full ioctl request number. See `nouveau_ioctl`.
//
// Layouts below are transcribed byte-for-byte from mesa's
// `src/nouveau/drm/nvif/{ioctl,cl0080}.h` (= Linux's `include/nvif/`).

/// `nvif_ioctl_v0.type` values (nvif/ioctl.h).
pub(super) const NVIF_IOCTL_V0_SCLASS: u8 = 0x01;
pub(super) const NVIF_IOCTL_V0_NEW: u8 = 0x02;
pub(super) const NVIF_IOCTL_V0_DEL: u8 = 0x03;
pub(super) const NVIF_IOCTL_V0_MTHD: u8 = 0x04;

/// `NV_DEVICE` class handle (nvif/class.h: `#define NV_DEVICE 0x00000080`).
pub(super) const NVIF_CLASS_NV_DEVICE: i32 = 0x0000_0080;
/// `NV_DEVICE_V0_INFO` method (nvif/cl0080.h).
pub(super) const NV_DEVICE_V0_INFO: u8 = 0x00;
/// `nv_device_info_v0.platform` = PCIE. Mesa maps PCI/AGP/PCIE/default to
/// `NV_DEVICE_TYPE_DIS` (discrete), which its conformance gate requires.
pub(super) const NV_DEVICE_INFO_V0_PCIE: u8 = 0x03;

/// Common 24-byte NVIF header (`struct nvif_ioctl_v0`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct NvifIoctlV0 {
    pub version: u8,
    pub type_: u8,
    pub pad02: [u8; 4],
    pub owner: u8,
    pub route: u8,
    pub token: u64,
    pub object: u64,
    // followed by a per-type body
}

/// `struct nvif_ioctl_new_v0` (32 bytes), body of a `NEW`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct NvifIoctlNewV0 {
    pub version: u8,
    pub pad01: [u8; 6],
    pub route: u8,
    pub token: u64,
    pub object: u64,
    pub handle: u32,
    pub oclass: i32,
    // followed by class data (e.g. NvDeviceV0)
}

/// `struct nvif_ioctl_mthd_v0` (8 bytes), body of an `MTHD`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct NvifIoctlMthdV0 {
    pub version: u8,
    pub method: u8,
    pub pad02: [u8; 6],
    // followed by method data (e.g. NvDeviceInfoV0)
}

/// `struct nvif_ioctl_sclass_v0` (8 bytes), body of an `SCLASS`, followed by
/// `count` entries of `NvifSclassOclassV0`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct NvifIoctlSclassV0 {
    pub version: u8,
    /// IN: how many entries the caller has room for (mesa passes 16).
    /// OUT: how many we filled.
    pub count: u8,
    pub pad02: [u8; 6],
}

/// `struct nvif_ioctl_sclass_oclass_v0` (8 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct NvifSclassOclassV0 {
    pub oclass: i32,
    pub minver: i16,
    pub maxver: i16,
}

/// `struct nv_device_v0` (16 bytes), class data for `NEW` of `NV_DEVICE`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct NvDeviceV0 {
    pub version: u8,
    pub pad01: [u8; 7],
    /// Device selector; mesa passes `~0` ("client default").
    pub device: u64,
}

/// `struct nv_device_info_v0` (104 bytes) -- the reply mesa reads for
/// chipset/VRAM/type. Mesa consumes `chipset`, `ram_user` (-> vram_size_B),
/// `platform` (-> device type) and copies `chip`/`name` as display strings.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct NvDeviceInfoV0 {
    pub version: u8,
    pub platform: u8,
    /// From NV_PMC_BOOT_0 -- the value mesa maps to an SM version.
    pub chipset: u16,
    pub revision: u8,
    pub family: u8,
    pub pad06: [u8; 2],
    pub ram_size: u64,
    pub ram_user: u64,
    pub chip: [u8; 16],
    pub name: [u8; 64],
}

// --- Compile-time ABI checks ------------------------------------------------
//
// These layouts were transcribed from mesa's `nvif/{ioctl,cl0080}.h` by hand
// and cannot be validated on hardware from here, so pin them at compile time:
// a wrong offset becomes a build failure instead of a GPU that silently
// refuses to enumerate. The totals are the exact payload sizes mesa sends
// (`sizeof` of its anonymous request structs).
const _: () = {
    use core::mem::size_of as sz;
    assert!(sz::<NvifIoctlV0>() == 24);
    assert!(sz::<NvifIoctlNewV0>() == 32);
    assert!(sz::<NvifIoctlMthdV0>() == 8);
    assert!(sz::<NvifIoctlSclassV0>() == 8);
    assert!(sz::<NvifSclassOclassV0>() == 8);
    assert!(sz::<NvDeviceV0>() == 16);
    assert!(sz::<NvDeviceInfoV0>() == 104);
    // nouveau_ws_device_alloc: ioctl + new + nv_device_v0
    assert!(sz::<NvifIoctlV0>() + sz::<NvifIoctlNewV0>() + sz::<NvDeviceV0>() == 72);
    // nouveau_ws_device_info: ioctl + mthd + nv_device_info_v0
    assert!(sz::<NvifIoctlV0>() + sz::<NvifIoctlMthdV0>() + sz::<NvDeviceInfoV0>() == 136);
    // nouveau_ws_context_query_classes: ioctl + sclass + 16 oclass slots
    assert!(
        sz::<NvifIoctlV0>() + sz::<NvifIoctlSclassV0>() + 16 * sz::<NvifSclassOclassV0>() == 160
    );
    // nouveau_ws_subchan_alloc: ioctl + new
    assert!(sz::<NvifIoctlV0>() + sz::<NvifIoctlNewV0>() == 56);
};

/// Ceiling on live nouveau channels per GPU. Only one can ever be RM-backed
/// (the `step16`+`step17` ladder builds a single GR channel); the rest are
/// discovery channels, which exist purely so a second client can enumerate.
pub(super) const MAX_CHANNELS: usize = 16;

/// Number of per-process GPU contexts (index 0 = the compositor's singleton;
/// 1.. = one per GL client). MUST match `ECLIPSE_MAX_CTX` in
/// `vendor/eclipse_rm_init.c`.
pub(super) const MAX_CTX: u32 = 8;

/// How many class slots mesa offers in an `SCLASS` call
/// (`NOUVEAU_WS_CONTEXT_MAX_CLASSES`).
pub(super) const NVIF_SCLASS_MAX: usize = 16;

// --- Engine classes advertised through SCLASS -------------------------------
//
// Mesa picks, per engine type, the HIGHEST advertised class whose LOW BYTE
// matches: 0xb5 copy, 0x2d 2d, 0x97 3d, 0x40 (else 0x39) m2mf, 0xc0 compute.
// A type with no match yields oclass 0 and mesa fails the device with EINVAL,
// so all five types must be present. Values from mesa's NVIDIA class headers.
/// `FERMI_TWOD_A` -- still the 2d class on every Turing+ chip.
pub(super) const CLASS_FERMI_TWOD_A: i32 = 0x902d;
/// `KEPLER_INLINE_TO_MEMORY_B` -- the m2mf (0x40) class on Turing+.
pub(super) const CLASS_KEPLER_INLINE_TO_MEMORY_B: i32 = 0xa140;

/// Per-architecture (3d, compute, dma-copy) class triple.
///
/// NVK's conformance gate additionally requires the 3d class to be in
/// `[KEPLER_A 0xa097 ..= ADA_A 0xc997]` or exactly `BLACKWELL_B 0xce97`;
/// anything else makes a release-built NVK skip the GPU silently.
pub(super) const CLASSES_TURING: (i32, i32, i32) = (0xc597, 0xc5c0, 0xc5b5);
pub(super) const CLASSES_AMPERE: (i32, i32, i32) = (0xc797, 0xc7c0, 0xc7b5);
/// Ada has NO copy class of its own -- RM's CE dispatch jumps straight from
/// `AMPERE_DMA_COPY_B` to `HOPPER_DMA_COPY_A`, so AD10x uses the Ampere-B one.
/// (0xc9b5 is `BLACKWELL_DMA_COPY_A`; advertising it here would make mesa emit
/// Blackwell CE methods on an Ada part and make our own GSP-RM reject the
/// NvRmAlloc.) Verified against this repo's vendored
/// `open-gpu-kernel-modules/.../g_allclasses.h`.
pub(super) const CLASSES_ADA: (i32, i32, i32) = (0xc997, 0xc9c0, 0xc7b5);
/// Hopper/Blackwell: UNVERIFIED against a real chip by this milestone. Note
/// `HOPPER_A` (0xcb97) and `BLACKWELL_A` (0xcd97) are NOT in NVK's conformant
/// range, so a release-built NVK skips such a GPU regardless of what we say;
/// `BLACKWELL_B` (0xce97) is the consumer GB20x class and IS accepted.
pub(super) const CLASSES_HOPPER: (i32, i32, i32) = (0xcb97, 0xcbc0, 0xc8b5);
/// Blackwell comes in two generations: `BLACKWELL_A`/`_COMPUTE_A`/`_DMA_COPY_A`
/// (0xcd97/0xcdc0/0xc9b5, GB100 datacenter) and `BLACKWELL_B`/`_COMPUTE_B`/
/// `_DMA_COPY_B` (0xce97/0xcec0/0xcab5, GB20x / RTX 50). We report the B set:
/// it matches the consumer parts this driver targets, and `BLACKWELL_B` is the
/// only Blackwell 3D class NVK accepts as conformant.
pub(super) const CLASSES_BLACKWELL: (i32, i32, i32) = (0xce97, 0xcec0, 0xcab5);

/// Minimum payload a caller must supply for a given driver-private NR.
///
/// The dispatch in `nvidia.rs` keys on NR alone (like Linux), which is what
/// lets it accept mesa's `_IOW`-encoded VM_INIT and NVIF's five different
/// shapes. The flip side is that the caller's declared size is no longer
/// implied by the request number, so it must be checked explicitly before any
/// arm casts the argument to a fixed struct and writes into it.
///
/// `None` means "no fixed minimum" -- only NVIF, which validates its own
/// header and per-type bodies internally because one NR carries five layouts.
pub(super) fn min_payload_for_nr(nr: u32) -> Option<usize> {
    use core::mem::size_of;
    Some(match nr {
        NR_GETPARAM => size_of::<DrmNouveauGetparam>(),
        NR_CHANNEL_ALLOC => size_of::<DrmNouveauChannelAlloc>(),
        NR_CHANNEL_FREE => size_of::<DrmNouveauChannelFree>(),
        NR_VM_INIT => size_of::<DrmNouveauVmInit>(),
        NR_VM_BIND => size_of::<DrmNouveauVmBind>(),
        NR_EXEC => size_of::<DrmNouveauExec>(),
        NR_GEM_NEW => size_of::<DrmNouveauGemNew>(),
        NR_GEM_PUSHBUF => size_of::<DrmNouveauGemPushbuf>(),
        NR_GEM_CPU_PREP => size_of::<DrmNouveauGemCpuPrep>(),
        NR_GEM_CPU_FINI => size_of::<DrmNouveauGemCpuFini>(),
        NR_GEM_INFO => size_of::<DrmNouveauGemInfo>(),
        _ => return None,
    })
}

// --- Driver-private ioctl NRs (dispatch keys) -------------------------------
//
// Linux dispatches driver-private ioctls by NR alone:
//   nouveau_drm.c: `switch (_IOC_NR(cmd) - DRM_COMMAND_BASE)`
// The caller's direction and size bits are advisory. Matching the FULL request
// number instead makes an ioctl unreachable whenever userspace encodes it
// differently -- which is exactly what happened with VM_INIT (mesa issues it
// via drmCommandWrite = _IOW, we only accepted _IOWR).
pub(super) const NR_GETPARAM: u32 = DRM_COMMAND_BASE + DRM_NOUVEAU_GETPARAM;
pub(super) const NR_CHANNEL_ALLOC: u32 = DRM_COMMAND_BASE + DRM_NOUVEAU_CHANNEL_ALLOC;
pub(super) const NR_CHANNEL_FREE: u32 = DRM_COMMAND_BASE + DRM_NOUVEAU_CHANNEL_FREE;
pub(super) const NR_NVIF: u32 = DRM_COMMAND_BASE + DRM_NOUVEAU_NVIF;
pub(super) const NR_VM_INIT: u32 = DRM_COMMAND_BASE + DRM_NOUVEAU_VM_INIT;
pub(super) const NR_VM_BIND: u32 = DRM_COMMAND_BASE + DRM_NOUVEAU_VM_BIND;
pub(super) const NR_EXEC: u32 = DRM_COMMAND_BASE + DRM_NOUVEAU_EXEC;
pub(super) const NR_GET_ZCULL_INFO: u32 = DRM_COMMAND_BASE + DRM_NOUVEAU_GET_ZCULL_INFO;
pub(super) const NR_GEM_NEW: u32 = DRM_COMMAND_BASE + DRM_NOUVEAU_GEM_NEW;
pub(super) const NR_GEM_PUSHBUF: u32 = DRM_COMMAND_BASE + DRM_NOUVEAU_GEM_PUSHBUF;
pub(super) const NR_GEM_CPU_PREP: u32 = DRM_COMMAND_BASE + DRM_NOUVEAU_GEM_CPU_PREP;
pub(super) const NR_GEM_CPU_FINI: u32 = DRM_COMMAND_BASE + DRM_NOUVEAU_GEM_CPU_FINI;
pub(super) const NR_GEM_INFO: u32 = DRM_COMMAND_BASE + DRM_NOUVEAU_GEM_INFO;
