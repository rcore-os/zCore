//! Safe Rust entry points for `vendor/eclipse_rm_init.c` -- Eclipse's own
//! equivalent of what NVIDIA's real Linux platform layer does in
//! arch/nvalloc/unix/src/osinit.c (osRmInitRm / osInitNvMapping /
//! RmInitAdapter) to bring up the real, portable RM core: construct the
//! OBJSYS singleton and resource server, then attach a GPU by real PCI
//! location and BAR info. See that file's own header comment for the
//! full real-vs-ours breakdown and the one known gap (REGISTER_ALL_HALS).
use crate::types::*;

// ---------------------------------------------------------------------
// RM call gate: serialize EVERY entry into the RM from ioctl-time paths.
//
// Why: the RM's own API lock (`rmapiLockAcquire`) is real, but its
// CONTENDED path -- priority queues, condition waits, os-layer sleep
// primitives -- has never executed in this port. It could not: while
// `os_get_current_thread` returned 0 for every thread, contention was
// refused up front (`NV_ERR_INVALID_LOCK_STATE`) instead of blocking, so
// the wait machinery was dead code. The moment real thread ids landed,
// the first concurrent ioctl pair (a compositor submitting on one thread
// while allocating its swapchain on another) drove the RM into that
// untested blocking path on real hardware and the machine froze at boot
// with a dead console.
//
// This gate keeps the RM effectively single-threaded, which is the state
// every line of RM code in this port has ever been validated in: one
// caller inside the RM at a time, everyone else spins HERE -- in OUR
// code, preemptible, with IRQs on -- rather than in the RM's wait path.
// `rmapiLockAcquire` then always finds the lock free and its blocking
// path stays unexercised.
//
// NOT `lock::Mutex`: that is an IRQ-disabling spinlock, and RM calls can
// legitimately take hundreds of milliseconds (`exec_submit_signaled`
// polls its fence for up to 500 ms) -- holding an IRQ-off lock that long
// stalls timers and input and trips the deadlock watchdog. A plain
// atomic + `spin_loop` burns at most one preemptible timeslice per
// waiting thread, which the scheduler already handles.
// ---------------------------------------------------------------------
static RM_CALL_GATE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

struct RmGate;

impl RmGate {
    fn lock() -> RmGate {
        use core::sync::atomic::Ordering;
        while RM_CALL_GATE.swap(true, Ordering::Acquire) {
            core::hint::spin_loop();
        }
        RmGate
    }
}

impl Drop for RmGate {
    fn drop(&mut self) {
        RM_CALL_GATE.store(false, core::sync::atomic::Ordering::Release);
    }
}

extern "C" {
    fn eclipse_rm_init_core() -> NV_STATUS;

    fn eclipse_rm_attach_gpu(
        domain: NvU32,
        bus: NvU8,
        device: NvU8,
        bar0_phys: NvU64,
        bar0_virt: *mut c_void,
        bar0_len: NvU64,
        bar1_phys: NvU64,
        bar1_len: NvU64,
        bar2_phys: NvU64,
        bar2_len: NvU64,
        out_device_instance: *mut NvU32,
    ) -> NV_STATUS;

    fn eclipse_rm_init_gsp(device_instance: NvU32, buf: *const c_void, size: NvU32) -> NV_STATUS;
}

/// Constructs the real OBJSYS singleton and the RM resource server.
/// Call exactly once, before the first `attach_gpu`.
pub fn init_core() -> NV_STATUS {
    unsafe { eclipse_rm_init_core() }
}

/// Attaches a GPU to RM by its real PCI location and BAR0/BAR1/BAR2
/// physical/virtual addresses, mirroring what NVIDIA's own
/// `osInitNvMapping` packages into a `GPUATTACHARG`. `bar0_virt` must
/// already be mapped (Eclipse maps BAR0 during PCI probe); BAR1 (FB) and
/// BAR2 (IMEM) are passed as physical addresses only, same as the real
/// driver (`fbBaseAddr`/`instBaseAddr = NULL // not mapped`). BAR2 becomes
/// `GPUATTACHARG.instPhysAddr`/`instLength`, required by the BAR2 MMU
/// self-test in `gpuStateInit` (osinit.c:708).
///
/// Returns the real RM device instance on success.
#[allow(clippy::too_many_arguments)]
pub fn attach_gpu(
    domain: u32,
    bus: u8,
    device: u8,
    bar0_phys: u64,
    bar0_virt: *mut c_void,
    bar0_len: u64,
    bar1_phys: u64,
    bar1_len: u64,
    bar2_phys: u64,
    bar2_len: u64,
) -> Result<u32, NV_STATUS> {
    let mut device_instance: NvU32 = 0;
    let status = unsafe {
        eclipse_rm_attach_gpu(
            domain,
            bus,
            device,
            bar0_phys,
            bar0_virt,
            bar0_len,
            bar1_phys,
            bar1_len,
            bar2_phys,
            bar2_len,
            &mut device_instance,
        )
    };
    if status == NV_OK {
        Ok(device_instance)
    } else {
        Err(status)
    }
}

/// Boots GSP-RM on an already-attached GPU via the real, vendored
/// `kgspInitRm`, given the raw bytes of NVIDIA's `gsp.bin` (the one
/// firmware blob genuinely external to the open-sourced RM core --
/// everything else `kgspInitRm` needs, it self-derives from `buf` or
/// from bindata already compiled into this crate). `device_instance` is
/// the value returned by a prior successful `attach_gpu`.
pub fn init_gsp(device_instance: u32, buf: &[u8]) -> Result<(), NV_STATUS> {
    let status = unsafe {
        eclipse_rm_init_gsp(
            device_instance,
            buf.as_ptr() as *const c_void,
            buf.len() as NvU32,
        )
    };
    if status == NV_OK {
        Ok(())
    } else {
        Err(status)
    }
}

/// Fixed-layout mirror of `EclipseGspInfo` (vendor/eclipse_rm_init.c): the
/// subset of `GspStaticConfigInfo` that the live GSP-RM returned during
/// `kgspInitRm`'s GET_GSP_STATIC_INFO RPC. All-zero fields mean the RPC has
/// not run yet (gpustep6 not completed on this GPU).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GspInfo {
    pub gpu_name: [u8; 64],
    pub gpu_short_name: [u8; 64],
    pub fb_length: NvU64,
    pub fb_bus_width: NvU32,
    pub fb_ram_type: NvU32,
    pub l2_cache_size: NvU32,
    pub vbios_valid: u8,
    pub vbios_sub_vendor: NvU32,
    pub vbios_sub_device: NvU32,
}

extern "C" {
    fn eclipse_rm_get_gsp_info(device_instance: NvU32, info: *mut GspInfo) -> NV_STATUS;
}

/// Reads back the firmware-provided static config for an attached GPU.
pub fn get_gsp_info(device_instance: u32) -> Result<GspInfo, NV_STATUS> {
    let mut info = GspInfo {
        gpu_name: [0; 64],
        gpu_short_name: [0; 64],
        fb_length: 0,
        fb_bus_width: 0,
        fb_ram_type: 0,
        l2_cache_size: 0,
        vbios_valid: 0,
        vbios_sub_vendor: 0,
        vbios_sub_device: 0,
    };
    let status = unsafe { eclipse_rm_get_gsp_info(device_instance, &mut info) };
    if status == NV_OK {
        Ok(info)
    } else {
        Err(status)
    }
}

/// Fixed-layout mirror of `EclipseRmApiDemo` (vendor/eclipse_rm_init.c):
/// results of three read-only RM API controls executed by the live GSP-RM's
/// own resource server via `rpcRmApiControl_GSP` (the GSP_RM_CONTROL RPC).
/// Each control carries its own NV_STATUS so partial success is visible.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RmApiDemo {
    pub name_status: NV_STATUS,
    pub name: [u8; 64],
    pub gid_status: NV_STATUS,
    pub gid_length: NvU32,
    pub gid: [u8; 136],
    pub fb_status: NV_STATUS,
    pub heap_size_kb: NvU32,
    pub heap_free_kb: NvU32,
    pub bus_width: NvU32,
}

extern "C" {
    fn eclipse_rm_step8(device_instance: NvU32, out: *mut RmApiDemo) -> NV_STATUS;
}

/// Runs the step-8 RM-API-control demo against the live GSP.
pub fn rm_api_demo(device_instance: u32) -> Result<RmApiDemo, NV_STATUS> {
    let mut out = RmApiDemo {
        name_status: 0,
        name: [0; 64],
        gid_status: 0,
        gid_length: 0,
        gid: [0; 136],
        fb_status: 0,
        heap_size_kb: 0,
        heap_free_kb: 0,
        bus_width: 0,
    };
    let status = unsafe { eclipse_rm_step8(device_instance, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Fixed-layout mirror of `EclipseGrProbe` (vendor/eclipse_rm_init.c): the
/// graphics/compute (GR) engine's shader config as reported by the live GSP-RM
/// via the GR_GET_GPC_MASK / GR_GET_TPC_MASK controls. Turing packs TWO SMs
/// per TPC (Volta+ layout), so the usable SM count is `2 * total_tpc`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GrProbe {
    pub gpc_mask_status: NV_STATUS,
    pub gpc_mask: NvU32,
    pub num_gpc: NvU32,
    pub tpc_mask_status: NV_STATUS,
    pub total_tpc: NvU32,
    pub per_gpc_tpc: [NvU32; 8],
}

extern "C" {
    fn eclipse_rm_step15(device_instance: NvU32, out: *mut GrProbe) -> NV_STATUS;
}

/// Probes the GR (graphics/compute) engine's GPC/TPC/SM config on a
/// state-loaded GPU, over the live GSP resource server.
pub fn step15(device_instance: u32) -> Result<GrProbe, NV_STATUS> {
    let _gate = RmGate::lock();
    let mut out = GrProbe {
        gpc_mask_status: 0,
        gpc_mask: 0,
        num_gpc: 0,
        tpc_mask_status: 0,
        total_tpc: 0,
        per_gpc_tpc: [0; 8],
    };
    let status = unsafe { eclipse_rm_step15(device_instance, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// One row of the GSP-reported interrupt kernel table (mirror of
/// `EclipseIntrTableEntry`): which engine (MC_ENGINE_IDX_*) owns which
/// stall/nonstall vector in the Turing+ CPU_INTR tree, plus its legacy PMC
/// mask. `0xFFFFFFFF` vectors mean not-applicable.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IntrTableEntry {
    pub engine_idx: NvU32,
    pub pmc_intr_mask: NvU32,
    pub vector_stall: NvU32,
    pub vector_non_stall: NvU32,
}

/// Mirror of `EclipseIntrTable`: the live GSP-RM's authoritative
/// vector->engine interrupt map (NV2080_CTRL_CMD_INTERNAL_INTR_GET_KERNEL_TABLE,
/// the same control kernel RM uses to build its own interrupt table).
#[repr(C)]
pub struct IntrTable {
    pub ctrl_status: NV_STATUS,
    pub table_len: NvU32,
    pub entries: [IntrTableEntry; 128],
}

extern "C" {
    fn eclipse_rm_intr_table(device_instance: NvU32, out: *mut IntrTable) -> NV_STATUS;
}

/// Mirror of `EclipseGrAlloc` (vendor/eclipse_rm_init.c): per-stage NV_STATUS
/// (`0xFFFFFFFF` = not reached) and the allocated handles of the GR
/// allocation ladder: client -> device -> subdevice -> VA space -> TSG
/// (GRAPHICS engine) -> context share. Handles stay alive C-side for step17.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GrAlloc {
    pub client_status: NvU32,
    pub device_status: NvU32,
    pub subdev_status: NvU32,
    pub vas_status: NvU32,
    pub tsg_status: NvU32,
    pub ctxshare_status: NvU32,
    pub h_client: NvU32,
    pub h_device: NvU32,
    pub h_subdevice: NvU32,
    pub h_vas: NvU32,
    pub h_tsg: NvU32,
    pub h_ctxshare: NvU32,
}

extern "C" {
    fn eclipse_rm_step16(device_instance: NvU32, out: *mut GrAlloc) -> NV_STATUS;
}

/// Runs the step-16 GR allocation ladder on a state-loaded GPU (idempotent:
/// repeat calls return the cached, still-alive allocation).
pub fn step16(device_instance: u32) -> Result<GrAlloc, NV_STATUS> {
    let _gate = RmGate::lock();
    let mut out = GrAlloc {
        client_status: 0xFFFF_FFFF,
        device_status: 0xFFFF_FFFF,
        subdev_status: 0xFFFF_FFFF,
        vas_status: 0xFFFF_FFFF,
        tsg_status: 0xFFFF_FFFF,
        ctxshare_status: 0xFFFF_FFFF,
        h_client: 0,
        h_device: 0,
        h_subdevice: 0,
        h_vas: 0,
        h_tsg: 0,
        h_ctxshare: 0,
    };
    let status = unsafe { eclipse_rm_step16(device_instance, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Mirror of `EclipseGrChannel` (vendor/eclipse_rm_init.c): per-stage
/// NV_STATUS (`0xFFFFFFFF` = not reached) and handles for the step-17
/// compute-channel bring-up on the step-16 ladder.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GrChannel {
    pub userd_status: NvU32,
    pub buf_status: NvU32,
    pub virt_status: NvU32,
    pub map_status: NvU32,
    pub notif_status: NvU32,
    pub chan_status: NvU32,
    pub compute_status: NvU32,
    pub sched_status: NvU32,
    pub h_userd: NvU32,
    pub h_phys_buf: NvU32,
    pub h_virt_buf: NvU32,
    pub h_notifier: NvU32,
    pub h_channel: NvU32,
    pub h_compute: NvU32,
    pub channel_class: NvU32,
    pub userd_size: NvU32,
    pub buf_gpu_va: u64,
}

extern "C" {
    fn eclipse_rm_step17(device_instance: NvU32, out: *mut GrChannel) -> NV_STATUS;
}

/// Runs step-17 (USERD + buffers + GPFIFO channel + TURING_COMPUTE_A +
/// schedule) on the cached step-16 ladder. Idempotent.
pub fn step17(device_instance: u32) -> Result<GrChannel, NV_STATUS> {
    let _gate = RmGate::lock();
    let mut out = GrChannel {
        userd_status: 0xFFFF_FFFF,
        buf_status: 0xFFFF_FFFF,
        virt_status: 0xFFFF_FFFF,
        map_status: 0xFFFF_FFFF,
        notif_status: 0xFFFF_FFFF,
        chan_status: 0xFFFF_FFFF,
        compute_status: 0xFFFF_FFFF,
        sched_status: 0xFFFF_FFFF,
        h_userd: 0,
        h_phys_buf: 0,
        h_virt_buf: 0,
        h_notifier: 0,
        h_channel: 0,
        h_compute: 0,
        channel_class: 0,
        userd_size: 0,
        buf_gpu_va: 0,
    };
    let status = unsafe { eclipse_rm_step17(device_instance, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Mirror of `EclipseCtxAlloc` (vendor/eclipse_rm_init.c): a full INDEPENDENT
/// per-process GR context -- its own VAS + TSG + context share + USERD +
/// channel buffer + GPFIFO channel + TURING_COMPUTE_A -- built on the shared
/// client/device/subdevice the compositor's step16 already created. Per-stage
/// NV_STATUS (`0xFFFFFFFF` = not reached) plus the handles and the channel
/// buffer's GPU VA. Field order/types MUST match the C struct byte-for-byte.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CtxAlloc {
    pub vas_status: NvU32,
    pub tsg_status: NvU32,
    pub ctxshare_status: NvU32,
    pub userd_status: NvU32,
    pub buf_status: NvU32,
    pub virt_status: NvU32,
    pub map_status: NvU32,
    pub notif_status: NvU32,
    pub chan_status: NvU32,
    pub compute_status: NvU32,
    pub sched_status: NvU32,
    pub h_vas: NvU32,
    pub h_tsg: NvU32,
    pub h_ctxshare: NvU32,
    pub h_userd: NvU32,
    pub h_phys_buf: NvU32,
    pub h_virt_buf: NvU32,
    pub h_notifier: NvU32,
    pub h_channel: NvU32,
    pub h_compute: NvU32,
    pub channel_class: NvU32,
    pub userd_size: NvU32,
    pub buf_gpu_va: u64,
}

extern "C" {
    fn eclipse_rm_ctx_alloc(gpuInstance: NvU32, ctxIdx: NvU32, out: *mut CtxAlloc) -> NV_STATUS;
}

/// Allocate an independent per-process GR context on the shared step-16 ladder.
/// `ctx_idx` is in `1..8` (index 0 is the compositor's singleton, served by
/// step16/step17 and never routed here). See the C for why a second client
/// cannot share the compositor's single channel/VA space. Idempotent per index.
/// Serialized through `RmGate` like every other RM entry.
pub fn ctx_alloc(device_instance: u32, ctx_idx: u32) -> Result<CtxAlloc, NV_STATUS> {
    let _gate = RmGate::lock();
    let mut out = CtxAlloc {
        vas_status: 0xFFFF_FFFF,
        tsg_status: 0xFFFF_FFFF,
        ctxshare_status: 0xFFFF_FFFF,
        userd_status: 0xFFFF_FFFF,
        buf_status: 0xFFFF_FFFF,
        virt_status: 0xFFFF_FFFF,
        map_status: 0xFFFF_FFFF,
        notif_status: 0xFFFF_FFFF,
        chan_status: 0xFFFF_FFFF,
        compute_status: 0xFFFF_FFFF,
        sched_status: 0xFFFF_FFFF,
        h_vas: 0,
        h_tsg: 0,
        h_ctxshare: 0,
        h_userd: 0,
        h_phys_buf: 0,
        h_virt_buf: 0,
        h_notifier: 0,
        h_channel: 0,
        h_compute: 0,
        channel_class: 0,
        userd_size: 0,
        buf_gpu_va: 0,
    };
    let status = unsafe { eclipse_rm_ctx_alloc(device_instance, ctx_idx, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

extern "C" {
    fn eclipse_rm_ctx_free(gpuInstance: NvU32, ctxIdx: NvU32) -> NV_STATUS;
}

/// Free a per-process GR context built by [`ctx_alloc`] (channel + compute +
/// USERD + notifier + virtual/physical buffer + context share + TSG + VA
/// space) and clear its cache slot, so the index can be rebuilt fresh. Called
/// on process exit. `ctx_idx` in `1..8` (index 0 is the compositor's singleton
/// and is never freed here). A no-op if the index was never allocated.
/// Serialized through `RmGate` like every other RM entry.
pub fn ctx_free(device_instance: u32, ctx_idx: u32) -> NV_STATUS {
    let _gate = RmGate::lock();
    unsafe { eclipse_rm_ctx_free(device_instance, ctx_idx) }
}

extern "C" {
    fn eclipse_rm_ctx_prime(gpuInstance: NvU32, ctxIdx: NvU32) -> NV_STATUS;
}

/// Prime a per-process GR context's golden context by running step-18's minimal
/// compute stream (host-sem RELEASE + `SET_OBJECT(TURING_COMPUTE_A)` + engine
/// report-sem RELEASE) on the context's own channel and CPU-polling the engine
/// semaphore. The compute engine loads a channel's golden context on its first
/// submission; for the compositor (ctx 0) step-18 does this at bring-up, but a
/// client context (ctx `1..8`) would otherwise hit that cold load on NVK's very
/// first push — which hangs the PBDMA before it reaches NVK's fence
/// (observed on RTX as `GPGet=1 GPPut=2`, no MMU fault, no GR exception).
/// Priming it here loads the golden context up front so NVK's first real
/// submission runs warm.
///
/// Returns `NV_OK` if the engine semaphore lands (golden context loaded), or
/// `NV_ERR_TIMEOUT` if the engine does not respond within the internal bound
/// (~500 ms). The caller keeps the context regardless: a prime timeout is
/// diagnostic, not fatal — worst case the client sees the same cold-load hang
/// it would have without priming. `ctx_idx` in `1..8`. Serialized through
/// `RmGate` like every other RM entry.
pub fn ctx_prime(device_instance: u32, ctx_idx: u32) -> NV_STATUS {
    let _gate = RmGate::lock();
    unsafe { eclipse_rm_ctx_prime(device_instance, ctx_idx) }
}

/// Mirror of `EclipseGrLaunch` (vendor/eclipse_rm_init.c): per-stage
/// NV_STATUS (`0xFFFFFFFF` = not reached) for step-18, the first
/// Eclipse-authored pushbuffer submission (host + compute-engine
/// semaphore releases through the live step-17 channel).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GrLaunch {
    pub lookup_status: NvU32,
    pub map_status: NvU32,
    pub token_status: NvU32,
    pub submit_status: NvU32,
    pub host_sem_status: NvU32,
    pub eng_sem_status: NvU32,
    pub work_token: NvU32,
    pub runlist_id: NvU32,
    pub host_sem_value: NvU32,
    pub eng_sem_value: NvU32,
    pub host_poll_iters: NvU32,
    pub eng_poll_iters: NvU32,
    pub push_dwords: NvU32,
}

extern "C" {
    fn eclipse_rm_step18(device_instance: NvU32, out: *mut GrLaunch) -> NV_STATUS;
}

/// Runs step-18: writes semaphore-release methods into the step-17
/// pushbuffer, submits via GP entry + GPPut + usermode doorbell, and
/// CPU-polls both landing zones. Idempotent once fully successful.
pub fn step18(device_instance: u32) -> Result<GrLaunch, NV_STATUS> {
    let mut out = GrLaunch {
        lookup_status: 0xFFFF_FFFF,
        map_status: 0xFFFF_FFFF,
        token_status: 0xFFFF_FFFF,
        submit_status: 0xFFFF_FFFF,
        host_sem_status: 0xFFFF_FFFF,
        eng_sem_status: 0xFFFF_FFFF,
        work_token: 0,
        runlist_id: 0,
        host_sem_value: 0,
        eng_sem_value: 0,
        host_poll_iters: 0,
        eng_poll_iters: 0,
        push_dwords: 0,
    };
    let status = unsafe { eclipse_rm_step18(device_instance, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Mirror of `EclipseGrCompute` (vendor/eclipse_rm_init.c): step-19, the
/// first real compute launch — a minimal SM75 kernel run via a QMD on the
/// live step-17/18 channel, verified by the QMD's RELEASE0 semaphore.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GrCompute {
    pub lookup_status: NvU32,
    pub map_status: NvU32,
    pub token_status: NvU32,
    pub submit_status: NvU32,
    pub fence_status: NvU32,
    pub sem_status: NvU32,
    pub work_token: NvU32,
    pub runlist_id: NvU32,
    pub fence_value: NvU32,
    pub fence_iters: NvU32,
    pub sem_value: NvU32,
    pub poll_iters: NvU32,
    pub push_dwords: NvU32,
    pub reserved_pad: NvU32,
    pub kernel_va: u64,
    pub qmd_va: u64,
}

extern "C" {
    fn eclipse_rm_step19(device_instance: NvU32, out: *mut GrCompute) -> NV_STATUS;
}

/// Runs step-19: builds a Turing (Volta V02_02) QMD pointing at a minimal
/// SM75 EXIT kernel, submits it via SEND_PCAS on the step-17/18 channel, and
/// CPU-polls the QMD RELEASE0 semaphore. Idempotent once the semaphore lands.
pub fn step19(device_instance: u32) -> Result<GrCompute, NV_STATUS> {
    let mut out = GrCompute {
        lookup_status: 0xFFFF_FFFF,
        map_status: 0xFFFF_FFFF,
        token_status: 0xFFFF_FFFF,
        submit_status: 0xFFFF_FFFF,
        fence_status: 0xFFFF_FFFF,
        sem_status: 0xFFFF_FFFF,
        work_token: 0,
        runlist_id: 0,
        fence_value: 0,
        fence_iters: 0,
        sem_value: 0,
        poll_iters: 0,
        push_dwords: 0,
        reserved_pad: 0,
        kernel_va: 0,
        qmd_va: 0,
    };
    let status = unsafe { eclipse_rm_step19(device_instance, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Mirror of `EclipseGrStore` (vendor/eclipse_rm_init.c): step-20, a kernel
/// that stores a chosen value to a chosen VA from the SM (immediates patched
/// into the SASS), triple-verified: post-PCAS fence, QMD RELEASE0, and a CPU
/// readback of the destination dword.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GrStore {
    pub lookup_status: NvU32,
    pub map_status: NvU32,
    pub token_status: NvU32,
    pub submit_status: NvU32,
    pub fence_status: NvU32,
    pub sem_status: NvU32,
    pub store_status: NvU32,
    pub work_token: NvU32,
    pub runlist_id: NvU32,
    pub fence_value: NvU32,
    pub fence_iters: NvU32,
    pub sem_value: NvU32,
    pub sem_iters: NvU32,
    pub store_value: NvU32,
    pub push_dwords: NvU32,
    pub reserved_pad: NvU32,
    pub kernel_va: u64,
    pub qmd_va: u64,
    pub dest_va: u64,
}

extern "C" {
    fn eclipse_rm_step20(device_instance: NvU32, out: *mut GrStore) -> NV_STATUS;
}

/// Runs step-20: MOV/MOV/MOV/STG.E.SYS/EXIT kernel with runtime-patched
/// immediates on the proven step-19 QMD harness. Idempotent once the
/// RELEASE0 semaphore AND the stored value verify.
pub fn step20(device_instance: u32) -> Result<GrStore, NV_STATUS> {
    let mut out = GrStore {
        lookup_status: 0xFFFF_FFFF,
        map_status: 0xFFFF_FFFF,
        token_status: 0xFFFF_FFFF,
        submit_status: 0xFFFF_FFFF,
        fence_status: 0xFFFF_FFFF,
        sem_status: 0xFFFF_FFFF,
        store_status: 0xFFFF_FFFF,
        work_token: 0,
        runlist_id: 0,
        fence_value: 0,
        fence_iters: 0,
        sem_value: 0,
        sem_iters: 0,
        store_value: 0,
        push_dwords: 0,
        reserved_pad: 0,
        kernel_va: 0,
        qmd_va: 0,
        dest_va: 0,
    };
    let status = unsafe { eclipse_rm_step20(device_instance, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Mirror of `EclipseGrThreads` (vendor/eclipse_rm_init.c): step-21, the
/// multi-thread kernel — 32 threads each compute out[tid] = tid*3+7 with
/// real scoreboarding (S2R + IMAD + IMAD.WIDE + STG), CPU-verified per slot.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GrThreads {
    pub lookup_status: NvU32,
    pub map_status: NvU32,
    pub token_status: NvU32,
    pub submit_status: NvU32,
    pub fence_status: NvU32,
    pub sem_status: NvU32,
    pub verify_status: NvU32,
    pub work_token: NvU32,
    pub runlist_id: NvU32,
    pub fence_iters: NvU32,
    pub sem_iters: NvU32,
    pub match_count: NvU32,
    pub first_bad_idx: NvU32,
    pub first_bad_val: NvU32,
    pub push_dwords: NvU32,
    pub reserved_pad: NvU32,
    pub kernel_va: u64,
    pub qmd_va: u64,
    pub out_va: u64,
    /// MMU-fault telemetry (filled by step23 only; 0xFFFF_FFFF ctrl status
    /// means "not queried"). Survives nv_printf capture truncation.
    pub fault_ctrl_status: NvU32,
    pub fault_addr_hi: NvU32,
    pub fault_addr_lo: NvU32,
    pub fault_type: NvU32,
}

extern "C" {
    fn eclipse_rm_step21(device_instance: NvU32, out: *mut GrThreads) -> NV_STATUS;
}

/// Runs step-21: 32-thread compute kernel with per-thread verification.
/// Idempotent once RELEASE0 lands and all 32 slots verify.
pub fn step21(device_instance: u32) -> Result<GrThreads, NV_STATUS> {
    let mut out = GrThreads {
        lookup_status: 0xFFFF_FFFF,
        map_status: 0xFFFF_FFFF,
        token_status: 0xFFFF_FFFF,
        submit_status: 0xFFFF_FFFF,
        fence_status: 0xFFFF_FFFF,
        sem_status: 0xFFFF_FFFF,
        verify_status: 0xFFFF_FFFF,
        work_token: 0,
        runlist_id: 0,
        fence_iters: 0,
        sem_iters: 0,
        match_count: 0,
        first_bad_idx: 0xFFFF_FFFF,
        first_bad_val: 0,
        push_dwords: 0,
        reserved_pad: 0,
        kernel_va: 0,
        qmd_va: 0,
        out_va: 0,
        fault_ctrl_status: 0xFFFF_FFFF,
        fault_addr_hi: 0,
        fault_addr_lo: 0,
        fault_type: 0,
    };
    let status = unsafe { eclipse_rm_step21(device_instance, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

extern "C" {
    fn eclipse_rm_step22(device_instance: NvU32, out: *mut GrThreads) -> NV_STATUS;
}

extern "C" {
    fn eclipse_rm_step23(device_instance: NvU32, out: *mut GrThreads) -> NV_STATUS;
}

/// Runs step-22: chip-scale grid — 68 CTAs x 32 threads (2176 threads over
/// all 34 SMs), out[gid] = gid*3+7, CPU-verified per slot. Same result
/// shape as step-21. Idempotent once RELEASE0 + all 2176 slots verify.
pub fn step22(device_instance: u32) -> Result<GrThreads, NV_STATUS> {
    let mut out = GrThreads {
        lookup_status: 0xFFFF_FFFF,
        map_status: 0xFFFF_FFFF,
        token_status: 0xFFFF_FFFF,
        submit_status: 0xFFFF_FFFF,
        fence_status: 0xFFFF_FFFF,
        sem_status: 0xFFFF_FFFF,
        verify_status: 0xFFFF_FFFF,
        work_token: 0,
        runlist_id: 0,
        fence_iters: 0,
        sem_iters: 0,
        match_count: 0,
        first_bad_idx: 0xFFFF_FFFF,
        first_bad_val: 0,
        push_dwords: 0,
        reserved_pad: 0,
        kernel_va: 0,
        qmd_va: 0,
        out_va: 0,
        fault_ctrl_status: 0xFFFF_FFFF,
        fault_addr_hi: 0,
        fault_addr_lo: 0,
        fault_type: 0,
    };
    let status = unsafe { eclipse_rm_step22(device_instance, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Runs step-23: integer SAXPY (y[i] = a*x[i] + y[i]) over 32 threads with
/// real global loads (LDG), CPU-verified per element. Reuses the GrThreads
/// result shape (matchCount over 32). Idempotent once release+verify pass.
pub fn step23(device_instance: u32) -> Result<GrThreads, NV_STATUS> {
    let mut out = GrThreads {
        lookup_status: 0xFFFF_FFFF,
        map_status: 0xFFFF_FFFF,
        token_status: 0xFFFF_FFFF,
        submit_status: 0xFFFF_FFFF,
        fence_status: 0xFFFF_FFFF,
        sem_status: 0xFFFF_FFFF,
        verify_status: 0xFFFF_FFFF,
        work_token: 0,
        runlist_id: 0,
        fence_iters: 0,
        sem_iters: 0,
        match_count: 0,
        first_bad_idx: 0xFFFF_FFFF,
        first_bad_val: 0,
        push_dwords: 0,
        reserved_pad: 0,
        kernel_va: 0,
        qmd_va: 0,
        out_va: 0,
        fault_ctrl_status: 0xFFFF_FFFF,
        fault_addr_hi: 0,
        fault_addr_lo: 0,
        fault_type: 0,
    };
    let status = unsafe { eclipse_rm_step23(device_instance, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Mirror of `EclipseGrBench` (vendor/eclipse_rm_init.c): the GIOPS
/// benchmark result. Field order/types MUST match the C struct exactly.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GrBench {
    pub lookup_status: NvU32,
    pub map_status: NvU32,
    pub token_status: NvU32,
    pub submit_status: NvU32,
    pub sem_status: NvU32,
    pub num_threads: NvU32,
    pub imads_per_thread: NvU32,
    pub push_dwords: NvU32,
    pub sem_iters: NvU32,
    pub reserved_pad: NvU32,
    pub t0_ns: u64,
    pub t1_ns: u64,
    pub elapsed_ns: u64,
    pub total_ops: u64,
    pub kernel_va: u64,
    pub qmd_va: u64,
}

extern "C" {
    fn eclipse_rm_bench(device_instance: NvU32, out: *mut GrBench) -> NV_STATUS;
}

/// Runs the integer-ALU GIOPS benchmark: a big grid of dependent-IMAD
/// chains, timed with the GPU PTIMER. Idempotent (cached after the first
/// run). Requires step17 first.
pub fn bench(device_instance: u32) -> Result<GrBench, NV_STATUS> {
    let mut out = GrBench {
        lookup_status: 0xFFFF_FFFF,
        map_status: 0xFFFF_FFFF,
        token_status: 0xFFFF_FFFF,
        submit_status: 0xFFFF_FFFF,
        sem_status: 0xFFFF_FFFF,
        num_threads: 0,
        imads_per_thread: 0,
        push_dwords: 0,
        sem_iters: 0,
        reserved_pad: 0,
        t0_ns: 0,
        t1_ns: 0,
        elapsed_ns: 0,
        total_ops: 0,
        kernel_va: 0,
        qmd_va: 0,
    };
    let status = unsafe { eclipse_rm_bench(device_instance, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Mirror of `EclipseGrEdid` (vendor/eclipse_rm_init.c): real EDID/connector
/// query via NV04_DISPLAY_COMMON + NV0073 GET_SUPPORTED/GET_CONNECT_STATE/
/// GET_EDID_V2/GET_CONNECTOR_DATA. 208 bytes: 10 u32 + edid_head[32] +
/// 2 u32 + 2x [u32; 16].
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GrEdid {
    pub alloc_status: NvU32,
    pub supported_status: NvU32,
    pub display_mask: NvU32,
    pub display_mask_ddc: NvU32,
    pub connect_status: NvU32,
    pub connected_mask: NvU32,
    pub edid_status: NvU32,
    pub edid_display_id: NvU32,
    pub edid_size: NvU32,
    pub edid_valid: NvU32,
    pub edid_head: [u8; 32],
    pub conn_type_status: NvU32,
    pub conn_type_count: NvU32,
    pub conn_type_display_id: [NvU32; 16],
    /// NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_* per output.
    pub conn_type: [NvU32; 16],
}

extern "C" {
    fn eclipse_rm_edid(device_instance: NvU32, out: *mut GrEdid) -> NV_STATUS;
}

/// Real display query: which outputs exist, which are connected, and the
/// EDID of the first connected one. Read-only (never programs the display
/// engine). Requires step16 first.
pub fn edid(device_instance: u32) -> Result<GrEdid, NV_STATUS> {
    let mut out = GrEdid {
        alloc_status: 0xFFFF_FFFF,
        supported_status: 0xFFFF_FFFF,
        display_mask: 0,
        display_mask_ddc: 0,
        connect_status: 0xFFFF_FFFF,
        connected_mask: 0,
        edid_status: 0xFFFF_FFFF,
        edid_display_id: 0,
        edid_size: 0,
        edid_valid: 0,
        edid_head: [0u8; 32],
        conn_type_status: 0xFFFF_FFFF,
        conn_type_count: 0,
        conn_type_display_id: [0u32; 16],
        conn_type: [0u32; 16],
    };
    let status = unsafe { eclipse_rm_edid(device_instance, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Fetches the GSP-reported interrupt kernel table (boxed: ~2 KiB).
pub fn intr_table(device_instance: u32) -> Result<alloc::boxed::Box<IntrTable>, NV_STATUS> {
    let mut out = alloc::boxed::Box::new(IntrTable {
        ctrl_status: 0,
        table_len: 0,
        entries: [IntrTableEntry {
            engine_idx: 0,
            pmc_intr_mask: 0,
            vector_stall: 0,
            vector_non_stall: 0,
        }; 128],
    });
    let status = unsafe { eclipse_rm_intr_table(device_instance, &mut *out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Mirror of `EclipseStateInitResult` (vendor/eclipse_rm_init.c): per-phase
/// NV_STATUS of the real RmInitAdapter device bring-up. `0xFFFFFFFF` means
/// the phase was not reached (an earlier phase failed).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StateInitResult {
    pub pre_init_status: NvU32,
    pub init_status: NvU32,
    pub load_status: NvU32,
}

extern "C" {
    fn eclipse_rm_state_init(device_instance: NvU32, out: *mut StateInitResult) -> NV_STATUS;
}

/// Runs gpumgrStatePreInitGpu / StateInitGpu / StateLoadGpu on an attached,
/// GSP-booted GPU -- the rest of the real RmInitAdapter sequence.
pub fn state_init(device_instance: u32) -> Result<StateInitResult, NV_STATUS> {
    let mut out = StateInitResult {
        pre_init_status: 0xFFFF_FFFF,
        init_status: 0xFFFF_FFFF,
        load_status: 0xFFFF_FFFF,
    };
    let status = unsafe { eclipse_rm_state_init(device_instance, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Mirror of `EclipseStep10Result` (vendor/eclipse_rm_init.c): per-phase
/// NV_STATUS of the first real copy-engine data movement (CE memset A,
/// CE memset B=poison, CE copy A->B, CPU readback verify of B through BAR2)
/// on the state-loaded GPU. `0xFFFFFFFF` = phase not reached.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Step10Result {
    pub ce_utils_status: NvU32,
    pub alloc_a_status: NvU32,
    pub alloc_b_status: NvU32,
    pub poison_status: NvU32,
    pub memset_status: NvU32,
    pub copy_status: NvU32,
    pub verify_status: NvU32,
    pub buffer_size: NvU64,
    pub pa_a: NvU64,
    pub pa_b: NvU64,
    pub pattern: NvU32,
    pub poison: NvU32,
    pub dwords_checked: NvU32,
    pub mismatch_count: NvU32,
    pub first_mismatch_idx: NvU32,
    pub first_mismatch_val: NvU32,
}

extern "C" {
    fn eclipse_rm_step10(device_instance: NvU32, out: *mut Step10Result) -> NV_STATUS;

    fn eclipse_rm_mark_console_gpu(
        device_instance: NvU32,
        console_size: NvU64,
        console_at_bar1_base: u8,
    ) -> NV_STATUS;

    fn eclipse_rm_ce_blit(
        gpu_instance: NvU32,
        dst_fb_vram_offset: NvU64,
        src_sysmem_pa: NvU64,
        size: NvU64,
    ) -> NV_STATUS;

    fn eclipse_rm_ce_fill_fb(
        gpu_instance: NvU32,
        fb_vram_offset: NvU64,
        size: NvU64,
        pattern: NvU32,
    ) -> NV_STATUS;

    fn eclipse_rm_ce_fill_fb_p2p(
        gpu_instance: NvU32,
        dst_host_pa: NvU64,
        size: NvU64,
        pattern: NvU32,
    ) -> NV_STATUS;

    fn eclipse_rm_ce_blit_p2p(
        gpu_instance: NvU32,
        dst_host_pa: NvU64,
        src_sysmem_pa: NvU64,
        size: NvU64,
    ) -> NV_STATUS;
}

/// Declares a GPU as the primary/console device to RM, NVIDIA's own way
/// (PDB_PROP_GPU_PRIMARY_DEVICE + BAR1-console preservation + reserved
/// console display memory -- what Linux's RmDeterminePrimaryDevice /
/// RmSetConsolePreservationParams do right before kgspInitRm). Must be
/// called BEFORE `init_gsp` so the SET_GUEST_SYSTEM_INFO RPC reports
/// `bIsPrimary = true` to the GSP.
pub fn mark_console_gpu(
    device_instance: u32,
    console_size: u64,
    console_at_bar1_base: bool,
) -> Result<(), NV_STATUS> {
    let status = unsafe {
        eclipse_rm_mark_console_gpu(device_instance, console_size, console_at_bar1_base as u8)
    };
    if status == NV_OK {
        Ok(())
    } else {
        Err(status)
    }
}

/// Runs the step-10 CE memset/copy + readback-verify test against the
/// state-loaded GPU (requires a successful `state_init` first).
pub fn step10(device_instance: u32) -> Result<Step10Result, NV_STATUS> {
    let mut out = Step10Result {
        ce_utils_status: 0xFFFF_FFFF,
        alloc_a_status: 0xFFFF_FFFF,
        alloc_b_status: 0xFFFF_FFFF,
        poison_status: 0xFFFF_FFFF,
        memset_status: 0xFFFF_FFFF,
        copy_status: 0xFFFF_FFFF,
        verify_status: 0xFFFF_FFFF,
        buffer_size: 0,
        pa_a: 0,
        pa_b: 0,
        pattern: 0,
        poison: 0,
        dwords_checked: 0,
        mismatch_count: 0,
        first_mismatch_idx: 0,
        first_mismatch_val: 0,
    };
    let status = unsafe { eclipse_rm_step10(device_instance, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// CE-copy from a pre-existing host-sysmem physical range (the compositor's
/// dumb buffer at `src_sysmem_pa`) into the GOP scanout framebuffer in VRAM
/// (at `dst_fb_vram_offset`) via the persistent CeUtils channel.
///
/// On TU10x, BAR1 is a linear window onto VRAM from offset 0, so:
/// `dst_fb_vram_offset = boot_fb_phys_cpu - bar1_phys`.
///
/// The call is synchronous: it returns only after the CE completion semaphore
/// fires.  The caller should flush any CPU write-combining writes to the dumb
/// buffer first (the C implementation also calls `osFlushCpuWriteCombineBuffer`
/// internally as a belt-and-suspenders measure).
///
/// Returns the raw `NV_STATUS` from `ceutilsMemcopy` (`NV_OK` = `0` on
/// success).
pub fn ce_blit(
    gpu_instance: u32,
    dst_fb_vram_offset: u64,
    src_sysmem_pa: u64,
    size: u64,
) -> NV_STATUS {
    // [rpc-lock] Serialize with every other RM entry, like the memory/exec
    // paths. A CE present runs from scanout WHILE NVK allocates on another
    // thread; without this gate the two enter the RM concurrently and trip its
    // API-lock invariant (`Assertion failed: RPC locking violation @
    // rpc.c:9834`), which fails the concurrent allocation RPC — seen on the RTX
    // as `zink: couldn't allocate memory heap=0` / `createImageFromDmaBufs
    // failed`. See the RM_CALL_GATE rationale at the top of this file.
    let _gate = RmGate::lock();
    unsafe { eclipse_rm_ce_blit(gpu_instance, dst_fb_vram_offset, src_sysmem_pa, size) }
}

/// CE-memset the GOP scanout framebuffer to a solid colour via the persistent
/// CeUtils channel.  Use as a cheap visual test to confirm `fb_vram_offset`
/// is correct before wiring the full [`ce_blit`] path.
///
/// Only the **low byte** of `pattern` is written (CE `SET_REMAP_COMPONENTS`
/// byte-replication semantics, same as step-10).  Pass `0x00` for black,
/// `0xFF` for white.
///
/// Returns the raw `NV_STATUS` from `ceutilsMemset`.
pub fn ce_fill_fb(gpu_instance: u32, fb_vram_offset: u64, size: u64, pattern: u32) -> NV_STATUS {
    // [rpc-lock] See `ce_blit`: gate this RM entry so a CE op never races a
    // concurrent NVK allocation into the RM (rpc.c:9834 API-lock violation).
    let _gate = RmGate::lock();
    unsafe { eclipse_rm_ce_fill_fb(gpu_instance, fb_vram_offset, size, pattern) }
}

/// P2P variant of [`ce_fill_fb`]: CE-memset a raw HOST physical address
/// (`dst_host_pa`, e.g. the console GPU's scanout-FB BAR1 address) via THIS
/// GPU's CeUtils channel, using an ADDR_SYSMEM descriptor. Run on the compute
/// GPU with `dst_host_pa = boot_fb_phys` to paint the console GPU's screen over
/// PCIe peer-to-peer, without bringing up the (flaky) console GPU. If P2P is
/// blocked by the chipset (ACS), the CE still returns NV_OK but nothing lands.
pub fn ce_fill_fb_p2p(gpu_instance: u32, dst_host_pa: u64, size: u64, pattern: u32) -> NV_STATUS {
    // [rpc-lock] See `ce_blit`: gate this RM entry (rpc.c:9834 API-lock).
    let _gate = RmGate::lock();
    unsafe { eclipse_rm_ce_fill_fb_p2p(gpu_instance, dst_host_pa, size, pattern) }
}

/// P2P variant of [`ce_blit`]: CE-copy `src_sysmem_pa` (dumb buffer, RAM) to a
/// raw HOST physical address `dst_host_pa` (peer GPU BAR1) via THIS GPU's
/// CeUtils channel. Run on the compute GPU to present into the console GPU's
/// scanout FB over PCIe peer-to-peer.
pub fn ce_blit_p2p(
    gpu_instance: u32,
    dst_host_pa: u64,
    src_sysmem_pa: u64,
    size: u64,
) -> NV_STATUS {
    // [rpc-lock] See `ce_blit`: gate this RM entry (rpc.c:9834 API-lock).
    let _gate = RmGate::lock();
    unsafe { eclipse_rm_ce_blit_p2p(gpu_instance, dst_host_pa, src_sysmem_pa, size) }
}

/// Mirror of `EclipseGemAlloc` (vendor/eclipse_rm_init.c).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GemAlloc {
    pub alloc_status: NvU32,
    pub h_memory: NvU32,
}

extern "C" {
    fn eclipse_rm_chan_notifier_pa(device_instance: NvU32, pa: *mut u64) -> NV_STATUS;
    fn eclipse_rm_class_alloc(
        device_instance: NvU32,
        ctx_idx: NvU32,
        class_id: NvU32,
        h_object: *mut NvU32,
        alloc_status: *mut NvU32,
    ) -> NV_STATUS;
    fn eclipse_rm_class_free(device_instance: NvU32, h_object: NvU32) -> NV_STATUS;
    fn eclipse_rm_gem_alloc(
        device_instance: NvU32,
        size: u64,
        b_sysmem: NvU32,
        out: *mut GemAlloc,
    ) -> NV_STATUS;
    fn eclipse_rm_gem_free(device_instance: NvU32, h_memory: NvU32) -> NV_STATUS;
}

/// Backing store for the nouveau-uAPI `GEM_NEW`, in one of the two domains
/// nouveau exposes:
///
/// * `sysmem == false` -> VRAM (`NV01_MEMORY_LOCAL_USER`, the class `step17`
///   uses for USERD), i.e. `NOUVEAU_GEM_DOMAIN_VRAM`.
/// * `sysmem == true` -> host system memory (`NV01_MEMORY_SYSTEM`,
///   contiguous), i.e. `NOUVEAU_GEM_DOMAIN_GART`.
///
/// Both are needed: NVK's `nvkmd_nouveau_alloc_tiled_mem` picks exactly ONE
/// domain per allocation, so a GART request arrives with no VRAM bit at all.
///
/// Requires `step16` first (needs `hClient`/`hDevice`). Not idempotent or
/// cached: every call allocates a new object.
pub fn gem_alloc(device_instance: u32, size: u64, sysmem: bool) -> Result<GemAlloc, NV_STATUS> {
    let _gate = RmGate::lock();
    let mut out = GemAlloc {
        alloc_status: 0xFFFF_FFFF,
        h_memory: 0,
    };
    let status =
        unsafe { eclipse_rm_gem_alloc(device_instance, size, sysmem as NvU32, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Allocates an ENGINE CLASS OBJECT (3D/compute/copy/2D/inline) on the channel
/// of GPU context `ctx_idx`. This is what makes the RM/GSP build the engine's
/// channel context -- for GR classes, the golden context image, patch buffer
/// and global buffers, mapped into that channel's VAS. Without it the first
/// method of that class makes the engine load a context that was never built
/// *on that channel*, and the channel hangs (MMU fault attributed to the
/// engine on the compositor's singleton channel; a silent PBDMA stall one GP
/// entry short of the fence on a client channel).
///
/// `ctx_idx` MUST be the context whose EXEC will submit this class's methods:
/// 0 for the compositor (the step16/step17 singleton), or the GL client's own
/// `>= 1` from [`ctx_alloc`]. Routing a client's class to context 0's channel
/// (as this once did unconditionally) is exactly the bug that left NVK's client
/// channel with no GR context. Requires `step16`+`step17` (shared ladder base)
/// and, for `ctx_idx >= 1`, that context already built. Returns
/// `(h_object, alloc_status)`; the object is real only when `alloc_status == 0`.
pub fn class_alloc(
    device_instance: u32,
    ctx_idx: u32,
    class_id: u32,
) -> Result<(u32, u32), NV_STATUS> {
    let _gate = RmGate::lock();
    let mut h_object = 0u32;
    let mut alloc_status = 0xffff_ffffu32;
    let status = unsafe {
        eclipse_rm_class_alloc(
            device_instance,
            ctx_idx,
            class_id,
            &mut h_object,
            &mut alloc_status,
        )
    };
    if status == NV_OK {
        Ok((h_object, alloc_status))
    } else {
        Err(status)
    }
}

/// Frees a class object from [`class_alloc`].
pub fn class_free(device_instance: u32, h_object: u32) -> NV_STATUS {
    let _gate = RmGate::lock();
    unsafe { eclipse_rm_class_free(device_instance, h_object) }
}

/// Physical address of the RM-backed channel's error notifier -- the 4 KiB
/// sysmem buffer `step17` registers as `hObjectError`, where the RM writes an
/// `NvNotification` when robust-channel recovery tears the channel down (MMU
/// fault / PBDMA error / GR exception). Pure memdesc bookkeeping, same recipe
/// as [`gem_map_cpu`]; the CALLER reads the page through its own memory
/// window (`crate::bus::phys_to_virt` on the drivers side). CPU-mapping it
/// through the RM's transfer surfaces instead is known to fault on this
/// hardware -- see the C function's comment.
pub fn chan_notifier_pa(device_instance: u32) -> Result<u64, NV_STATUS> {
    let _gate = RmGate::lock();
    let mut pa = 0u64;
    let status = unsafe { eclipse_rm_chan_notifier_pa(device_instance, &mut pa) };
    if status == NV_OK && pa != 0 {
        Ok(pa)
    } else if status == NV_OK {
        Err(NV_ERR_GENERIC)
    } else {
        Err(status)
    }
}

/// Frees a GEM object allocated by [`gem_alloc`].
pub fn gem_free(device_instance: u32, h_memory: u32) -> NV_STATUS {
    let _gate = RmGate::lock();
    unsafe { eclipse_rm_gem_free(device_instance, h_memory) }
}

/// Mirror of `EclipseGemMapCpu` (vendor/eclipse_rm_init.c).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GemMapCpu {
    pub lookup_status: NvU32,
    /// 0 (`ADDR_FBMEM`) when `phys_addr` is meaningful; any other value
    /// means the lookup itself succeeded but this object isn't vidmem, and
    /// `phys_addr`/`size` were left zeroed.
    pub address_space: NvU32,
    pub phys_addr: u64,
    pub size: u64,
}

extern "C" {
    fn eclipse_rm_gem_map_cpu(device_instance: NvU32, h_memory: NvU32, out: *mut GemMapCpu) -> NV_STATUS;
}

/// `NV_ADDRESS_SPACE` values (open-gpu-kernel-modules
/// `g_mem_desc_nvoc.h`): the aperture a memory descriptor lives in.
/// `eclipse_rm_gem_map_cpu` only fills `phys_addr` when the object is in
/// `ADDR_FBMEM`, because only then is the address BAR1-relative.
pub const ADDR_UNKNOWN: u32 = 0;
pub const ADDR_SYSMEM: u32 = 1;
pub const ADDR_FBMEM: u32 = 2;


/// Resolves `h_memory` (from [`gem_alloc`]) to the physical address a CPU can
/// reach it at -- through the GPU's BAR1 aperture for VRAM, or the plain host
/// physical address for GART. A pure RM bookkeeping query (no register
/// access, cannot hang). The caller still needs to check `.lookup_status == 0`
/// (this wrapper only reports the outer NV_STATUS, matching [`gem_alloc`]'s
/// own `.alloc_status` convention) and that `.address_space` is `ADDR_FBMEM`
/// (2) or `ADDR_SYSMEM` (1) -- NOT 0, which is `ADDR_UNKNOWN` -- before
/// trusting `.phys_addr`. The C side refuses a non-contiguous sysmem object,
/// since one physical address cannot describe a scattered allocation.
pub fn gem_map_cpu(device_instance: u32, h_memory: u32) -> Result<GemMapCpu, NV_STATUS> {
    let _gate = RmGate::lock();
    let mut out = GemMapCpu {
        lookup_status: 0xFFFF_FFFF,
        address_space: 0xFFFF_FFFF,
        phys_addr: 0,
        size: 0,
    };
    let status = unsafe { eclipse_rm_gem_map_cpu(device_instance, h_memory, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Mirror of `EclipseVmBind` (vendor/eclipse_rm_init.c).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VmBind {
    pub virt_status: NvU32,
    pub map_status: NvU32,
    pub h_virt: NvU32,
    pub actual_va: u64,
}

extern "C" {
    fn eclipse_rm_vm_bind_map(
        device_instance: NvU32,
        ctx_idx: NvU32,
        h_memory: NvU32,
        size: u64,
        requested_va: u64,
        bo_offset: u64,
        out: *mut VmBind,
    ) -> NV_STATUS;
    fn eclipse_rm_vm_bind_unmap(device_instance: NvU32, h_virt: NvU32, size: u64, va: u64) -> NV_STATUS;
}

/// Maps `h_memory` (from [`gem_alloc`]) into the VAS of context `ctx_idx` at
/// `requested_va`, generalizing `step17`'s items 3+4. `ctx_idx` 0 is the
/// compositor's singleton VAS (`step16`); `ctx_idx >= 1` is a GL client's own
/// VAS from [`ctx_alloc`]. Requires that context's VAS allocated first.
pub fn vm_bind_map(
    device_instance: u32,
    ctx_idx: u32,
    h_memory: u32,
    size: u64,
    requested_va: u64,
    bo_offset: u64,
) -> Result<VmBind, NV_STATUS> {
    let _gate = RmGate::lock();
    let mut out = VmBind {
        virt_status: 0xFFFF_FFFF,
        map_status: 0xFFFF_FFFF,
        h_virt: 0,
        actual_va: 0,
    };
    let status = unsafe {
        eclipse_rm_vm_bind_map(device_instance, ctx_idx, h_memory, size, requested_va, bo_offset, &mut out)
    };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Unmaps and frees the VA range created by [`vm_bind_map`] (`h_virt` from
/// its result, same `size`/`actual_va`).
pub fn vm_bind_unmap(device_instance: u32, h_virt: u32, size: u64, va: u64) -> NV_STATUS {
    let _gate = RmGate::lock();
    unsafe { eclipse_rm_vm_bind_unmap(device_instance, h_virt, size, va) }
}

/// Mirror of `EclipseExecSubmit` (vendor/eclipse_rm_init.c).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExecSubmit {
    pub lookup_status: NvU32,
    pub map_status: NvU32,
    pub token_status: NvU32,
    pub submit_status: NvU32,
    pub work_token: NvU32,
    pub runlist_id: NvU32,
    pub gp_put_after: NvU32,
}

extern "C" {
    fn eclipse_rm_exec_submit(
        device_instance: NvU32,
        ctx_idx: NvU32,
        push_va: u64,
        push_len_bytes: NvU32,
        out: *mut ExecSubmit,
    ) -> NV_STATUS;
}

/// Submits `(push_va, push_len_bytes)` -- a pushbuffer the caller already
/// wrote (via a `vm_bind_map`ed GEM object) -- on the live `step17`/
/// `CHANNEL_ALLOC` channel, generalizing `step18`'s GP-entry/GPPut/doorbell
/// mechanics for arbitrary content instead of a hardcoded method stream.
/// Requires `step17` (or the nouveau-uAPI `CHANNEL_ALLOC`, which calls it)
/// first. `push_len_bytes` must be a non-zero multiple of 4.
pub fn exec_submit(
    device_instance: u32,
    ctx_idx: u32,
    push_va: u64,
    push_len_bytes: u32,
) -> Result<ExecSubmit, NV_STATUS> {
    let _gate = RmGate::lock();
    let mut out = ExecSubmit {
        lookup_status: 0xFFFF_FFFF,
        map_status: 0xFFFF_FFFF,
        token_status: 0xFFFF_FFFF,
        submit_status: 0xFFFF_FFFF,
        work_token: 0,
        runlist_id: 0,
        gp_put_after: 0,
    };
    let status = unsafe { eclipse_rm_exec_submit(device_instance, ctx_idx, push_va, push_len_bytes, &mut out) };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Mirror of `EclipseExecSignal` (vendor/eclipse_rm_init.c).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExecSignal {
    pub lookup_status: NvU32,
    pub map_status: NvU32,
    pub token_status: NvU32,
    pub submit_status: NvU32,
    pub fence_submit_status: NvU32,
    pub fence_wait_status: NvU32,
    pub fence_value: NvU32,
    pub work_token: NvU32,
    pub runlist_id: NvU32,
    /// PHYSICAL address of this submit's fence semaphore (in the channel's
    /// sysmem buffer). Set once the submit reaches the doorbell. For the async
    /// path ([`exec_submit_async`]) this is how the caller polls the fence
    /// WITHOUT holding the RM gate: `phys_to_virt(fence_sem_phys)` reads the
    /// same u32 the RM's own inline poll would.
    pub fence_sem_phys: u64,
}

extern "C" {
    fn eclipse_rm_exec_submit_signaled(
        device_instance: NvU32,
        ctx_idx: NvU32,
        push_va: u64,
        push_len_bytes: NvU32,
        fence_payload: NvU32,
        timeout_ms: NvU32,
        out: *mut ExecSignal,
    ) -> NV_STATUS;
}

/// Like [`exec_submit`], but appends a second, kernel-authored GP entry
/// (a single host semaphore RELEASE writing `fence_payload` to a fixed
/// scratch offset in the channel's OWN buffer -- never the caller's) right
/// after the caller's pushbuffer, then polls it for up to `timeout_ms`.
/// Backs the nouveau-uAPI `EXEC` ioctl's `sig_count == 1` path -- see that
/// function's doc for exactly what a landed fence does and does not prove
/// (HOST/PBDMA fetch, not necessarily compute-engine completion).
pub fn exec_submit_signaled(
    device_instance: u32,
    ctx_idx: u32,
    push_va: u64,
    push_len_bytes: u32,
    fence_payload: u32,
    timeout_ms: u32,
) -> Result<ExecSignal, NV_STATUS> {
    let _gate = RmGate::lock();
    let mut out = ExecSignal {
        lookup_status: 0xFFFF_FFFF,
        map_status: 0xFFFF_FFFF,
        token_status: 0xFFFF_FFFF,
        submit_status: 0xFFFF_FFFF,
        fence_submit_status: 0xFFFF_FFFF,
        fence_wait_status: 0xFFFF_FFFF,
        fence_value: 0,
        work_token: 0,
        runlist_id: 0,
        fence_sem_phys: 0,
    };
    let status = unsafe {
        eclipse_rm_exec_submit_signaled(
            device_instance,
            ctx_idx,
            push_va,
            push_len_bytes,
            fence_payload,
            timeout_ms,
            &mut out,
        )
    };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}

/// Like [`exec_submit_signaled`] but ASYNC: submits the caller's pushbuffer plus
/// the kernel fence GP entry and rings the doorbell, then returns IMMEDIATELY
/// (`timeout_ms = 0` tells the C side not to poll). The `RmGate` is held only for
/// the submit -- microseconds -- and released the instant this returns; the
/// caller then polls `out.fence_sem_phys` via `phys_to_virt` on its own, WITHOUT
/// the gate.
///
/// This is the whole point of the split: the old path held the gate (a spinlock)
/// across the full up-to-1 s fence poll, so every other thread -- the compositor
/// included -- busy-waited behind it; a hung client wedged the desktop. Now a
/// slow or hung fence spins only the client's OWN thread, which is preemptible,
/// and never the RM gate. Safe against the RPC/VMAR/TLB-shootdown deadlocks the
/// gate guards: only ONE thread is ever inside the RM (the async poll is a plain
/// CPU read of a sysmem semaphore, not an RM entry).
///
/// `out.fence_wait_status` is NOT set here (no poll happened) -- the caller sets
/// it from its own poll of `fence_sem_phys`.
pub fn exec_submit_async(
    device_instance: u32,
    ctx_idx: u32,
    push_va: u64,
    push_len_bytes: u32,
    fence_payload: u32,
) -> Result<ExecSignal, NV_STATUS> {
    let _gate = RmGate::lock();
    let mut out = ExecSignal {
        lookup_status: 0xFFFF_FFFF,
        map_status: 0xFFFF_FFFF,
        token_status: 0xFFFF_FFFF,
        submit_status: 0xFFFF_FFFF,
        fence_submit_status: 0xFFFF_FFFF,
        fence_wait_status: 0xFFFF_FFFF,
        fence_value: 0,
        work_token: 0,
        runlist_id: 0,
        fence_sem_phys: 0,
    };
    // timeout_ms = 0 => submit only, do not poll (see the C function).
    let status = unsafe {
        eclipse_rm_exec_submit_signaled(
            device_instance,
            ctx_idx,
            push_va,
            push_len_bytes,
            fence_payload,
            0,
            &mut out,
        )
    };
    if status == NV_OK {
        Ok(out)
    } else {
        Err(status)
    }
}
