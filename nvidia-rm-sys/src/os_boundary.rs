//! Implementation of the remaining "OBJOS" internal-service surface
//! (src/nvidia/generated/g_os_nvoc.h) revealed once nearly the entire
//! real Resource Manager core links (see build.rs) -- distinct from
//! both `os_interface.rs` (the real os-interface.h ABI) and the smaller
//! `os_services.rs` set from an earlier pass. Signatures transcribed
//! verbatim from the real generated header.
//!
//! A handful (`osPciInitHandle`, `osUnmapKernelSpace`, `osGetRandomBytes`)
//! delegate straight to the matching os_interface.rs function since the
//! signatures happen to line up exactly. `osPciReadByte/Word/Dword`,
//! `osPciWriteWord/Dword`, and `osMapKernelSpace` have a DIFFERENT
//! arity/return convention than their os_interface.rs namesakes (this
//! surface returns the value directly rather than NV_STATUS-plus-
//! out-param, and osMapKernelSpace has an extra `Protect` argument), so
//! those are hand-written against the same `KernelHooks` facade
//! instead of naively delegating.
//!
//! Everything else defaults to the same "not supported" convention
//! already established throughout os_interface.rs: ACPI method calls,
//! Tegra SoC integration, NUMA, cgroups, vGPU, IMEX fabric memory
//! export, host filesystem access, and internal RM diagnostic/crash-log
//! bookkeeping are all out of scope for Eclipse's single discrete-GPU
//! bring-up.
#![allow(non_snake_case)]

use crate::hooks::with_hooks;
use crate::types::*;

#[no_mangle]
pub extern "C" fn osAcquireRmSema(arg0: *mut c_void) -> *mut c_void {
    let _ = arg0;
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn osAddRecordForCrashLog(arg0: *mut c_void, arg1: NvU32) {
    let _ = arg0;
    let _ = arg1;
}

#[no_mangle]
pub extern "C" fn osAllocAcquirePage(arg0: NvU64, arg1: NvU32) {
    let _ = arg0;
    let _ = arg1;
}

// osAllocPagesInternal / osFreePagesInternal / osMapSystemMemory /
// osUnmapSystemMemory now have REAL implementations in
// vendor/eclipse_rm_mem.c (backed by Eclipse's contiguous DMA frame
// allocator + kernel physmap), replacing the no-op stubs that used to live
// here. The no-op osAllocPagesInternal in particular silently "succeeded"
// (returned null == NV_OK) without allocating anything, so every sysmem
// memdesc had empty PTEs -- which surfaced on real hardware as
// GspMsgQueuesInit failing to map its shared buffer (NV_ERR_NO_MEMORY).

#[no_mangle]
pub extern "C" fn osAllocPagesNode(
    arg0: NvS32,
    arg1: NvU64,
    arg2: NvU32,
    arg3: *mut NvU64,
) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    let _ = arg3;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osAllocReleasePage(arg0: NvU64, arg1: NvU32) {
    let _ = arg0;
    let _ = arg1;
}

#[no_mangle]
pub extern "C" fn osAllocWaitQueue(ppWq: *mut *mut c_void) -> NV_STATUS {
    let _ = ppWq;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osAllocatedRmClient(pOSInfo: *mut c_void) {
    let _ = pOSInfo;
}

#[no_mangle]
pub extern "C" fn osApiLockAcquireConfigureFlags(flags: NvU32) -> NvU32 {
    let _ = flags;
    0
}

#[no_mangle]
pub extern "C" fn osAssertFailed() {
    // Loud on purpose: a silent assert hook cost real diagnostic time --
    // the assert TEXT arrives separately via nv_printf (now with real
    // formatting, see vendor/glue.c), but this marker proves the failure
    // path itself ran even if that message is ever garbled or filtered.
    log::error!("[nvidia-rm] osAssertFailed() -- an RM NV_ASSERT fired; see adjacent [nvidia-rm] message for details");
}

#[no_mangle]
pub extern "C" fn osAttachGpu(arg0: *mut c_void, arg1: *mut c_void) -> NV_STATUS {
    // Real osAttachGpu (osinit.c:300) is Linux platform bookkeeping --
    // it links pGpu <-> nv_state_t via pOsGpuInfo and returns NV_OK.
    // Eclipse has no nv_state_t (we pass pOsAttachArg=NULL) and tracks the
    // GPU its own way, so there is nothing to do -- but it MUST return
    // NV_OK: gpumgrAttachGpu (gpu_mgr.c:1569) checks the result and bails
    // the whole attach with this status otherwise. The old NOT_SUPPORTED
    // stub is exactly the 0x56 real hardware reported.
    let _ = arg0;
    let _ = arg1;
    NV_OK
}

#[no_mangle]
pub extern "C" fn osAttachToProcess(arg0: *mut *mut c_void, arg1: NvU32) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osBugCheck(bugCode: NvU32) {
    let _ = bugCode;
}

#[no_mangle]
pub extern "C" fn osCallACPI_DDC(
    arg0: *mut c_void,
    arg1: NvU32,
    arg2: *mut NvU8,
    arg3: *mut NvU32,
    arg4: NvBool,
) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    let _ = arg3;
    let _ = arg4;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osCallACPI_DOD(
    arg0: *mut c_void,
    arg1: *mut NvU32,
    arg2: *mut NvU32,
) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osCallACPI_DSM(
    pGpu: *mut c_void,
    acpiDSMFunction: *mut c_void,
    NVHGDSMSubfunction: NvU32,
    pInOut: *mut NvU32,
    size: *mut NvU16,
) -> NV_STATUS {
    let _ = pGpu;
    let _ = acpiDSMFunction;
    let _ = NVHGDSMSubfunction;
    let _ = pInOut;
    let _ = size;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osCallACPI_MXDM(arg0: *mut c_void, arg1: NvU32, arg2: *mut NvU32) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osCallACPI_MXDS(arg0: *mut c_void, arg1: NvU32, arg2: *mut NvU32) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osCallACPI_NVHG_ROM(
    arg0: *mut c_void,
    arg1: *mut NvU32,
    arg2: *mut NvU32,
) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osCancelNanoTimer(pArg1: *mut c_void, pArg2: *mut c_void) -> NV_STATUS {
    let _ = pArg1;
    let _ = pArg2;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osCgroupImplementation() -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn osCgroupRegisterRegion(pGpu: *mut c_void, size: NvU64) -> *mut c_void {
    let _ = pGpu;
    let _ = size;
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn osCgroupUnregisterRegion(region: *mut c_void) {
    let _ = region;
}

#[no_mangle]
pub extern "C" fn osCheckAccess(accessRight: *mut c_void) -> NvBool {
    let _ = accessRight;
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osClientGcoffDisallowRefcount(pArg1: *mut c_void, arg2: NvBool) {
    let _ = pArg1;
    let _ = arg2;
}

#[no_mangle]
pub extern "C" fn osClientGroupID(process: NvU64, pidInfo: *mut c_void) -> *mut c_void {
    let _ = process;
    let _ = pidInfo;
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn osCloseFile(pFile: *mut c_void) {
    let _ = pFile;
}

#[no_mangle]
pub extern "C" fn osCondAcquireRmSema(arg0: *mut c_void) -> *mut c_void {
    let _ = arg0;
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn osConfigurePcieReqAtomics(
    pOsGpuInfo: *mut c_void,
    pMask: *mut NvU32,
) -> NV_STATUS {
    let _ = pOsGpuInfo;
    let _ = pMask;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osCountTailPages(arg0: NvU64) -> NvU32 {
    let _ = arg0;
    0
}

#[no_mangle]
pub extern "C" fn osCreateMemFromOsDescriptor(
    pGpu: *mut c_void,
    pDescriptor: *mut c_void,
    hClient: *mut c_void,
    flags: NvU32,
    pLimit: *mut NvU64,
    ppMemDesc: *mut *mut c_void,
    descriptorType: NvU32,
    privilegeLevel: *mut c_void,
) -> NV_STATUS {
    let _ = pGpu;
    let _ = pDescriptor;
    let _ = hClient;
    let _ = flags;
    let _ = pLimit;
    let _ = ppMemDesc;
    let _ = descriptorType;
    let _ = privilegeLevel;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osCreateNanoTimer(
    pArg1: *mut c_void,
    tmrEvent: *mut c_void,
    tmrUserData: *mut *mut c_void,
) -> NV_STATUS {
    let _ = pArg1;
    let _ = tmrEvent;
    let _ = tmrUserData;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osCxlSetCaching(pGpu: *mut c_void, bEnableCache: NvBool) {
    let _ = pGpu;
    let _ = bEnableCache;
}

#[no_mangle]
pub extern "C" fn osDelay(ms: NvU32) -> NV_STATUS {
    // Bounded spin on the monotonic clock: Eclipse's RM bring-up is
    // single-threaded and polled, so a busy-wait is the correct primitive
    // at this layer (there is no scheduler sleep to yield to). Used by RM
    // retry paths and by the eclipse glue (EXP1c PDISP-restore pacing,
    // GET_CONNECT_STATE real-detection retry).
    let start = with_hooks(0u64, |h| h.monotonic_time_ns());
    if start == 0 {
        // No clock hook (early boot): degrade to no delay rather than hang.
        return NV_OK;
    }
    let target = start + (ms as u64) * 1_000_000;
    while with_hooks(0u64, |h| h.monotonic_time_ns()) < target {
        core::hint::spin_loop();
    }
    NV_OK
}

#[no_mangle]
pub extern "C" fn osDelayNs(arg0: NvU32) -> NV_STATUS {
    let _ = arg0;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osDeleteRecordForCrashLog(arg0: *mut c_void) {
    let _ = arg0;
}

#[no_mangle]
pub extern "C" fn osDereferenceObjectCount(pEvent: *mut c_void) -> NV_STATUS {
    let _ = pEvent;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osDestroyNanoTimer(pArg1: *mut c_void, pArg2: *mut c_void) -> NV_STATUS {
    let _ = pArg1;
    let _ = pArg2;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osDetachFromProcess(arg0: *mut c_void) {
    let _ = arg0;
}

// osDevRead/WriteReg{008,016,032} were unconditional stubs (always read 0,
// writes dropped) -- a confirmed real bug, not a deliberate "not supported"
// stub: this is the exact function real chip-ID detection uses BEFORE an
// OBJGPU even exists (gpumgrGetGpuHalFactor in gpu_mgr.c, pGpu=NULL), so
// stubbing it made TU106 undetectable (chipId0/chipId1 always read as 0).
//
// Real DEVICE_MAPPING layout (src/nvidia/generated/g_gpu_access_nvoc.h,
// aggregated from gpu_access.h -- confirmed against the actual struct body,
// not guessed):
//   typedef struct DEVICE_MAPPING {
//       GPUHWREG  *gpuNvAddr;   // CPU Virtual Address -- first field
//       RmPhysAddr gpuNvPAddr;
//       NvU64      gpuNvLength;
//       ...
//   } DEVICE_MAPPING;
// `gpuNvAddr` is the already page-table-mapped CPU virtual base of the
// aperture (for the discrete-GPU path, Eclipse's own bar0_virt -- see
// gpu_mgr.c's `gpuDevMapping.gpuNvAddr = pAttachArg->regBaseAddr`, and
// eclipse_rm_init.c's `gpuAttachArg.regBaseAddr = (GPUHWREG*)bar0_virt`).
// Being first means reading the pointer-sized value at pMapping's own
// address gets it directly, with no need to know the rest of the struct.
// This is a plain mapped-memory read, not port I/O or PCI config space, so
// (unlike PCI/MMIO-map/timing) it needs no KernelHooks indirection.
/// Set once SEC2 is started for the GSP-RM resume (STARTCPU bracket in
/// osDevWriteReg032): from that point on, GSP-falcon-aperture accesses are
/// probed too, to catch the first post-"GSP FW RM ready" MMIO (the RPC
/// doorbell) that the machine now freezes on.
static GSP_PROBE_ON: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Console-GPU SEC2-resume register trace (step 11 diagnostics). Armed by
/// `seq_trace_arm()` (bringup_step11) before the console GPU's GSP boot;
/// goes LIVE at that boot's own SEC2 STARTCPU write (the point after which
/// the machine has always wedged on this GPU). From then on EVERY BAR0
/// register access logs at ERROR level (passes LOG=error, renders live),
/// pre-logged BEFORE the volatile access -- so when the wedge hits, the
/// last line on screen names the exact register and direction. Consecutive
/// same-offset repeats are collapsed (poll loops), capped as a backstop.
/// CPU pixel writes during this window are exonerated as the trigger: the
/// fully-frozen-console run (KD_GRAPHICS, zero presents) wedged identically.
static SEQ_TRACE_ARMED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static SEQ_TRACE_LIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Arm the SEC2-resume register trace for the next GSP boot. Resets the
/// per-boot line budget and dedup state so a second armed boot (e.g. the
/// console GPU after the secondary) gets a full budget of its own.
pub fn seq_trace_arm() {
    use core::sync::atomic::Ordering;
    SEQ_TRACE_LINES.store(0, Ordering::Relaxed);
    SEQ_TRACE_LAST.store(u64::MAX, Ordering::Relaxed);
    SEQ_TRACE_ARMED.store(true, Ordering::Relaxed);
    SEQ_TRACE_LIVE.store(false, Ordering::Relaxed);
}

/// Disarm the SEC2-resume register trace (boot returned).
pub fn seq_trace_disarm() {
    use core::sync::atomic::Ordering;
    SEQ_TRACE_ARMED.store(false, Ordering::Relaxed);
    SEQ_TRACE_LIVE.store(false, Ordering::Relaxed);
}

/// Post-STARTCPU bus autopsy (EXP 2 of the SEC2-resume investigation).
/// Config-space cycles are completed by the root complex even when the
/// endpoint's host interface is priv-locked (a failed config read returns
/// all-ones instead of hanging the core, unlike a MMIO read), so a paced
/// config poll right after the STARTCPU write classifies the failure
/// physics without risking the machine: (a) config alive + BAR0 dead =
/// HS-payload priv holdoff; (b) config dead / ID=0xFFFF = device fell off
/// the bus; (c) everything alive after ~2s = race, not a wedge. Armed by
/// bringup_step11/12 with the GPU's and its root port's packed config
/// handles (same 0x8000_0000|bus<<16|dev<<8|fn packing as
/// os_pci_init_handle).
static AUTOPSY_GPU_HANDLE: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
static AUTOPSY_RP_HANDLE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Arm the post-STARTCPU autopsy. `rp_handle` may be 0 (no root port found).
pub fn autopsy_arm(gpu_handle: usize, rp_handle: usize) {
    use core::sync::atomic::Ordering;
    AUTOPSY_GPU_HANDLE.store(gpu_handle, Ordering::Relaxed);
    AUTOPSY_RP_HANDLE.store(rp_handle, Ordering::Relaxed);
}

/// Disarm the post-STARTCPU autopsy.
pub fn autopsy_disarm() {
    use core::sync::atomic::Ordering;
    AUTOPSY_GPU_HANDLE.store(0, Ordering::Relaxed);
    AUTOPSY_RP_HANDLE.store(0, Ordering::Relaxed);
}

/// EXP 2 (pre-STARTCPU interrupt drain -- the Copilot "pseudo-ISR service
/// loop" hypothesis). Eclipse is 100% polled with INTx masked and NO RM ISR,
/// so during the SEC2 CORE_RESUME window a fabric/display interrupt the GPU
/// raises is never serviced -- the leading theory for why the console GPU's
/// STARTCPU posted-write stalls (fabric flow-control credit exhaustion) while
/// Linux, whose ISR drains that same interrupt, does not wedge. When armed,
/// the STARTCPU bracket in osDevWriteReg032 does, right BEFORE the posted
/// store: (1) snapshot the CPU-facing top-level interrupt tree at ERROR level
/// (survives a wedge -> names the exact pending vector), then (2) a bounded
/// write-1-to-clear service loop draining the leaf-pending registers until the
/// top register quiesces. If the wedge is unserviced-interrupt backpressure
/// this makes STARTCPU drain and the autopsy then runs; if not, the snapshot
/// still tells us which source Linux's ISR services in that window. Does NOT
/// touch PDISP (unlike EXP1), so it can't break GSP-RM's early boot. Armed
/// only by bringup_step13.
static SEC2_DRAIN_ARMED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Arm the pre-STARTCPU interrupt-drain experiment for the next GSP boot.
pub fn sec2_drain_arm() {
    SEC2_DRAIN_ARMED.store(true, core::sync::atomic::Ordering::Relaxed);
}

/// Disarm the pre-STARTCPU interrupt-drain experiment (boot returned).
pub fn sec2_drain_disarm() {
    SEC2_DRAIN_ARMED.store(false, core::sync::atomic::Ordering::Relaxed);
}

/// Linux byte-parity mode for the STARTCPU bracket. Stock Linux CORE_RESUME
/// performs EXACTLY: read SEC2 CPUCTL (0x840100), write CPUCTL_ALIAS
/// (0x840130) -- with ZERO intervening MMIO (kernel_falcon_tu102.c:231-244).
/// Eclipse's bracket historically inserted a BSI pre-read, an interrupt-tree
/// snapshot (including a DISPLAY register read at 0x611C30 and priv-ring
/// reads at 0x120058), and a W1C drain sweep between those two accesses.
/// Falsification status of the wedge theories so far: console-silent boot +
/// pre-boot PBUS clean still wedged, and the single successful boot ran a
/// build WITHOUT the display/priv-ring probe reads. When armed, the bracket
/// does NOTHING: no BSI read, no snapshot, no drain -- the store goes through
/// the generic path exactly like Linux. Armed by gsp_boot_run(quiet=true)
/// together with the console-silent window; supersedes SEC2_DRAIN_ARMED.
static LINUX_PARITY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Wedge containment (the "no more pointless reboots" machinery). The console
/// GPU's SEC2 CORE_RESUME wedge is a ~25-30% race that no driver-visible knob
/// controls (falsification ladder: console silence, PBUS clear, VGA decode,
/// MPS normalize, Linux byte-parity bracket, bIsPrimary on/off -- every
/// single-variable theory died). Instead of avoiding it, contain it: after
/// the STARTCPU store the bracket runs a SILENT config-space watch (config
/// cycles are completed by the root complex even when the endpoint's fabric
/// is dead -- a failed config read returns all-ones instead of hanging the
/// core, the same physics the old autopsy relied on). On detection,
/// WEDGE_FAKE_MMIO flips: every osDevReadReg* returns all-ones WITHOUT
/// touching hardware and every osDevWriteReg* is dropped, so the vendored
/// RM's own BSI/mailbox polling fails GRACEFULLY (NV_ERR_TIMEOUT) instead of
/// wedging the CPU on its next BAR0 access -- the machine survives, the
/// /proc read completes, and the caller can attempt a secondary-bus-reset
/// recovery + retry. NOTE: the flag is global (both GPUs' MMIO gets faked
/// while set) -- acceptable: it is only set when the fabric is already dead,
/// and cleared after a successful recovery.
static WEDGE_FAKE_MMIO: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static WEDGE_DETECTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static WEDGE_CFG_HANDLE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Arm the post-STARTCPU wedge watch with the GPU's packed config handle
/// (same 0x8000_0000|bus<<16|dev<<8|fn packing as os_pci_init_handle).
pub fn wedge_watch_arm(gpu_cfg_handle: usize) {
    WEDGE_CFG_HANDLE.store(gpu_cfg_handle, core::sync::atomic::Ordering::Relaxed);
    WEDGE_DETECTED.store(false, core::sync::atomic::Ordering::Relaxed);
}

/// Disarm the wedge watch (boot returned).
pub fn wedge_watch_disarm() {
    WEDGE_CFG_HANDLE.store(0, core::sync::atomic::Ordering::Relaxed);
}

/// Did the last armed boot detect a fabric wedge after STARTCPU?
pub fn wedge_detected() -> bool {
    WEDGE_DETECTED.load(core::sync::atomic::Ordering::Relaxed)
}

/// Clear fake-MMIO mode after a successful bus recovery (BARs restored and
/// the device answering config cycles again).
pub fn wedge_fake_mmio_clear() {
    WEDGE_FAKE_MMIO.store(false, core::sync::atomic::Ordering::Relaxed);
}

/// Is fake-MMIO mode active?
pub fn wedge_fake_mmio_on() -> bool {
    WEDGE_FAKE_MMIO.load(core::sync::atomic::Ordering::Relaxed)
}

/// Arm Linux byte-parity mode for the next GSP boot's STARTCPU bracket.
pub fn linux_parity_arm() {
    LINUX_PARITY.store(true, core::sync::atomic::Ordering::Relaxed);
}

/// Disarm Linux byte-parity mode (boot returned).
pub fn linux_parity_disarm() {
    LINUX_PARITY.store(false, core::sync::atomic::Ordering::Relaxed);
}

/// Snapshot + drain the CPU-facing top-level interrupt tree just before the
/// SEC2 STARTCPU posted write. `base` is this GPU's BAR0 virtual base
/// (`DEVICE_MAPPING::gpuNvAddr`). Register map (Turing bare-metal, PF: the
/// VF-priv window sits at BAR0 + `NV_VIRTUAL_FUNCTION_FULL_PHYS_OFFSET` =
/// 0x00B8_0000, per gpuGetVirtRegPhysOffset_TU102):
///   CPU_INTR_TOP(0)      = 0x00B8_1600  (RO pending, 32 subtree bits)
///   CPU_INTR_LEAF(i)     = 0x00B8_1000 + i*4, i=0..7  (RW, write-1-to-clear)
///   CPU_INTR_LEAF_EN(i)  = 0x00B8_1200 + i*4, i=0..7  (enable mask)
/// plus the legacy NV_PMC_INTR(0)/(1) at 0x100/0x104. All are ordinary BAR0
/// accesses (reads/writes immediately before STARTCPU are known-good from
/// prior traces); none touch the SEC2 aperture.
unsafe fn sec2_intr_snapshot_and_drain(base: *mut u8) {
    const TOP: usize = 0x00B8_1600;
    const LEAF: usize = 0x00B8_1000;
    const LEAF_EN: usize = 0x00B8_1200;
    let rd = |off: usize| core::ptr::read_volatile(base.add(off) as *const NvU32);
    let wr = |off: usize, v: NvU32| core::ptr::write_volatile(base.add(off) as *mut NvU32, v);

    // (1) Snapshot BEFORE draining. probe_line captures ALWAYS (folded into
    // the /proc read) and renders at ERROR when outside the console-quiet
    // window (quiet mode deliberately gives up live wedge forensics; step13
    // stays loud for that).
    let top0 = rd(TOP);
    let pmc0 = rd(0x100);
    let pmc1 = rd(0x104);
    crate::os_interface::probe_line(&alloc::format!(
        "[nvidia-rm] EXP2 pre-STARTCPU intr snapshot: CPU_INTR_TOP={:#010x} PMC_INTR0={:#010x} PMC_INTR1={:#010x}",
        top0, pmc0, pmc1
    ));
    for i in 0..8usize {
        let leaf = rd(LEAF + i * 4);
        let en = rd(LEAF_EN + i * 4);
        if leaf != 0 || en != 0 {
            crate::os_interface::probe_line(&alloc::format!(
                "[nvidia-rm] EXP2   LEAF[{}] pending={:#010x} en={:#010x}",
                i,
                leaf,
                en
            ));
        }
    }
    // EXP4's display/priv-ring probe reads (NV_PDISP_FE_RM_INTR_STAT_CTRL_DISP
    // 0x611C30 + NV_PPRIV_MASTER_RING_INTERRUPT_STATUS0/1 0x120058/5c) are
    // REMOVED. Cross-build success statistics: the only two boots that ever
    // survived STARTCPU ran builds with the tight leaf-W1C and WITHOUT these
    // reads (359cef1e r13, 0058b6f4 full r14); the commit that added them
    // (d6ddc853) and everything after -- including full Linux byte-parity
    // with NO bracket at all -- wedged 100%. Empirically, touching the
    // display PRI cluster microseconds before the SEC2 HS resume correlates
    // with the wedge, and the leaf W1C right before the store correlates
    // with survival. This restores the exact 0058b6f4 recipe.

    // (2) EXP3 -- UNCONDITIONAL write-1-to-clear drain. The EXP2 run showed
    // CPU_INTR_TOP=0 (the top register only reflects ENABLED leaves) yet
    // LEAF[4] bit28 (0x10000000) was pending with en=0 -- a MASKED-but-
    // asserted source, mirrored in the legacy PMC_INTR0 bit28. The old loop
    // broke on TOP==0 and never touched that leaf. Now: clear every nonzero
    // leaf regardless of TOP (a masked pending bit is still latched there),
    // then re-read each to classify the source -- a bit that STAYS cleared was
    // latched/edge (W1C suffices, mimics servicing); a bit that RE-ASSERTS is
    // a live level source that must be cleared at the engine, not the leaf.
    let mut cleared_bits = 0u32;
    for i in 0..8usize {
        let leaf = rd(LEAF + i * 4);
        if leaf != 0 {
            wr(LEAF + i * 4, leaf); // write-1-to-clear (unconditional)
            cleared_bits = cleared_bits.wrapping_add(leaf.count_ones());
        }
    }
    // EXP5 -- retire the PBUS PRI-error latch AT THE UNIT, i.e. do what
    // EXP3's own verdict says ("clear at engine"). The pre-boot quench
    // (pbus_pri_diagnose_and_clear) demonstrably gets undone by the boot
    // itself: the r15 wedge photo shows LEAF[4] bit28 pending again at this
    // bracket on a build whose pre-boot clear ran minutes earlier -- one of
    // the sequencer's own PRI accesses re-latches it. A leaf W1C can never
    // retire a level line that follows the unit latch, so quench the unit
    // here, at the last instant before the STARTCPU store: nouveau's
    // documented sequence (SAVE_0(0x9084)=0, then W1C the bits into
    // NV_PBUS_INTR_0(0x1100)). PBUS registers only -- none of the
    // empirically-fatal display/priv-ring cluster reads.
    let pbus_intr = rd(0x1100);
    if pbus_intr != 0 && (pbus_intr & 0xFFFF_0000) != 0xBADF_0000 {
        let save0 = rd(0x9084);
        crate::os_interface::probe_line(&alloc::format!(
            "[nvidia-rm] EXP5 PBUS at bracket: INTR_0={:#010x} SAVE_0={:#010x} -- unit retire",
            pbus_intr,
            save0
        ));
        wr(0x9084, 0);
        wr(0x1100, pbus_intr);
        // The leaf may still hold the (now-orphaned) latched bit; W1C it
        // once more now that the level line should have dropped.
        let leaf4 = rd(LEAF + 4 * 4);
        if leaf4 != 0 {
            wr(LEAF + 4 * 4, leaf4);
        }
        crate::os_interface::probe_line(&alloc::format!(
            "[nvidia-rm] EXP5 after unit retire: PBUS_INTR_0={:#010x} LEAF[4]={:#010x} PMC_INTR0={:#010x}",
            rd(0x1100),
            rd(LEAF + 4 * 4),
            rd(0x100)
        ));
    }
    // Re-read pass: which leaves re-asserted (level source) vs stayed clear?
    let mut still_pending = 0u32;
    for i in 0..8usize {
        let after = rd(LEAF + i * 4);
        if after != 0 {
            still_pending = still_pending.wrapping_add(1);
            crate::os_interface::probe_line(&alloc::format!(
                "[nvidia-rm] EXP3 LEAF[{}] STILL pending after W1C = {:#010x} (LIVE LEVEL source -- clear at engine)",
                i, after
            ));
        }
    }
    let top_final = rd(TOP);
    let pmc0_final = rd(0x100);
    crate::os_interface::probe_line(&alloc::format!(
        "[nvidia-rm] EXP3 drain done: bits_cleared={} leaves_still_pending={} CPU_INTR_TOP_final={:#010x} PMC_INTR0_final={:#010x} ({})",
        cleared_bits,
        still_pending,
        top_final,
        pmc0_final,
        if still_pending == 0 { "leaves latched -- W1C stuck" } else { "re-asserting level source" }
    ));
}

/// EXP 1 (PDISP hold-in-reset): when armed, the first completed read of
/// NV_PDISP_VGA_WORKSPACE_BASE (0x625F04 -- kgspCalculateFbLayout's read,
/// i.e. "after the FB-layout code consumed the register, before Booter/
/// bootstrap") triggers clearing NV_PMC_ENABLE bit 30 (PDISP,
/// tu102/dev_boot.h:116) with the same clear+2-readback propagation
/// sequence kbifDoFullChipReset_GM107 uses. Engine-level reset = zero
/// isochronous display FB fetch during the SEC2-RTOS resume. One-way for
/// the boot: the console goes dark (scanout stops) until reboot. Armed
/// only by /proc/gpustep12.
static PDISP_KILL_ARMED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

// EXP1c: PDISP is held in reset only for the SEC2 HS-resume window (which
// wedges the bus on live scanout), then RESTORED once the RM narrates
// "RISCV started" -- past the wedge, GSP-RM about to run its own init, which
// timed out in EXP1/EXP1b because it touches the display engine we'd left in
// reset. Restoring here mirrors Linux (scanout live throughout GSP-RM run).
static PDISP_SAVED_BASE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static PDISP_SAVED_PMC: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static PDISP_RESTORE_PENDING: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Arm the PDISP hold-in-reset experiment for the next GSP boot.
pub fn pdisp_kill_arm() {
    PDISP_KILL_ARMED.store(true, core::sync::atomic::Ordering::Relaxed);
}

/// Disarm the PDISP experiment (boot returned or never triggered).
pub fn pdisp_kill_disarm() {
    PDISP_KILL_ARMED.store(false, core::sync::atomic::Ordering::Relaxed);
    PDISP_RESTORE_PENDING.store(false, core::sync::atomic::Ordering::Relaxed);
}

/// Restore PDISP (re-set NV_PMC_ENABLE bit 30) if the kill experiment armed
/// it. Called from the RM narration hook the moment "RISCV started" is seen
/// -- the SEC2 HS-resume window is over, so scanout can safely come back and
/// GSP-RM finds the display engine alive for its own init.
pub fn pdisp_restore() {
    use core::sync::atomic::Ordering;
    if !PDISP_RESTORE_PENDING.swap(false, Ordering::Relaxed) {
        return;
    }
    let base = PDISP_SAVED_BASE.load(Ordering::Relaxed);
    let pmc = PDISP_SAVED_PMC.load(Ordering::Relaxed);
    if base != 0 {
        unsafe {
            let p = (base + 0x200) as *mut NvU32;
            core::ptr::write_volatile(p, pmc);
            let _ = core::ptr::read_volatile(p);
            let _ = core::ptr::read_volatile(p);
        }
        seq_trace_emit(&alloc::format!(
            "[nvidia-rm] EXP1c: PDISP restored (PMC_ENABLE <- {:#010x}) after RISCV start -- past the wedge window, scanout back for GSP-RM init",
            pmc
        ));
    }
}

/// Switch the armed trace live. Called by os_interface's narration hook the
/// moment the RM logs receiving the GSP_RUN_CPU_SEQUENCER RPC, so the trace
/// covers the entire sequencer buffer execution, and (belt-and-braces) by
/// the SEC2 STARTCPU bracket below in case the narration line never matched.
pub fn seq_trace_go_live() {
    use core::sync::atomic::Ordering;
    if SEQ_TRACE_ARMED.load(Ordering::Relaxed) && !SEQ_TRACE_LIVE.swap(true, Ordering::Relaxed) {
        let line = "[nvidia-rm] SEQ trace LIVE (sequencer RPC received; every reg access narrates until boot returns)";
        if crate::os_interface::live_echo_on() {
            log::error!("{}", line);
        }
        crate::os_interface::capture_line(line);
    }
}

// Collapse consecutive repeats of the same (kind, offset) so a poll loop
// doesn't eat the cap; 0 = read, 1 = write, packed with the offset.
static SEQ_TRACE_LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(u64::MAX);
static SEQ_TRACE_LINES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
const SEQ_TRACE_CAP: u32 = 400;

/// Emit a trace line to both sinks: the /proc capture buffer (always -- so
/// the SUCCESSFUL secondary-GPU boot's full register sequence is readable at
/// leisure in /proc/gpustep6 and diffable against the console GPU's), and
/// the live screen at ERROR level (only when live-echo is armed, i.e. the
/// console-GPU step-11 boot -- where a wedge makes the last line the
/// diagnosis).
fn seq_trace_emit(line: &str) {
    if crate::os_interface::live_echo_on() {
        log::error!("{}", line);
    }
    crate::os_interface::capture_line(line);
}

/// Pre-log a traced register access. Returns true if a line was emitted
/// (first access of a run, under cap) so the read path can pair it with a
/// completion line carrying the value.
#[inline]
fn seq_trace(kind_write: bool, offset: NvU32, value: Option<NvU32>) -> bool {
    use core::sync::atomic::Ordering;
    if !SEQ_TRACE_LIVE.load(Ordering::Relaxed) {
        return false;
    }
    let key = ((kind_write as u64) << 32) | offset as u64;
    if SEQ_TRACE_LAST.swap(key, Ordering::Relaxed) == key {
        return false; // consecutive repeat (poll loop) -- already narrated once
    }
    if SEQ_TRACE_LINES.fetch_add(1, Ordering::Relaxed) >= SEQ_TRACE_CAP {
        return false;
    }
    if kind_write {
        seq_trace_emit(&alloc::format!(
            "[nvidia-rm] SEQ WR off={:#x} <= {:#x} (about to)",
            offset,
            value.unwrap_or(0)
        ));
    } else {
        seq_trace_emit(&alloc::format!(
            "[nvidia-rm] SEQ RD off={:#x} (about to)",
            offset
        ));
    }
    true
}

#[no_mangle]
pub extern "C" fn osDevReadReg008(
    _pGpu: *mut c_void,
    pMapping: *mut c_void,
    this_address: NvU32,
) -> NvU8 {
    if WEDGE_FAKE_MMIO.load(core::sync::atomic::Ordering::Relaxed) {
        return 0xFF;
    }
    match dev_mapping_base(pMapping) {
        Some(base) => unsafe { core::ptr::read_volatile(base.add(this_address as usize)) },
        None => 0xFF,
    }
}

#[no_mangle]
pub extern "C" fn osDevReadReg016(
    _pGpu: *mut c_void,
    pMapping: *mut c_void,
    this_address: NvU32,
) -> NvU16 {
    if WEDGE_FAKE_MMIO.load(core::sync::atomic::Ordering::Relaxed) {
        return 0xFFFF;
    }
    match dev_mapping_base(pMapping) {
        Some(base) => unsafe {
            core::ptr::read_volatile(base.add(this_address as usize) as *const NvU16)
        },
        None => 0xFFFF,
    }
}

#[no_mangle]
pub extern "C" fn osDevReadReg032(
    _pGpu: *mut c_void,
    pMapping: *mut c_void,
    this_address: NvU32,
) -> NvU32 {
    // Wedge containment: with the fabric confirmed dead, answer all-ones
    // WITHOUT touching hardware so the RM's polls fail gracefully instead of
    // hanging the CPU (see WEDGE_FAKE_MMIO).
    if WEDGE_FAKE_MMIO.load(core::sync::atomic::Ordering::Relaxed) {
        return 0xFFFF_FFFF;
    }
    // SEC2-aperture probe (0x840000..0x844000). The box hard-freezes in
    // kflcnStartCpu_TU102(SEC2) at CORE_RESUME reading FALCON_CPUCTL (0x840100)
    // -- the first BAR0 access above ~7 MiB. Log the offset synchronously
    // BEFORE the volatile deref (so if the read itself stalls the last line on
    // the monitor is "about to deref"), and the value AFTER (so if the read
    // returns and the hang is the following write, we see the value first).
    // Only reads are probed -- the ucode-load flood is writes -- and it is
    // capped so a runaway can't spam the console.
    // SEC2 aperture (0x840000..0x844000) OR the PGC6/BSI scratch island
    // (0x118000..0x118200, which holds BSI_SECURE_SCRATCH_14 @ 0x1180f8 --
    // bit 26 is the SEC2 GSP-RM BOOT_STAGE_3_HANDOFF DONE flag polled by
    // _kgspIsReloadCompleted). Logging the BSI read shows whether the reload
    // wait exits immediately on a garbage 0xffffffff (bit 26 set) read --
    // which would explain reading SEC2 MAILBOX0 while SEC2 is still executing
    // and hanging.
    //
    // GSP-falcon aperture (0x110000..0x112000) probing is gated on
    // GSP_PROBE_ON (set when SEC2 is started for the GSP-RM resume): the
    // machine now freezes ~60 ms AFTER "GSP FW RM ready.", and the first
    // post-ready MMIO is the RPC doorbell write to the GSP falcon
    // (SET_GUEST_SYSTEM_INFO -> GspMsgQueueSendCommand -> queue-head write).
    // Gating avoids burning the line cap on the hundreds of GSP-falcon
    // accesses the earlier bootstrap performs.
    // GSP-falcon (GSPF) probe logging RETIRED: the one register it kept
    // catching post-boot is 0x110c00 = NV_PGSP_QUEUE_HEAD(0), the normal
    // per-RPC doorbell (kgspSetCmdQueueHead writes it 0 after every
    // GspMsgQueueSendCommand). During step-9 gpuStateLoad that floods the
    // console with hundreds of identical "GSPF ... 0x110c00" lines and buries
    // the eng_state transition trace that names the crashing engine. The SEC2
    // boot-hang this probe was added for is solved, so drop GSPF entirely and
    // keep only the SEC2/BSI apertures (boot-only, capped, don't fire here).
    let is_sec2 = (0x0084_0000..0x0084_4000).contains(&this_address);
    let is_bsi = (0x0011_8000..0x0011_8200).contains(&this_address);
    if is_sec2 || is_bsi {
        use core::sync::atomic::{AtomicU32, Ordering};
        static PROBE_RD_LOGS: AtomicU32 = AtomicU32::new(0);
        if PROBE_RD_LOGS.fetch_add(1, Ordering::Relaxed) < 160 {
            let tag = if is_bsi { "BSI" } else { "SEC2" };
            log::debug!(
                "[nvidia-rm] {} RD off={:#x} (about to deref)",
                tag,
                this_address
            );
        }
    }
    // Step-11 SEC2-resume trace: pre-log EVERY read live (ERROR level)
    // before the volatile access -- the read that never completes leaves
    // its "(about to)" line as the last thing on screen.
    let seq_logged = seq_trace(false, this_address, None);
    let v = match dev_mapping_base(pMapping) {
        Some(base) => unsafe {
            core::ptr::read_volatile(base.add(this_address as usize) as *const NvU32)
        },
        None => 0xFFFF_FFFF,
    };
    if seq_logged {
        seq_trace_emit(&alloc::format!(
            "[nvidia-rm] SEQ RD off={:#x} = {:#x}",
            this_address,
            v
        ));
    }
    // EXP 1 trigger: kgspCalculateFbLayout has just consumed
    // NV_PDISP_VGA_WORKSPACE_BASE -- from here to bootstrap nothing else
    // needs the display engine, so hold PDISP in engine-level reset
    // (NV_PMC_ENABLE bit 30 clear + two propagation readbacks, the
    // kbifDoFullChipReset_GM107 sequence). Scanout DMA stops dead; the
    // console goes dark until reboot. Only /proc/gpustep12 arms this.
    if this_address == 0x0062_5F04
        && PDISP_KILL_ARMED.swap(false, core::sync::atomic::Ordering::Relaxed)
    {
        if let Some(base) = dev_mapping_base(pMapping) {
            unsafe {
                let pmc = base.add(0x200) as *mut NvU32;
                let old = core::ptr::read_volatile(pmc);
                core::ptr::write_volatile(pmc, old & !(1u32 << 30));
                let rb1 = core::ptr::read_volatile(pmc);
                let rb2 = core::ptr::read_volatile(pmc);
                // Stash the original PMC_ENABLE and this GPU's BAR0 base so
                // pdisp_restore() (fired from the "RISCV started" narration
                // marker, i.e. AFTER the SEC2 HS resume window that wedges on
                // live scanout) can bring PDISP back before GSP-RM needs it.
                PDISP_SAVED_BASE.store(base as usize, core::sync::atomic::Ordering::Relaxed);
                PDISP_SAVED_PMC.store(old, core::sync::atomic::Ordering::Relaxed);
                PDISP_RESTORE_PENDING.store(true, core::sync::atomic::Ordering::Relaxed);
                seq_trace_emit(&alloc::format!(
                    "[nvidia-rm] EXP1: PDISP held in reset (PMC_ENABLE {:#010x} -> {:#010x}, readbacks {:#010x}/{:#010x}); screen goes dark now, boot continues",
                    old,
                    old & !(1u32 << 30),
                    rb1,
                    rb2
                ));
            }
        }
    }
    if is_sec2 || is_bsi {
        use core::sync::atomic::{AtomicU32, Ordering};
        static PROBE_RD_DONE: AtomicU32 = AtomicU32::new(0);
        if PROBE_RD_DONE.fetch_add(1, Ordering::Relaxed) < 160 {
            let tag = if is_bsi { "BSI" } else { "SEC2" };
            log::debug!("[nvidia-rm] {} RD off={:#x} = {:#x}", tag, this_address, v);
        }
    }
    v
}

#[no_mangle]
pub extern "C" fn osDevWriteReg008(
    _pGpu: *mut c_void,
    pMapping: *mut c_void,
    this_address: NvU32,
    this_value: NvU8,
) {
    if WEDGE_FAKE_MMIO.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    if let Some(base) = dev_mapping_base(pMapping) {
        unsafe { core::ptr::write_volatile(base.add(this_address as usize), this_value) };
    }
}

#[no_mangle]
pub extern "C" fn osDevWriteReg016(
    _pGpu: *mut c_void,
    pMapping: *mut c_void,
    this_address: NvU32,
    this_value: NvU16,
) {
    if WEDGE_FAKE_MMIO.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    if let Some(base) = dev_mapping_base(pMapping) {
        unsafe {
            core::ptr::write_volatile(base.add(this_address as usize) as *mut NvU16, this_value)
        };
    }
}

#[no_mangle]
pub extern "C" fn osDevWriteReg032(
    _pGpu: *mut c_void,
    pMapping: *mut c_void,
    this_address: NvU32,
    this_value: NvU32,
) {
    if WEDGE_FAKE_MMIO.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    // SEC2-aperture write probe (companion to the read probe above). SEC2 reads
    // return real values, so the CORE_RESUME freeze is at/after kflcnStartCpu's
    // STARTCPU write (which the read-only probe couldn't show). Log SEC2 writes
    // to the FALCON *control* registers only -- CPUCTL (0x100), BOOTVEC (0x104),
    // CPUCTL_ALIAS (0x130) all sit below 0x180, while the IMEMC/IMEMD (0x180+)
    // and DMEMC/DMEMD (0x1c0+) ucode-load windows must be excluded or the
    // hundreds of ucode DWORD writes flood the console and exhaust the cap
    // before CORE_RESUME. STARTCPU goes to CPUCTL_ALIAS (0x130), so this shows
    // it and nothing noisy.
    // GSPF write logging retired for the same reason as the read probe above
    // (0x110c00 = NV_PGSP_QUEUE_HEAD RPC doorbell floods step-9 postLoad).
    // Keep only the SEC2 falcon *control* registers (boot-only, capped).
    let is_sec2 = (0x0084_0000..0x0084_4000).contains(&this_address);
    if is_sec2 && (this_address & 0xffff) < 0x0180 {
        use core::sync::atomic::{AtomicU32, Ordering};
        static SEC2_WR_LOGS: AtomicU32 = AtomicU32::new(0);
        if SEC2_WR_LOGS.fetch_add(1, Ordering::Relaxed) < 200 {
            // Logged BEFORE the store: if the posted write itself wedges the
            // CPU, the last line on screen names the exact register.
            log::debug!(
                "[nvidia-rm] SEC2 WR off={:#x} <= {:#x}",
                this_address,
                this_value
            );
        }
    }
    // Step-11 SEC2-resume trace (see seq_trace): pre-log every write live.
    let _ = seq_trace(true, this_address, Some(this_value));
    // STARTCPU bracket. Observed on real hardware: the last line that ever
    // renders is this probe's own "WR off=0x840130 <= 0x2" -- the BSI read
    // probe's PRE-log line never appears, even though it prints before its
    // deref. The console framebuffer lives in this same GPU's BAR1, so the
    // freeze is the first CPU access to the GPU (BAR0 read or BAR1 pixel
    // write) after SEC2 launches: SEC2's secure GSP-RM reload blocks the
    // GPU's apertures, and any access during that window stalls the CPU with
    // IRQs off. So after the STARTCPU store, go completely silent -- no MMIO,
    // no console (console IS MMIO here) -- for 500 ms (TSC-based delay only),
    // then read the BSI handoff register RAW (no pre-log) and only log once
    // it has returned. If the window theory is right this also simply works:
    // the RM's own reload poll then sees DONE on its first iteration.
    let is_sec2_startcpu = this_address == 0x0084_0130;
    let base_for_bracket = dev_mapping_base(pMapping);
    // Set when the STARTCPU store is posted inside the drain path below (tight
    // W1C-then-store), so the generic write at the end is skipped (no double-post).
    let mut startcpu_posted = false;
    // Linux byte-parity mode: the bracket contributes ZERO extra MMIO and zero
    // logging -- the store proceeds through the generic write below, exactly
    // like kflcnStartCpu_TU102 on Linux (read CPUCTL, write CPUCTL_ALIAS,
    // nothing between). See LINUX_PARITY's doc for the falsification trail.
    let parity = LINUX_PARITY.load(core::sync::atomic::Ordering::Relaxed);
    if is_sec2_startcpu && !parity {
        GSP_PROBE_ON.store(true, core::sync::atomic::Ordering::Relaxed);
        // Belt-and-braces trace trigger: normally the trace went live already
        // when the RUN_CPU_SEQUENCER narration line was seen (covering the
        // whole sequencer buffer); if that match ever fails, going live at
        // STARTCPU still covers the resume window.
        seq_trace_go_live();
        if let Some(base) = base_for_bracket {
            let bsi_before =
                unsafe { core::ptr::read_volatile(base.add(0x0011_80f8) as *const NvU32) };
            log::debug!(
                "[nvidia-rm] STARTCPU(SEC2): BSI_SECURE_SCRATCH_14 before = {:#x}",
                bsi_before
            );
        }
        // EXP2 (pre-STARTCPU interrupt drain): when armed, snapshot + drain the
        // CPU-facing top-level interrupt tree BEFORE letting the posted store
        // through -- the pseudo-ISR service window that a fully-polled/INTx-
        // masked driver otherwise lacks. See sec2_intr_snapshot_and_drain.
        if SEC2_DRAIN_ARMED.load(core::sync::atomic::Ordering::Relaxed) {
            if let Some(base) = base_for_bracket {
                // Diagnostic snapshot + classification (does its own logging).
                unsafe { sec2_intr_snapshot_and_drain(base) };
                // FINAL tight de-assert, then the STARTCPU store, with NOTHING
                // between them. The console display underflow is a live LEVEL
                // source that re-asserts within microseconds (EXP3 confirmed it
                // survives a W1C), and its fabric backpressure is what stalls
                // the posted STARTCPU write. The "drain done" log above takes
                // milliseconds to render -- long enough for the interrupt to
                // come back and re-stall the store (the flaky wedge). So here,
                // as the immediately-preceding operations, W1C every pending
                // leaf and then issue the posted store on the very next
                // instruction: the interrupt line stays de-asserted across the
                // write so its flow-control credits drain and STARTCPU
                // completes. Bypass the generic write below (would double-post).
                unsafe {
                    // Clear leaves 0..7 EXCEPT leaf 4 first...
                    for i in 0..8usize {
                        if i == 4 {
                            continue;
                        }
                        let off = 0x00B8_1000 + i * 4; // CPU_INTR_LEAF(i)
                        let leaf = core::ptr::read_volatile(base.add(off) as *const NvU32);
                        if leaf != 0 {
                            core::ptr::write_volatile(base.add(off) as *mut NvU32, leaf);
                        }
                    }
                    // ...then W1C leaf 4 (the known live-level source) as the
                    // last pre-op before the store.
                    let l4_off = 0x00B8_1000 + 4 * 4;
                    let l4 = core::ptr::read_volatile(base.add(l4_off) as *const NvU32);
                    if l4 != 0 {
                        core::ptr::write_volatile(base.add(l4_off) as *mut NvU32, l4);
                    }
                    // SYS-domain settle fence (Copilot's cross-cluster
                    // forward-progress model): a NON-POSTED read of the same
                    // leaf register forces the posted W1C to complete inside
                    // the GPU and the level line to settle BEFORE the
                    // STARTCPU store issues. The proven 0058b6f4 recipe fired
                    // the store nanoseconds after the posted W1C -- the
                    // deassert may not have propagated, which would explain
                    // the 2-of-3 (rather than deterministic) survival rate.
                    // Same register, SYS/VF interrupt fabric: honors the
                    // hard no-DISP/no-PBUS/no-PPRIV critical-window rule.
                    let _settle = core::ptr::read_volatile(base.add(l4_off) as *const NvU32);
                    // Survival breadcrumb: record "about to STARTCPU" to CMOS
                    // (GPU-independent) immediately before the posted store, so a
                    // wedge here is legible on the next boot via /proc/gpusurvive.
                    crate::survival::checkpoint(crate::survival::milestone::STARTCPU_PRE);
                    // Last-visible-line MSI status: whether the GPU's MSI
                    // delivery is online for this window and how many fired, so a
                    // wedge's frozen screen answers "did MSI even come online?".
                    let (mv, mc) = crate::survival::msi_status();
                    crate::os_interface::probe_line(&alloc::format!(
                        "[nvidia-rm] STARTCPU: MSI online={} vector={} count={} (about to post store)",
                        mv != usize::MAX,
                        mv,
                        mc
                    ));
                    core::ptr::write_volatile(
                        base.add(this_address as usize) as *mut NvU32,
                        this_value,
                    );
                    crate::survival::checkpoint(crate::survival::milestone::STARTCPU_POST);
                }
                // EXP1d: CPU-side re-anchor of the EXP1c PDISP restore. The
                // old trigger ("RISCV started" narration) NEVER fired -- that
                // NV_PRINTF is LEVEL_INFO and info is filtered before the
                // narration hook -- so PDISP stayed in reset through all of
                // GSP-RM's init: its display StateInit failed, display
                // classes were pruned, and NV04_DISPLAY_COMMON allocs
                // returned 0x22 INVALID_CLASS (run r19). Restore here
                // instead: 15 ms after the STARTCPU store the SEC2 HS
                // CORE_RESUME payload (microseconds-scale) is over with
                // margin, while GSP-RM's libos hasn't even loaded the RM
                // image yet -- so its display init finds PDISP alive. The
                // 400 ms fabric watch below then doubles as containment if
                // resuming scanout at +15 ms re-triggers the hazard.
                crate::hooks::with_hooks((), |h| h.delay_us(15_000));
                pdisp_restore();
                crate::survival::checkpoint(crate::survival::milestone::PDISP_RESTORE);
                // Post-store SILENT fabric watch (wedge containment). ZERO
                // MMIO and ZERO logging here -- if the fabric wedged, any
                // BAR0/BAR1 access (a log line renders into this GPU's BAR1
                // framebuffer!) hangs the CPU for good. Config cycles are
                // root-complex-completed and return all-ones on a dead
                // endpoint instead of hanging, so poll the GPU's config
                // VendorID/DeviceID: ~50 consecutive dead milliseconds =>
                // fabric wedged => flip fake-MMIO + suppress all console
                // rendering; the RM's own BSI poll then times out gracefully
                // and the machine SURVIVES with the full report in /proc.
                // On a healthy boot this adds a bounded pre-poll delay only
                // (the RM's reload poll then sees DONE, as every successful
                // run did).
                let cfg_h = WEDGE_CFG_HANDLE.load(core::sync::atomic::Ordering::Relaxed);
                if cfg_h != 0 {
                    let mut dead_run = 0u32;
                    for _ in 0..400u32 {
                        crate::hooks::with_hooks((), |h| h.delay_us(1_000));
                        let id = crate::hooks::with_hooks(0xFFFF_FFFFu32, |h| {
                            h.pci_config_read(cfg_h, 0, 4)
                        });
                        if id == 0xFFFF_FFFF {
                            dead_run += 1;
                            if dead_run >= 50 {
                                break;
                            }
                        } else {
                            dead_run = 0;
                        }
                    }
                    if dead_run >= 50 {
                        WEDGE_FAKE_MMIO.store(true, core::sync::atomic::Ordering::Relaxed);
                        WEDGE_DETECTED.store(true, core::sync::atomic::Ordering::Relaxed);
                        // The console framebuffer lives in the dead GPU's
                        // BAR1: suppress ALL rendering or the next log line
                        // kills the machine. Capture keeps recording for the
                        // /proc report.
                        crate::os_interface::console_quiet_begin();
                    }
                }
                startcpu_posted = true;
            }
        }
    }
    if !startcpu_posted {
        if let Some(base) = dev_mapping_base(pMapping) {
            // Breadcrumb only for the STARTCPU store (never per-register — a CMOS
            // write per MMIO would be ruinous); the non-drain path (Linux-parity
            // etc.) still hits the same wedge point.
            if is_sec2_startcpu {
                crate::survival::checkpoint(crate::survival::milestone::STARTCPU_PRE);
                let (mv, mc) = crate::survival::msi_status();
                crate::os_interface::probe_line(&alloc::format!(
                    "[nvidia-rm] STARTCPU: MSI online={} vector={} count={} (about to post store)",
                    mv != usize::MAX,
                    mv,
                    mc
                ));
            }
            unsafe {
                core::ptr::write_volatile(base.add(this_address as usize) as *mut NvU32, this_value)
            };
            if is_sec2_startcpu {
                crate::survival::checkpoint(crate::survival::milestone::STARTCPU_POST);
            }
        }
        // EXP1d for the non-drain STARTCPU paths (parity mode etc.): same
        // CPU-side restore anchor; pdisp_restore() is a no-op unless the
        // EXP1 kill armed it (PDISP_RESTORE_PENDING swap).
        if is_sec2_startcpu {
            crate::hooks::with_hooks((), |h| h.delay_us(15_000));
            pdisp_restore();
        }
    }
    // The old post-STARTCPU "500ms silent window + raw BSI readback" bracket
    // is GONE: it was an Eclipse-only divergence from the real driver (which
    // starts polling _kgspIsReloadCompleted immediately), it delayed the
    // resume handling, and both wedge rounds proved silence changes nothing.
    // The per-access seq_trace above now narrates the RM's own immediate
    // BSI polling instead -- higher fidelity AND better diagnostics.
    //
    // EXP 2 (post-STARTCPU bus autopsy): when armed, spend ~2s after the
    // STARTCPU write reading ONLY config space (root-complex-completed, so a
    // dead endpoint yields all-ones instead of a hang), logging transitions;
    // then attempt exactly one BAR0 read. Every outcome classifies the
    // failure physics -- see autopsy_arm's doc comment.
    if is_sec2_startcpu {
        let gpu_h = AUTOPSY_GPU_HANDLE.load(core::sync::atomic::Ordering::Relaxed);
        if gpu_h != 0 {
            let rp_h = AUTOPSY_RP_HANDLE.load(core::sync::atomic::Ordering::Relaxed);
            let mut last_gpu_id = 0xDEAD_BEEFu32;
            let mut last_gpu_cs = 0xDEAD_BEEFu32;
            let mut last_rp_id = 0xDEAD_BEEFu32;
            log::error!(
                "[nvidia-rm] AUTOPSY: config-space watch begins (2s @ 1ms; transitions only)"
            );
            for ms in 0..2000u32 {
                crate::hooks::with_hooks((), |h| h.delay_us(1_000));
                let gpu_id =
                    crate::hooks::with_hooks(0xFFFF_FFFF, |h| h.pci_config_read(gpu_h, 0, 4));
                let gpu_cs =
                    crate::hooks::with_hooks(0xFFFF_FFFF, |h| h.pci_config_read(gpu_h, 4, 4));
                let rp_id = if rp_h != 0 {
                    crate::hooks::with_hooks(0xFFFF_FFFF, |h| h.pci_config_read(rp_h, 0, 4))
                } else {
                    0
                };
                if gpu_id != last_gpu_id || gpu_cs != last_gpu_cs || rp_id != last_rp_id {
                    log::error!(
                        "[nvidia-rm] AUTOPSY t={}ms: GPU id={:#010x} cmd/status={:#010x} RP id={:#010x}",
                        ms, gpu_id, gpu_cs, rp_id
                    );
                    last_gpu_id = gpu_id;
                    last_gpu_cs = gpu_cs;
                    last_rp_id = rp_id;
                }
            }
            log::error!(
                "[nvidia-rm] AUTOPSY t=2000ms final: GPU id={:#010x} cmd/status={:#010x} RP id={:#010x}; now ONE BAR0 read of 0x1180f8 (about to)",
                last_gpu_id, last_gpu_cs, last_rp_id
            );
            if let Some(base) = base_for_bracket {
                let bsi =
                    unsafe { core::ptr::read_volatile(base.add(0x0011_80F8) as *const NvU32) };
                log::error!(
                    "[nvidia-rm] AUTOPSY: BAR0 read RETURNED = {:#010x} (bit26 DONE={}) -- bus alive",
                    bsi,
                    (bsi >> 26) & 1
                );
            }
        }
    }
}

/// Extracts `DEVICE_MAPPING::gpuNvAddr` (the first field) without needing
/// the rest of the struct's layout. Returns `None` for a null mapping or a
/// null/not-yet-mapped `gpuNvAddr`.
fn dev_mapping_base(p_mapping: *mut c_void) -> Option<*mut u8> {
    if p_mapping.is_null() {
        return None;
    }
    let base = unsafe { *(p_mapping as *const *mut u8) };
    if base.is_null() {
        None
    } else {
        Some(base)
    }
}

#[no_mangle]
pub extern "C" fn osDisableConsoleManagement(pGpu: *mut c_void) {
    let _ = pGpu;
}

#[no_mangle]
pub extern "C" fn osDisableInterrupts(pGpu: *mut c_void, bIsr: NvBool) {
    let _ = pGpu;
    let _ = bIsr;
}

#[no_mangle]
pub extern "C" fn osDmaSetAddressSize(pArg1: *mut c_void, bits: NvU32) {
    let _ = pArg1;
    let _ = bits;
}

#[no_mangle]
pub extern "C" fn osDmabufIsSupported() -> NvBool {
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osDpcAttachGpu(arg0: *mut c_void, arg1: *mut c_void) -> NV_STATUS {
    // Real osDpcAttachGpu (osinit.c:325) is literally
    // `return NV_OK; // Nothing to do for unix`. gpumgrAttachGpu checks
    // its result too, so the old NOT_SUPPORTED stub would fail attach.
    let _ = arg0;
    let _ = arg1;
    NV_OK
}

#[no_mangle]
pub extern "C" fn osDpcDetachGpu(arg0: *mut c_void) {
    let _ = arg0;
}

#[no_mangle]
pub extern "C" fn osEnableInterrupts(pGpu: *mut c_void) {
    let _ = pGpu;
}

#[no_mangle]
pub extern "C" fn osErrorLogV(
    pGpu: *mut c_void,
    context: *mut c_void,
    pFormat: *mut c_char,
    arglist: *mut c_void,
) -> *mut c_void {
    let _ = pGpu;
    let _ = context;
    let _ = pFormat;
    let _ = arglist;
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn osEventNotification(
    arg0: *mut c_void,
    arg1: *mut c_void,
    arg2: NvU32,
    arg3: *mut c_void,
    arg4: NvU32,
) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    let _ = arg3;
    let _ = arg4;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osEventNotificationWithInfo(
    arg0: *mut c_void,
    arg1: *mut c_void,
    arg2: NvU32,
    arg3: NvU32,
    arg4: NvU16,
    arg5: *mut c_void,
    arg6: NvU32,
) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    let _ = arg3;
    let _ = arg4;
    let _ = arg5;
    let _ = arg6;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osFindNsPid(pOsPidInfo: *mut c_void, pNsPid: *mut NvU32) -> NV_STATUS {
    let _ = pOsPidInfo;
    let _ = pNsPid;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osFlushCpuCache() -> NV_STATUS {
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osFlushCpuWriteCombineBuffer() {}

#[no_mangle]
pub extern "C" fn osFlushGpuCoherentCpuCacheRange(
    pOsGpuInfo: *mut c_void,
    cpuVirtual: NvU64,
    size: NvU64,
) {
    let _ = pOsGpuInfo;
    let _ = cpuVirtual;
    let _ = size;
}

// osFreePagesInternal: real impl in vendor/eclipse_rm_mem.c.

#[no_mangle]
pub extern "C" fn osFreeWaitQueue(pWq: *mut c_void) {
    let _ = pWq;
}

#[no_mangle]
pub extern "C" fn osGC6PowerControl(arg0: *mut c_void, arg1: NvU32, arg2: *mut NvU32) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osGetAcpiRsdpFromUefi(pRsdpAddr: *mut NvU32) -> NV_STATUS {
    let _ = pRsdpAddr;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osGetCpuCount() -> NvU32 {
    0
}

#[no_mangle]
pub extern "C" fn osGetCpuFrequency() -> NvU32 {
    0
}

#[no_mangle]
pub extern "C" fn osGetCurrentProcess() -> NvU32 {
    0
}

#[no_mangle]
pub extern "C" fn osGetCurrentProcessName(arg0: *mut c_char, arg1: NvU32) {
    let _ = arg0;
    let _ = arg1;
}

#[no_mangle]
pub extern "C" fn osGetCurrentUidToken() -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn osGetDriverBlock(arg0: *mut c_void, arg1: *mut c_void) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osGetDynamicPowerSupportMask() -> NvU32 {
    0
}

#[no_mangle]
pub extern "C" fn osGetEgmInfo(
    pGpu: *mut c_void,
    pPhysAddr: *mut NvU64,
    pSize: *mut NvU64,
    pNodeId: *mut NvS32,
) -> NV_STATUS {
    let _ = pGpu;
    let _ = pPhysAddr;
    let _ = pSize;
    let _ = pNodeId;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osGetFbNumaInfo(
    pGpu: *mut c_void,
    pAddrPhys: *mut NvU64,
    pSizePhys: *mut NvU64,
    pAddrRsvdPhys: *mut NvU64,
    pNodeId: *mut NvS32,
) -> NV_STATUS {
    let _ = pGpu;
    let _ = pAddrPhys;
    let _ = pSizePhys;
    let _ = pAddrRsvdPhys;
    let _ = pNodeId;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osGetForcedNVLinkConnection(
    pGpu: *mut c_void,
    maxLinks: NvU32,
    pLinkConnection: *mut NvU32,
) -> NV_STATUS {
    let _ = pGpu;
    let _ = maxLinks;
    let _ = pLinkConnection;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osGetGridCspSupport() -> NvU32 {
    0
}

#[no_mangle]
pub extern "C" fn osGetMaxUserVa() -> NvU64 {
    0
}

#[no_mangle]
pub extern "C" fn osGetMemoryPages(
    pMemDesc: *mut c_void,
    pPages: *mut c_void,
    pNumPages: *mut NvU32,
) -> NV_STATUS {
    let _ = pMemDesc;
    let _ = pPages;
    let _ = pNumPages;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osGetNumMemoryPages(pMemDesc: *mut c_void, pNumPages: *mut NvU32) -> NV_STATUS {
    let _ = pMemDesc;
    let _ = pNumPages;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn NVBIT(
    numaId: *mut c_void,
    free_memory_bytes: *mut NvU64,
    total_memory_bytes: *mut NvU64,
) -> *mut c_void {
    let _ = numaId;
    let _ = free_memory_bytes;
    let _ = total_memory_bytes;
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn osGetNvlinkLinkCallbacks() -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn osGetPageRefcount(arg0: NvU64) -> NvU32 {
    let _ = arg0;
    0
}

#[no_mangle]
pub extern "C" fn osGetPageShift() -> NvU8 {
    0
}

#[no_mangle]
pub extern "C" fn osGetPageSize() -> NvU64 {
    // The host CPU page size, exactly as NVIDIA's Linux osGetPageSize
    // returns os_page_size (== PAGE_SIZE == 4096 on x86_64). Returning 0
    // here was a real bug: memdesc's _memdescCalculateAllocSize does
    // `allocSize = RM_ALIGN_UP(requestedSize, osGetPageSize())` for every
    // ADDR_SYSMEM allocation, and RM_ALIGN_UP(x, 0) collapses to 0, so the
    // subsequent `allocSize < requestedSize` check returned
    // NV_ERR_INVALID_ARGUMENT (0x1F) -- which is exactly what
    // kmemsysInitFlushSysmemBuffer_HAL's first sysmem memdescCreate hit on
    // real hardware (kern_mem_sys.c:98). Must match the os_page_size global
    // (os_interface.rs) RM also reads directly.
    crate::os_interface::os_page_size
}

#[no_mangle]
pub extern "C" fn osGetPerformanceCounter(arg0: *mut NvU64) -> NV_STATUS {
    let _ = arg0;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osGetPidInfo() -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn osGetPlatformNvlinkLinerate(
    pGpu: *mut c_void,
    lineRate: *mut NvU32,
) -> NV_STATUS {
    let _ = pGpu;
    let _ = lineRate;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osGetRandomBytes(pBytes: *mut NvU8, numBytes: NvU16) -> NV_STATUS {
    crate::os_interface::os_get_random_bytes(pBytes, numBytes)
}

#[no_mangle]
pub extern "C" fn osGetSecurityToken() -> *mut c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn osGetSupportedSysmemPageSizeMask() -> NvU64 {
    0
}

#[no_mangle]
pub extern "C" fn osGetSyncpointAperture(
    pOsGpuInfo: *mut c_void,
    syncpointId: NvU32,
    physAddr: *mut NvU64,
    limit: *mut NvU64,
    offset: *mut NvU32,
) -> NV_STATUS {
    let _ = pOsGpuInfo;
    let _ = syncpointId;
    let _ = physAddr;
    let _ = limit;
    let _ = offset;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osGetTimestamp() -> NvU64 {
    // Real semantics (arch/nvalloc/unix/src/os.c): a timestamp in the
    // unit whose frequency osGetTimestampFreq() reports -- Linux returns
    // microseconds with freq=1e6, and Eclipse mirrors that exactly. The
    // previous auto-generated stub returned 0 with a WRONG signature
    // (*mut c_void) -- same register ABI, but semantically garbage.
    crate::hooks::with_hooks(0u64, |h| h.monotonic_time_ns()) / 1_000
}

#[no_mangle]
pub extern "C" fn osGetTimestampFreq() -> NvU64 {
    // MUST NOT be 0: rcdbConstruct_IMPL (journal.c) computes
    // `(timeStamp * 1000000) / osGetTimestampFreq()` during OBJSYS
    // construction -- the previous 0 stub caused a kernel divide-by-zero
    // (#DE) that killed /proc/gpustep5 with no output on real hardware,
    // right after _sysCreateOs's benign "Sys Cap creation failed: 0x56"
    // line (OBJRCDB is constructed a few children later). Matches the
    // real Linux value for microsecond timestamps (os.c: return 1000000).
    1_000_000
}

#[no_mangle]
pub extern "C" fn osGetCurrentTick(pTimeInNs: *mut NvU64) -> NV_STATUS {
    // Real Linux (arch/nvalloc/unix/src/os.c): *pTimeInNs = os_get_current_tick().
    // Feeds the RM GPU-lock acquire timeout (locks.c _rmGpuLocksAcquire) and
    // gpu_timeout.c's OS-timer deadlines, so this MUST return a real,
    // monotonically-increasing nanosecond clock -- a constant would make lock
    // acquisition either time out instantly or never. The 570.144 RM
    // references this unconditionally (610 did not link it in), which is why
    // it only surfaced at link time on this re-vendor.
    if !pTimeInNs.is_null() {
        unsafe {
            *pTimeInNs = with_hooks(0u64, |h| h.monotonic_time_ns());
        }
    }
    NV_OK
}

#[no_mangle]
pub extern "C" fn osGetTickResolution() -> NvU64 {
    // Real Linux (os.c: os_get_tick_resolution). Used ONLY additively in
    // gpu_timeout.c timeoutSet() to pad a deadline out to the next tick, never
    // as a divisor, so any small non-zero value is safe. Mirror the Linux
    // fallback where os_get_current_tick has microsecond granularity:
    // NSEC_PER_USEC = 1000 ns.
    1_000
}

#[no_mangle]
pub extern "C" fn osGetCurrentTime(pSeconds: *mut NvU32, pMicroSeconds: *mut NvU32) -> NV_STATUS {
    // Real Linux (os.c: os_get_current_time) returns wall-clock seconds +
    // microseconds since the epoch. Eclipse has no RTC wired into the RM, so
    // derive both from the same monotonic nanosecond clock used everywhere
    // else here; callers use it for log/journal timestamps and elapsed-time
    // deltas (client_resource.c, rpc.c), for which a monotonic base is fine.
    let ns = with_hooks(0u64, |h| h.monotonic_time_ns());
    if !pSeconds.is_null() {
        unsafe {
            *pSeconds = (ns / 1_000_000_000) as NvU32;
        }
    }
    if !pMicroSeconds.is_null() {
        unsafe {
            *pMicroSeconds = ((ns / 1_000) % 1_000_000) as NvU32;
        }
    }
    NV_OK
}

#[no_mangle]
pub extern "C" fn osGetVersionDump(arg0: *mut c_void) -> NV_STATUS {
    let _ = arg0;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osGpuIsCxlDevice(pGpu: *mut c_void) -> NvBool {
    let _ = pGpu;
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn DPC_RELEASE_ALL_GPU_LOCKS(
    pGpu: *mut c_void,
    dpcGpuLockRelease: NvU32,
) -> *mut c_void {
    let _ = pGpu;
    let _ = dpcGpuLockRelease;
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn osGpuSupportsAts(pGpu: *mut c_void) -> NvBool {
    let _ = pGpu;
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osHandleDeferredRecovery(arg0: *mut c_void) {
    let _ = arg0;
}

#[no_mangle]
pub extern "C" fn osHandleGpuLost(arg0: *mut c_void, arg1: NvBool) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osImexChannelCount() -> NvS32 {
    -1
}

#[no_mangle]
pub extern "C" fn osImexChannelGet(descriptor: NvU64) -> NvS32 {
    let _ = descriptor;
    -1
}

#[no_mangle]
pub extern "C" fn osImexChannelIsSupported() -> NvBool {
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osInitMapping(pGpu: *mut c_void) -> NV_STATUS {
    let _ = pGpu;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osInitSystemStaticConfig(arg0: *mut c_void) {
    let _ = arg0;
}

#[no_mangle]
pub extern "C" fn osIovaMap(pIovaMapping: *mut c_void) -> NV_STATUS {
    let _ = pIovaMapping;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osIovaUnmap(pIovaMapping: *mut c_void) {
    let _ = pIovaMapping;
}

#[no_mangle]
pub extern "C" fn osIsAdministrator() -> NvBool {
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osIsGpuAccessible(pGpu: *mut c_void) -> NvBool {
    let _ = pGpu;
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osIsGpuShutdown(pGpu: *mut c_void) -> NvBool {
    let _ = pGpu;
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osIsGridSupported(arg0: *mut c_void) -> NvBool {
    let _ = arg0;
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osIsISR() -> NvBool {
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osIsInitNs() -> NvBool {
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osIsNvswitchPresent() -> NvBool {
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osIsRaisedIRQL() -> NvBool {
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osIsSwPreInitOnly(arg0: *mut c_void) -> NvBool {
    let _ = arg0;
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osIsVfioPciCorePresent() -> NV_STATUS {
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osIsVgpuDeviceVmPresent() -> NV_STATUS {
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osIsVgpuVfioPresent() -> NV_STATUS {
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osLockMem(arg0: *mut c_void) -> *mut c_void {
    let _ = arg0;
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn osLockShouldToggleInterrupts(arg0: *mut c_void) -> NvBool {
    let _ = arg0;
    NV_FALSE
}

// osMapGPU / osUnmapGPU: REAL implementations live in C
// (vendor/eclipse_rm_init.c), NOT here. The auto-stub this crate used to
// carry had the WRONG ABI (returned `*mut c_void` null = 0 = NV_OK while
// never writing the `NvP64 *pAddress` out-param), which made
// rmapiMapToCpu's ADDR_REGMEM path "succeed" with a NULL CPU pointer.
// memmgrScrubMapDoorbellRegion_GV100 then set
// pDoorbellRegisterOffset = NULL + NVC361_NOTIFY_CHANNEL_PENDING (0x90)
// and bUseDoorbellRegister = TRUE, and the first CE work submission
// (channelFillGpFifo) faulted writing vaddr 0x90 -- the exact step-9
// gpuStateLoad crash on real hardware. Same bug family as the
// osMapPciMemoryKernel64/Old fix below.

#[no_mangle]
pub extern "C" fn osMapPciMemoryAreaUser(
    arg0: *mut c_void,
    arg1: *mut c_void,
    arg2: NvU32,
    arg3: NvU32,
    arg4: *mut c_void,
    arg5: *mut c_void,
) -> *mut c_void {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    let _ = arg3;
    let _ = arg4;
    let _ = arg5;
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn osMapPciMemoryKernel64(
    _pGpu: *mut c_void,
    busAddress: NvU64,
    length: NvU64,
    _protect: NvU32,
    pVirtualAddress: *mut NvU64,
    modeFlag: NvU32,
) -> NV_STATUS {
    // Real semantics (arch/nvalloc/unix/src/os.c): NV_STATUS return, kernel
    // VA written through the NvP64 out-param, backed by os_map_kernel_space.
    // The previous auto-generated stub returned NULL with a void* return
    // type -- NULL reads as NV_OK while the out-param stayed garbage/NULL,
    // which is exactly how kbusInitVirtualBar2 died with pMapping == NULL /
    // 0x1A during gpuStateInit's KernelBus phase on real hardware: this is
    // the function that maps the BAR2 PCI aperture for CPU access.
    if pVirtualAddress.is_null() {
        return NV_ERR_GENERIC;
    }
    let va = crate::os_interface::os_map_kernel_space(busAddress, length, modeFlag);
    if va.is_null() {
        return NV_ERR_GENERIC;
    }
    unsafe { *pVirtualAddress = va as usize as NvU64 };
    NV_OK
}

#[no_mangle]
pub extern "C" fn osMapPciMemoryKernelOld(
    _pGpu: *mut c_void,
    busAddress: NvU64,
    length: NvU64,
    _protect: NvU32,
    pVirtualAddress: *mut *mut c_void,
    modeFlag: NvU32,
) -> NV_STATUS {
    // Same contract as osMapPciMemoryKernel64 (see above), void** flavor.
    // Linux additionally chains an nv_kern_mapping_t bookkeeping node for
    // driver teardown; Eclipse never tears the driver down, and unmap goes
    // straight to os_unmap_kernel_space, so no list is needed.
    if pVirtualAddress.is_null() {
        return NV_ERR_GENERIC;
    }
    let va = crate::os_interface::os_map_kernel_space(busAddress, length, modeFlag);
    if va.is_null() {
        return NV_ERR_GENERIC;
    }
    unsafe { *pVirtualAddress = va };
    NV_OK
}

#[no_mangle]
pub extern "C" fn osMapPciMemoryUser(
    arg0: *mut c_void,
    arg1: NvU64,
    arg2: NvU64,
    arg3: NvU32,
    arg4: *mut c_void,
    arg5: *mut c_void,
    arg6: NvU32,
) -> *mut c_void {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    let _ = arg3;
    let _ = arg4;
    let _ = arg5;
    let _ = arg6;
    core::ptr::null_mut()
}

// osMapSystemMemory: real impl in vendor/eclipse_rm_mem.c.

#[no_mangle]
pub extern "C" fn osMatchGpuOsInfo(pGpu: *mut c_void, pOsInfo: *mut c_void) -> NvBool {
    let _ = pGpu;
    let _ = pOsInfo;
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osMemacctReleaseCharge(pool: *mut c_void, size: NvU64) {
    let _ = pool;
    let _ = size;
}

#[no_mangle]
pub extern "C" fn osMemacctTryCharge(
    pRegion: *mut c_void,
    size: NvU64,
    ppPool: *mut *mut c_void,
) -> NV_STATUS {
    let _ = pRegion;
    let _ = size;
    let _ = ppPool;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osModifyGpuSwStatePersistence(arg0: *mut c_void, arg1: NvBool) {
    let _ = arg0;
    let _ = arg1;
}

#[no_mangle]
pub extern "C" fn osNotifyEvent(
    arg0: *mut c_void,
    arg1: *mut c_void,
    arg2: NvU32,
    arg3: NvU32,
    arg4: NV_STATUS,
    arg5: NvBool,
) -> *mut c_void {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    let _ = arg3;
    let _ = arg4;
    let _ = arg5;
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn osNumaAddGpuMemory(
    pOsGpuInfo: *mut c_void,
    offset: NvU64,
    size: NvU64,
    pNumaNodeId: *mut NvU32,
) -> NV_STATUS {
    let _ = pOsGpuInfo;
    let _ = offset;
    let _ = size;
    let _ = pNumaNodeId;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osNumaMemblockSize(arg0: *mut NvU64) -> NV_STATUS {
    let _ = arg0;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osNumaOnliningEnabled(arg0: *mut c_void) -> NvBool {
    let _ = arg0;
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osNumaRemoveGpuMemory(
    pOsGpuInfo: *mut c_void,
    offset: NvU64,
    size: NvU64,
    numaNodeId: NvU32,
) {
    let _ = pOsGpuInfo;
    let _ = offset;
    let _ = size;
    let _ = numaNodeId;
}

#[no_mangle]
pub extern "C" fn osObjectEventNotification(
    arg0: *mut c_void,
    arg1: *mut c_void,
    arg2: NvU32,
    arg3: *mut c_void,
    arg4: NvU32,
    arg5: *mut c_void,
    arg6: NvU32,
) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    let _ = arg3;
    let _ = arg4;
    let _ = arg5;
    let _ = arg6;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osOfflinePageAtAddress(address: NvU64) -> NV_STATUS {
    let _ = address;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osOpenTemporaryFile(ppFile: *mut *mut c_void) -> NV_STATUS {
    let _ = ppFile;
    NV_ERR_NOT_SUPPORTED
}

// osPackageRegistry is implemented in C (vendor/eclipse_rm_init.c) where the
// real PACKED_REGISTRY_TABLE type is in scope: the GSP SET_REGISTRY RPC
// (rpcSetRegistry_v17_00) needs a well-formed packed table, and the prior
// Rust NV_ERR_NOT_SUPPORTED stub aborted kgspInitRm at kernel_gsp.c:3376.

#[no_mangle]
pub extern "C" fn osPciInitHandle(
    domain: NvU32,
    bus: NvU8,
    slot: NvU8,
    function: NvU8,
    pVendor: *mut NvU16,
    pDevice: *mut NvU16,
) -> *mut c_void {
    crate::os_interface::os_pci_init_handle(domain, bus, slot, function, pVendor, pDevice)
}

#[no_mangle]
pub extern "C" fn osPutPidInfo(pOsPidInfo: *mut c_void) {
    let _ = pOsPidInfo;
}

#[no_mangle]
pub extern "C" fn osQueueDrainP2PHandler(arg0: *mut NvU8) -> NV_STATUS {
    let _ = arg0;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osQueueMMUFaultHandler(arg0: *mut c_void) {
    let _ = arg0;
}

#[no_mangle]
pub extern "C" fn osQueueResumeP2PHandler(arg0: *mut NvU8) {
    let _ = arg0;
}

#[no_mangle]
pub extern "C" fn osQueueSystemWorkItem(arg0: *mut c_void, arg1: *mut c_void) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    NV_ERR_NOT_SUPPORTED
}

// 570.144 replaced the bare osQueueWorkItem entry point with
// osQueueWorkItemWithFlags(OBJGPU *, OSWorkItemFunction, void *, NvU32 flags)
// -- osQueueWorkItem is now a static inline in g_os_nvoc.h that forwards to it,
// so the linker needs the *WithFlags* symbol. Deferred work items require an OS
// worker-thread pool Eclipse's RM host does not run; returning NOT_SUPPORTED
// matches the prior 610 behaviour (the old osQueueWorkItem stub also refused),
// and callers (mem_mapper.c, gpuRefreshRecoveryAction, vgpu_events.c) treat a
// failed queue as "not available" rather than fatal.
#[no_mangle]
pub extern "C" fn osQueueWorkItemWithFlags(
    pGpu: *mut c_void,
    pFunction: *mut c_void,
    pParams: *mut c_void,
    flags: NvU32,
) -> NV_STATUS {
    let _ = pGpu;
    let _ = pFunction;
    let _ = pParams;
    let _ = flags;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osReadFromFile(
    pFile: *mut c_void,
    buffer: *mut NvU8,
    size: NvU64,
    offset: NvU64,
) -> NV_STATUS {
    let _ = pFile;
    let _ = buffer;
    let _ = size;
    let _ = offset;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osReadRegistryBinary(
    arg0: *mut c_void,
    arg1: *mut c_char,
    arg2: *mut NvU8,
    arg3: *mut NvU32,
) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    let _ = arg3;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osReadRegistryDwordBase(
    arg0: *mut c_void,
    arg1: *mut c_char,
    arg2: *mut NvU32,
) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osReadRegistryStringBase(
    arg0: *mut c_void,
    arg1: *mut c_char,
    arg2: *mut NvU8,
    arg3: *mut NvU32,
) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    let _ = arg3;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osReadRegistryVolatile(
    arg0: *mut c_void,
    arg1: *mut c_char,
    arg2: *mut NvU8,
    arg3: NvU32,
) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    let _ = arg3;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osReadRegistryVolatileSize(
    arg0: *mut c_void,
    arg1: *mut c_char,
    arg2: *mut NvU32,
) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osRefGpuAccessNeeded(pOsGpuInfo: *mut c_void) -> NV_STATUS {
    let _ = pOsGpuInfo;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osReleaseGpuOsInfo(pOsInfo: *mut c_void) {
    let _ = pOsInfo;
}

#[no_mangle]
pub extern "C" fn osReleaseRmSema(arg0: *mut c_void, arg1: *mut c_void) -> NvU32 {
    let _ = arg0;
    let _ = arg1;
    0
}

#[no_mangle]
pub extern "C" fn osRemoveGpu(domain: NvU32, bus: NvU8, device: NvU8) {
    let _ = domain;
    let _ = bus;
    let _ = device;
}

#[no_mangle]
pub extern "C" fn osRemoveGpuSupported() -> NvBool {
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osRmCapAcquire(
    pOsRmCaps: *mut c_void,
    rmCap: NvU32,
    capDescriptor: NvU64,
    dupedCapDescriptor: *mut NvU64,
) -> NV_STATUS {
    let _ = pOsRmCaps;
    let _ = rmCap;
    let _ = capDescriptor;
    let _ = dupedCapDescriptor;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osRmCapInitDescriptor(pCapDescriptor: *mut NvU64) {
    let _ = pCapDescriptor;
}

#[no_mangle]
pub extern "C" fn osRmCapRegisterGpu(
    pOsGpuInfo: *mut c_void,
    ppOsRmCaps: *mut *mut c_void,
) -> NV_STATUS {
    // Real osRmCapRegisterGpu (os.c:4834) opens with
    // "Return success on the unsupported platforms. if (nvidia_caps_root
    // == NULL) return NV_OK;" -- i.e. when there's no capability
    // filesystem (exactly Eclipse's case) it succeeds without doing
    // anything. _gpumgrRegisterRmCapsForGpu (gpu_mgr.c:95) RETURNS this
    // status and gpumgrAttachGpu bails on it (gpu_mgr.c:1579), so the old
    // NOT_SUPPORTED stub also broke attach. Leave *ppOsRmCaps as-is (NULL).
    let _ = pOsGpuInfo;
    let _ = ppOsRmCaps;
    NV_OK
}

#[no_mangle]
pub extern "C" fn osRmCapRegisterSmcExecutionPartition(
    pPartitionOsRmCaps: *mut c_void,
    ppExecPartitionOsRmCaps: *mut *mut c_void,
    execPartitionId: NvU32,
) -> NV_STATUS {
    let _ = pPartitionOsRmCaps;
    let _ = ppExecPartitionOsRmCaps;
    let _ = execPartitionId;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osRmCapRegisterSmcPartition(
    pGpuOsRmCaps: *mut c_void,
    ppPartitionOsRmCaps: *mut *mut c_void,
    partitionId: NvU32,
) -> NV_STATUS {
    let _ = pGpuOsRmCaps;
    let _ = ppPartitionOsRmCaps;
    let _ = partitionId;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osRmCapRegisterSys(ppOsRmCaps: *mut *mut c_void) -> NV_STATUS {
    let _ = ppOsRmCaps;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osRmCapRelease(dupedCapDescriptor: NvU64) {
    let _ = dupedCapDescriptor;
}

#[no_mangle]
pub extern "C" fn osRmCapUnregister(ppOsRmCaps: *mut *mut c_void) {
    let _ = ppOsRmCaps;
}

// osRmInitRm is NOT stubbed here: sysConstruct_IMPL (system.c) calls it
// during OBJSYS construction and a NOT_SUPPORTED return would fail the
// whole construction. The real Eclipse implementation (REGISTER_ALL_HALS
// + threadStateInitSetupFlags, mirroring Linux's osinit.c version) lives
// in vendor/eclipse_rm_init.c where the generated g_hal_register.h
// inline registration functions are reachable from C.

#[no_mangle]
pub extern "C" fn osSetEvent(arg0: *mut c_void, arg1: *mut c_void) -> NvU32 {
    let _ = arg0;
    let _ = arg1;
    0
}

#[no_mangle]
pub extern "C" fn osSpinLoop() {
    // Heartbeat: osSpinLoop is called on every iteration of every RM wait
    // loop (timeoutCondWait, RPC recv polls, register polls). After "GSP FW
    // RM ready." the machine stopped producing output with no error -- this
    // distinguishes "alive but stuck in an RM wait loop" (heartbeats keep
    // printing, and the count says how hard it is spinning) from "CPU wedged
    // on a dead MMIO access" (heartbeats stop too). Throttled to one line
    // per 2M iterations and hard-capped so it can never flood the console.
    use core::sync::atomic::{AtomicU64, Ordering};
    static SPIN_COUNT: AtomicU64 = AtomicU64::new(0);
    let n = SPIN_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    // First beat early (200k ≈ ~10 ms): the machine froze ~60 ms after "GSP
    // FW RM ready.", too soon for a 2M-iteration first beat to prove whether
    // a wait loop was even running.
    if n == 200_000 || (n.is_multiple_of(2_000_000) && n <= 80_000_000) {
        log::debug!(
            "[nvidia-rm] osSpinLoop heartbeat: {}k iterations",
            n / 1_000
        );
    }
    // Drain this CPU's TLB-shootdown queue at a coarse cadence. osSpinLoop is
    // the common heartbeat of EVERY RM wait loop — timeoutCondWait, GSP RPC
    // recv polls, register polls — and these run with interrupts off (behind
    // an RM spinlock or a Rust lock held across the FFI call), so without this
    // the CPU cannot acknowledge a peer's shootdown for the whole wait. Another
    // CPU doing a munmap/VM_BIND unmap then spins forever inside
    // remote_flush_tlb_aspace holding the VMAR inner+page_table locks, and every
    // later address-space op convoys behind it: the vmar.rs `DEADLOCK` on real
    // RTX hardware after "GSP FW RM ready." The udelay hook only covered the
    // os_delay_us waits; this covers the tight-spin ones (osSpinLoop) too — the
    // whole set. Cheap: one relaxed load when the queue is empty. QEMU never hit
    // it because the RM barely runs without a real GPU.
    if n & 511 == 0 {
        lock::pump();
    }
    core::hint::spin_loop();
}

#[no_mangle]
pub extern "C" fn osStartNanoTimer(
    pArg1: *mut c_void,
    pTimer: *mut c_void,
    timeNs: NvU64,
) -> NV_STATUS {
    let _ = pArg1;
    let _ = pTimer;
    let _ = timeNs;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osSyncWithGpuDestroy(arg0: NvBool) {
    let _ = arg0;
}

#[no_mangle]
pub extern "C" fn osSyncWithRmDestroy() {}

#[no_mangle]
pub extern "C" fn osTegraAllocateDisplayBandwidth(
    pOsGpuInfo: *mut c_void,
    averageBandwidthKBPS: NvU32,
    floorBandwidthKBPS: NvU32,
) -> NV_STATUS {
    let _ = pOsGpuInfo;
    let _ = averageBandwidthKBPS;
    let _ = floorBandwidthKBPS;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osTegraDceClientIpcSendRecv(
    clientId: NvU32,
    msg: *mut c_void,
    msgLength: NvU32,
) -> NV_STATUS {
    let _ = clientId;
    let _ = msg;
    let _ = msgLength;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osTegraDceRegisterIpcClient(
    interfaceType: NvU32,
    usrCtx: *mut c_void,
    clientId: *mut NvU32,
) -> NV_STATUS {
    let _ = interfaceType;
    let _ = usrCtx;
    let _ = clientId;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osTegraDceUnregisterIpcClient(clientId: NvU32) -> NV_STATUS {
    let _ = clientId;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osTegraSocGetDispClockRates(
    pOsGpuInfo: *mut c_void,
    pMaxDispClkRateDisppll: *mut NvU32,
    pMaxDispClkRateSppllClkouta: *mut NvU32,
    pMaxHubClkRateSppllClkoutb: *mut NvU32,
) -> NV_STATUS {
    let _ = pOsGpuInfo;
    let _ = pMaxDispClkRateDisppll;
    let _ = pMaxDispClkRateSppllClkouta;
    let _ = pMaxHubClkRateSppllClkoutb;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osTegraSocGetImpImportData(
    pGpu: *mut c_void,
    pTegraImpImportData: *mut c_void,
) -> NV_STATUS {
    let _ = pGpu;
    let _ = pTegraImpImportData;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osTegraSocGetImpUefiData(
    pOsGpuInfo: *mut c_void,
    pIsoBwKbps: *mut NvU32,
    pFloorBwKbps: *mut NvU32,
) -> NV_STATUS {
    let _ = pOsGpuInfo;
    let _ = pIsoBwKbps;
    let _ = pFloorBwKbps;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osTegraiGpuPerfBoost(
    pGpu: *mut c_void,
    enable: NvBool,
    duration: NvU32,
) -> NV_STATUS {
    let _ = pGpu;
    let _ = enable;
    let _ = duration;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osTestPcieExtendedConfigAccess(handle: *mut c_void, offset: NvU32) -> NvBool {
    let _ = handle;
    let _ = offset;
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osUidTokensEqual(arg1: *mut c_void, arg2: *mut c_void) -> NvBool {
    let _ = arg1;
    let _ = arg2;
    NV_FALSE
}

#[no_mangle]
pub extern "C" fn osUnlockMem(arg0: *mut c_void) -> NV_STATUS {
    let _ = arg0;
    NV_ERR_NOT_SUPPORTED
}

// osUnmapGPU: real implementation in C (vendor/eclipse_rm_init.c),
// paired with osMapGPU -- see the rationale at the osMapGPU comment above.

#[no_mangle]
pub extern "C" fn osUnmapKernelSpace(addr: *mut c_void, size: NvU64) {
    crate::os_interface::os_unmap_kernel_space(addr, size)
}

#[no_mangle]
pub extern "C" fn osUnmapPciMemoryKernel64(arg0: *mut c_void, arg1: *mut c_void) {
    let _ = arg0;
    let _ = arg1;
}

#[no_mangle]
pub extern "C" fn osUnmapPciMemoryKernelOld(arg0: *mut c_void, arg1: *mut c_void) {
    let _ = arg0;
    let _ = arg1;
}

#[no_mangle]
pub extern "C" fn osUnmapPciMemoryUser(
    arg0: *mut c_void,
    arg1: *mut c_void,
    arg2: NvU64,
    arg3: *mut c_void,
) {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    let _ = arg3;
}

// osUnmapSystemMemory: real impl in vendor/eclipse_rm_mem.c.

#[no_mangle]
pub extern "C" fn osUnrefGpuAccessNeeded(pOsGpuInfo: *mut c_void) {
    let _ = pOsGpuInfo;
}

#[no_mangle]
pub extern "C" fn osUserHandleToKernelPtr(
    hClient: NvU32,
    Handle: *mut c_void,
    pHandle: *mut c_void,
) -> NV_STATUS {
    let _ = hClient;
    let _ = Handle;
    let _ = pHandle;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osValidateClientTokens(arg1: *mut c_void, arg2: *mut c_void) -> NV_STATUS {
    let _ = arg1;
    let _ = arg2;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osWaitInterruptible(pWq: *mut c_void) {
    let _ = pWq;
}

#[no_mangle]
pub extern "C" fn osWakeRemoveVgpu(arg0: NvU32, arg1: NvU32) {
    let _ = arg0;
    let _ = arg1;
}

#[no_mangle]
pub extern "C" fn osWakeUp(pWq: *mut c_void) {
    let _ = pWq;
}

#[no_mangle]
pub extern "C" fn osWriteRegistryDword(
    arg0: *mut c_void,
    arg1: *mut c_char,
    arg2: NvU32,
) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osWriteRegistryVolatile(
    arg0: *mut c_void,
    arg1: *mut c_char,
    arg2: *mut NvU8,
    arg3: NvU32,
) -> NV_STATUS {
    let _ = arg0;
    let _ = arg1;
    let _ = arg2;
    let _ = arg3;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn osWriteToFile(
    pFile: *mut c_void,
    buffer: *mut NvU8,
    size: NvU64,
    offset: NvU64,
) -> NV_STATUS {
    let _ = pFile;
    let _ = buffer;
    let _ = size;
    let _ = offset;
    NV_ERR_NOT_SUPPORTED
}

#[no_mangle]
pub extern "C" fn os_get_system_time(arg0: *mut NvU32, arg1: *mut NvU32) -> *mut c_void {
    let _ = arg0;
    let _ = arg1;
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn osPciReadByte(pHandle: *mut c_void, offset: NvU32) -> NvU8 {
    with_hooks(0xFF, |h| h.pci_config_read(pHandle as usize, offset, 1)) as NvU8
}

#[no_mangle]
pub extern "C" fn osPciReadWord(pHandle: *mut c_void, offset: NvU32) -> NvU16 {
    with_hooks(0xFFFF, |h| h.pci_config_read(pHandle as usize, offset, 2)) as NvU16
}

#[no_mangle]
pub extern "C" fn osPciReadDword(pHandle: *mut c_void, offset: NvU32) -> NvU32 {
    with_hooks(0xFFFF_FFFF, |h| {
        h.pci_config_read(pHandle as usize, offset, 4)
    })
}

#[no_mangle]
pub extern "C" fn osPciWriteWord(pHandle: *mut c_void, offset: NvU32, value: NvU16) {
    with_hooks((), |h| {
        h.pci_config_write(pHandle as usize, offset, 2, value as u32)
    });
}

#[no_mangle]
pub extern "C" fn osPciWriteDword(pHandle: *mut c_void, offset: NvU32, value: NvU32) {
    with_hooks((), |h| {
        h.pci_config_write(pHandle as usize, offset, 4, value)
    });
}

#[no_mangle]
pub extern "C" fn osMapKernelSpace(
    start: NvU64,
    size: NvU64,
    _mode: NvU32,
    _protect: NvU32,
) -> *mut c_void {
    crate::os_interface::os_map_kernel_space(start, size, _mode)
}

// The functions below surfaced as real link failures on actual hardware
// builds: they're referenced from vendored core files (os_init.c, cpu.c,
// phys_mem_allocator.c, locks.c) but aren't part of the open-sourced
// Linux platform glue (arch/nvalloc/unix/src/), so they were missed by
// the original os_boundary.rs sweep. Real declarations transcribed from
// g_os_nvoc.h.
//
// Note: osNv_rdxcr0 is NOT implemented here even though it's declared
// right alongside osNv_rdcr4/osNv_cpuid -- unlike those two, the real
// os_stubs.c (vendored, always compiled) defines it unconditionally
// (no RMCFG_FEATURE_PLATFORM_UNIX/NVCPU_IS_X86_64 guard), so providing
// it here is a duplicate-symbol link error, confirmed against a real
// build. osNv_rdcr4/osNv_cpuid ARE guarded out by os_stubs.c specifically
// for RMCFG_FEATURE_PLATFORM_UNIX && NVCPU_IS_X86_64, which is why they
// were genuinely missing.
//
// osNv_cpuid/osNv_rdcr4 are genuine host-CPU operations -- Eclipse runs
// directly on the x86_64 CPU, so these execute the real instructions
// rather than stubbing, unlike GPU-hardware-dependent calls.

#[no_mangle]
pub extern "C" fn osInitObjOS(pOS: *mut c_void) {
    // Real Linux implementations of this hook are platform-specific
    // bookkeeping (not part of the open-sourced RM core); OBJOS is
    // already zero-initialized by NVOC construction, so there is
    // nothing Eclipse needs to add here.
    let _ = pOS;
}

#[no_mangle]
pub extern "C" fn osNv_rdcr4() -> NvU32 {
    // x86_64: read the CR4 control register directly.
    // On non-x86_64 architectures there is no CR4 equivalent; return 0
    // so RM's CPU feature detection sees a clear register (no unexpected
    // bits set) rather than an undefined value.
    #[cfg(target_arch = "x86_64")]
    {
        let value: u64;
        unsafe {
            core::arch::asm!("mov {}, cr4", out(reg) value);
        }
        value as NvU32
    }
    #[cfg(not(target_arch = "x86_64"))]
    0
}

#[no_mangle]
pub extern "C" fn osNv_cpuid(
    leaf: i32,
    subleaf: i32,
    peax: *mut NvU32,
    pebx: *mut NvU32,
    pecx: *mut NvU32,
    pedx: *mut NvU32,
) -> i32 {
    // x86_64: execute the CPUID instruction.
    // CRITICAL: save/restore rbx via the STACK, and never name ebx as
    // an operand. The previous version used a scratch-reg save/restore
    // ("mov tmp,ebx; cpuid; xchg tmp,ebx"), which corrupted a caller
    // pointer -- reproduced on the host as an immediate segfault, and
    // on real hardware as the [KERNEL PAGE FAULT] WRITE to a
    // 32-bit-truncated stack address (0x311eed4) during RmInitCpuInfo's
    // very first osNv_cpuid call. cpuid clobbers all of eax/ebx/ecx/edx
    // at once; leaving rbx for LLVM to juggle across it is fragile.
    // push/pop rbx keeps rbx fully out of the register allocator's way.
    #[cfg(target_arch = "x86_64")]
    {
        let mut a: u32 = leaf as u32;
        let b: u32;
        let mut c: u32 = subleaf as u32;
        let d: u32;
        unsafe {
            core::arch::asm!(
                "push rbx",
                "cpuid",
                "mov {ebx_out:e}, ebx",
                "pop rbx",
                ebx_out = lateout(reg) b,
                inout("eax") a,
                inout("ecx") c,
                lateout("edx") d,
                options(preserves_flags),
            );
            if !peax.is_null() {
                *peax = a;
            }
            if !pebx.is_null() {
                *pebx = b;
            }
            if !pecx.is_null() {
                *pecx = c;
            }
            if !pedx.is_null() {
                *pedx = d;
            }
        }
        1
    }
    // On non-x86_64 architectures CPUID does not exist; zero all outputs
    // and return 0 (unsupported) so RM's CPU-info initialisation sees
    // an empty feature set rather than undefined values.
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (leaf, subleaf);
        unsafe {
            if !peax.is_null() {
                *peax = 0;
            }
            if !pebx.is_null() {
                *pebx = 0;
            }
            if !pecx.is_null() {
                *pecx = 0;
            }
            if !pedx.is_null() {
                *pedx = 0;
            }
        }
        0
    }
}

#[no_mangle]
pub extern "C" fn osGetNumaMemoryUsage(
    numa_id: NvS32,
    free_memory_bytes: *mut NvU64,
    total_memory_bytes: *mut NvU64,
) {
    // STUB: Eclipse has no NUMA concept (single discrete GPU, no ACPI
    // NUMA topology), matching the existing os_get_numa_node_memory_usage
    // precedent in os_interface.rs.
    let _ = numa_id;
    unsafe {
        if !free_memory_bytes.is_null() {
            *free_memory_bytes = 0;
        }
        if !total_memory_bytes.is_null() {
            *total_memory_bytes = 0;
        }
    }
}

#[no_mangle]
pub extern "C" fn osGpuLocksQueueRelease(
    pGpu: *mut c_void,
    dpc_gpu_lock_release: NvU32,
) -> NV_STATUS {
    // Real Linux implementation defers lock release to a deferred
    // procedure call (DPC) queue for interrupt-context safety; Eclipse's
    // lock implementation doesn't need that indirection, so this is a
    // no-op that reports success (locks are released synchronously by
    // the caller elsewhere).
    let _ = pGpu;
    let _ = dpc_gpu_lock_release;
    NV_OK
}
