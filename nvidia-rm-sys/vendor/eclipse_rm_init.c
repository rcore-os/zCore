/*
 * OUR code, not NVIDIA's -- the Eclipse-native equivalent of what
 * NVIDIA's real Linux platform layer does around RM bring-up
 * (kernel-open's rm_init_rm entry + arch/nvalloc/unix/src/osinit.c's
 * osRmInitRm / osInitNvMapping / RmInitAdapter), which Eclipse doesn't
 * vendor since it's Linux-specific. Every RM function called from here
 * (tlsInitialize, coreInitializeRm, REGISTER_ALL_HALS,
 * threadStateInitSetupFlags, rmapiLockAcquire/Release,
 * gpumgrAllocGpuInstance, rmGpuLockAlloc, gpumgrCreateDevice,
 * gpumgrAttachGpu, gpuEncodeDomainBusDevice) is real, unmodified NVIDIA
 * code already vendored via build.rs -- this file only sequences the
 * calls and packages Eclipse's own PCI/BAR info into the real
 * GPUATTACHARG struct, the same way osInitNvMapping does from
 * nv_state_t.
 *
 * HISTORY -- the original version of this file hand-rolled init as
 * __nvoc_objCreate_OBJSYS + rmapiInitialize + threadStateInitSetupFlags,
 * which hung real hardware solid on first use: constructing OBJSYS runs
 * portMemAllocNonPaged (and much more) on an nvport runtime that
 * portInitialize() had never set up (uninitialized global-allocator
 * state and tracking locks -- undefined behavior, in practice a wedge
 * with no output), and then called rmapiInitialize a SECOND time (the
 * real sysConstruct_IMPL already calls it internally, system.c). The
 * real, portable, already-vendored entry point that does everything in
 * the right order is coreInitializeRm() (system.c): portInitialize ->
 * NVLOG_INIT -> DBG_INIT -> objCreate(OBJSYS) -> nvAssertInit, with
 * sysConstruct_IMPL itself then running osRmInitRm, RmInitCpuInfo,
 * rmLocksAlloc, threadStateGlobalAlloc, rmapiInitialize and the rest.
 * tlsInitialize() (tls.c, also real+vendored) must run first, exactly
 * like NVIDIA's own kernel-open interface layer does, since the rmapi
 * lock allocates TLS entries.
 *
 * REGISTER_ALL_HALS mystery, RESOLVED: it's a real generated
 * `static NV_INLINE` function in src/nvidia/generated/g_hal_register.h
 * (not a Linux-only macro as previously hypothesized), and TU106's HAL
 * module is the third thing it registers. sysConstruct_IMPL calls the
 * platform hook osRmInitRm() (which Linux implements in osinit.c to run
 * REGISTER_ALL_HALS + threadStateInitSetupFlags); Eclipse's equivalent
 * implementation of that hook lives right here in this file now,
 * replacing the old os_boundary.rs stub that returned NOT_SUPPORTED
 * (which would have failed OBJSYS construction cleanly).
 */
#include "gpu_mgr/gpu_mgr.h"
#include "gpu/gpu.h"
#include "core/system.h"
#include "core/locks.h"
#include "rmapi/rmapi.h"
#include "core/thread_state.h"
#include "gpu/gsp/kernel_gsp.h"
#include "gpu/gpu_timeout.h"
#include "ctrl/ctrl2080/ctrl2080gpu.h"
#include "ctrl/ctrl2080/ctrl2080fb.h"
#include "ctrl/ctrl2080/ctrl2080gr.h"
#include "ctrl/ctrl2080/ctrl2080internal.h"
#include "os/os.h"
#include "tls/tls.h"
#include "resserv/rs_server.h"
#include "resserv/rs_client.h"
#include "class/cl0080.h"      /* NV01_DEVICE_0 */
#include "class/cl0073.h"      /* NV04_DISPLAY_COMMON */
#include "ctrl/ctrl0073/ctrl0073system.h"    /* GET_SUPPORTED / GET_CONNECT_STATE */
#include "ctrl/ctrl0073/ctrl0073specific.h"  /* GET_EDID_V2 */
#include "kernel/gpu/disp/kern_disp.h" /* GPU_GET_KERNEL_DISPLAY + kdispGet* handles */
#include "class/cl2080.h"      /* NV20_SUBDEVICE_0 */
#include "class/cl2080_notification.h" /* NV2080_ENGINE_TYPE_GRAPHICS */
#include "class/cl90f1.h"      /* FERMI_VASPACE_A */
#include "class/cla06c.h"      /* KEPLER_CHANNEL_GROUP_A */
#include "class/cl9067.h"      /* FERMI_CONTEXT_SHARE_A */
#include "class/cl003e.h"      /* NV01_MEMORY_SYSTEM */
#include "class/cl0040.h"      /* NV01_MEMORY_LOCAL_USER */
#include "class/cl50a0.h"      /* NV50_MEMORY_VIRTUAL */
#include "class/clc5c0.h"      /* TURING_COMPUTE_A */
#include "alloc/alloc_channel.h" /* NV_CHANNEL_ALLOC_PARAMS */
#include "ctrl/ctrla06f.h"     /* NVA06F_CTRL_CMD_GPFIFO_SCHEDULE */
#include "ctrl/ctrl906f.h"     /* NV906F_CTRL_CMD_GET_MMU_FAULT_INFO */
#include "kernel/gpu/fifo/kernel_fifo.h" /* kfifoGetChannelClassId / UserdSizeAlign */
#include "kernel/gpu/fifo/kernel_channel.h" /* CliGetKernelChannel / runlist / USERD memdesc */
#include "class/cl906f.h"      /* GP_ENTRY + DMA method encoding (Fermi format, current) */
#include "class/clc46f.h"      /* TURING_CHANNEL_GPFIFO_A host methods + Nvc46fControl */
#include "mem_mgr/mem.h"       /* Memory / memGetByHandle */
#include "gpu/mem_mgr/heap.h"  /* HEAP_OWNER_RM_CLIENT_GENERIC */
#include "g_hal_register.h"
/* Step 10 (CE memset/copy + readback verify) */
#include "gpu/mem_mgr/mem_mgr.h"
#include "gpu/mem_mgr/mem_desc.h"
#include "gpu/mem_mgr/ce_utils.h"
#include "gpu/bus/kern_bus.h"

/*
 * Real, vendored (src/nvidia/src/kernel/core/system.c), but its declaring
 * header (rmosxfac.h) is not part of the vendored include surface --
 * declared here directly instead.
 */
extern NV_STATUS coreInitializeRm(void);

/*
 * TEMPORARY: checkpoint tracing to find exactly where real hardware hung
 * during the first-ever exercise of this call chain (gpustep5 froze the
 * machine solid, confirmed on real Turing hardware -- no output at all
 * reached /proc/gpustep5, so the hang is somewhere in here or deeper).
 * nv_printf (vendor/glue.c) forwards straight to Eclipse's synchronous
 * console logger, so whatever printed last before a freeze pinpoints the
 * exact call that never returns. Remove once the real hang is found.
 */
extern int nv_printf(unsigned int debuglevel, const char *printf_format, ...);
#define ECLIPSE_TRACE(msg) nv_printf(0, "[eclipse-rm-trace] " msg "\n")

/*
 * Eclipse's implementation of the osRmInitRm platform hook, called from
 * the real sysConstruct_IMPL (system.c) during OBJSYS construction --
 * the portable subset of what Linux's osinit.c version does: register
 * every generated HAL module (TU10X first -- the user's TU106 included)
 * and set up the default ThreadState flags (same four flags as the real
 * Linux implementation). The Linux-only parts (module-param registry
 * import, PCIe-Gen3 regkey plumbing, S0ix checks, nvlog re-init) have no
 * Eclipse equivalent yet and are deliberately omitted.
 */
NV_STATUS osRmInitRm(void)
{
    NV_STATUS status;

    ECLIPSE_TRACE("osRmInitRm: before REGISTER_ALL_HALS");
    status = REGISTER_ALL_HALS();
    ECLIPSE_TRACE("osRmInitRm: after REGISTER_ALL_HALS");
    if (status != NV_OK)
    {
        return status;
    }

    threadStateInitSetupFlags(THREAD_STATE_SETUP_FLAGS_ENABLED |
                              THREAD_STATE_SETUP_FLAGS_TIMEOUT_ENABLED |
                              THREAD_STATE_SETUP_FLAGS_SLI_LOGIC_ENABLED |
                              THREAD_STATE_SETUP_FLAGS_DO_NOT_INCLUDE_SLEEP_TIME_ENABLED);
    ECLIPSE_TRACE("osRmInitRm: done");

    return NV_OK;
}

/*
 * Brings up the real RM core exactly the way NVIDIA's own driver does:
 * tlsInitialize() (as kernel-open's interface layer does before touching
 * RM) followed by coreInitializeRm() (system.c) -- which runs
 * portInitialize, NVLOG_INIT, DBG_INIT, then constructs OBJSYS, whose
 * real constructor performs the entire remaining init sequence
 * (osRmInitRm above, RmInitCpuInfo, rmLocksAlloc, threadStateGlobalAlloc,
 * rmapiInitialize, ...). Call exactly once, before eclipse_rm_attach_gpu.
 */
NV_STATUS eclipse_rm_init_core(void)
{
    NV_STATUS status;

    ECLIPSE_TRACE("init_core: before tlsInitialize");
    status = tlsInitialize();
    ECLIPSE_TRACE("init_core: after tlsInitialize");
    if (status != NV_OK)
    {
        return status;
    }

    ECLIPSE_TRACE("init_core: before coreInitializeRm");
    status = coreInitializeRm();
    ECLIPSE_TRACE("init_core: after coreInitializeRm, done");
    if (status != NV_OK)
    {
        return status;
    }

    /*
     * Eclipse diagnostic (not NVIDIA's): make the RM narrate the attach
     * chain. In a release build nvlog_printf.c's nvDbg_PrintMsg filters
     * every NV_PRINTF below LEVEL_ERROR (debuglevel_min = LEVEL_ERROR),
     * and the RmMsg override string that would lower that threshold is
     * only ever populated by nvDbgInitRmMsg() -- which lives in the Linux
     * platform layer (osReadRegistryString of NV_REG_STR_RM_MSG) that
     * Eclipse does not vendor, so RmMsg stays empty (bss) and all of
     * NVIDIA's own INFO/NOTICE step tracing is silently dropped. That's
     * why a graceful failure like gpumgrAttachGpu returning 0x40
     * (NV_ERR_INVALID_STATE) reaches us with no RM-side explanation on
     * the console at all.
     *
     * Populate RmMsg directly with a *targeted* rule. Per the RmMsg
     * grammar (nvlog_printf.c: `"dmanv50.c" - enable all printfs in
     * dmanv50.c`), a bare filename forces every printf level in that file
     * to print. Scope it to just the two files the attach walks --
     * gpu.c (gpuPostConstruct / gpuDetermineVirtualMode / child-engine
     * construction) and gpu_mgr.c (gpumgrAttachGpu /
     * _gpumgrDetermineConfComputeCapabilities) -- so the last step before
     * the failure is visible without flooding the console (and scrolling
     * the critical line off a monitor-only bring-up box) with every
     * subsystem's INFO spam. Written here, once, after DBG_INIT (inside
     * coreInitializeRm) has run and before eclipse_rm_attach_gpu.
     */
    {
        extern char RmMsg[];
        /*
         * RmMsg grammar (nvDbgRmMsgCheck, nvlog_printf.c): a comma-separated
         * list of nouns; each noun is substring-matched (nv_strnstr) against
         * the source filename, and a hit forces every printf level in that
         * file to print regardless of the release build's LEVEL_ERROR
         * threshold. So the bare substring "kernel_gsp" enables all six
         * kernel_gsp*.c files (kernel_gsp.c orchestration plus the per-arch
         * kernel_gsp_tu102/ga102/gh100/booter_tu102.c that run the actual
         * GSP bootstrap), and "kernel_falcon" enables the FALCON reset/RISC-V
         * bring-up files. This makes the GSP boot narrate itself live: each
         * NV_PRINTF reaches log::warn! -> the graphic console synchronously
         * (drivers/.../nvidia_hooks.rs), so on a hard hang inside
         * kgspBootstrap the LAST line left on the monitor pinpoints which
         * FALCON/Booter/FWSEC step wedged -- the captured /proc/gpustep6
         * buffer is useless for a hang because it is only flushed when the
         * read returns. The chatty post-boot log-polling (kgspStartLogPolling)
         * runs only after _kgspBootGspRm succeeds, so enabling kernel_gsp.c
         * does not flood a boot that hangs before then. gpu.c/gpu_mgr.c stay
         * for the attach (gpustep5) narration.
         */
        /*
         * eng_state.c added for step 9: gpuStatePreInit failed with a silent
         * NV_ERR_STATE_IN_USE (0x63) from some engine's StatePreInit (no
         * assert printed). engstateLogStateTransitionPost prints
         * "Engine %s state change: ..." at LEVEL_INFO for every engine the
         * state machine touches, so with this rule the LAST engine line
         * before the 0x63 names the culprit.
         */
        /*
         * kernel_graphics added for step 9 postLoad: gpuStateLoad reaches the
         * Post-Load phase (KernelBus/BAR2 passed, KernelCE loaded) then faults
         * with a NULL write (vaddr 0x90) after a burst of ~23 GSP RPCs. That
         * RPC count matches kgraphicsStatePostLoad -> kgraphicsLoadStaticInfo,
         * which issues ~12 internal STATIC_KGR_GET_* controls plus the GR
         * internal client/device/subdevice allocs. Enabling kernel_graphics
         * makes that engine narrate itself so the last line before the fault
         * confirms whether GR static-info load is the crasher (its eng_state
         * "state change" line never prints -- it dies mid-postLoad).
         */
        static const char rule[] = "gpu.c,gpu_mgr.c,kernel_gsp,kernel_falcon,eng_state.c,kernel_graphics";
        unsigned int i;
        for (i = 0; rule[i] != '\0'; i++)
        {
            RmMsg[i] = rule[i];
        }
        RmMsg[i] = '\0';
    }

    return status;
}

/*
 * Real equivalent of osInitNvMapping's GPU-attach sequence, packaging
 * Eclipse's own PCI/BAR discovery (from drivers/src/display/nvidia.rs)
 * into the real GPUATTACHARG struct instead of nv_state_t.
 *
 * bar0_phys/bar0_virt/bar0_len: BAR0, the MMIO register aperture.
 * bar1_phys/bar1_len: BAR1, the framebuffer/VRAM aperture (not mapped
 * by Eclipse ahead of time the way BAR0 is, matching
 * osInitNvMapping's own `fbBaseAddr = (GPUHWREG*) 0 // not mapped`).
 * bar2_phys/bar2_len: BAR2 (NVIDIA logical index IMEM), the small
 * instance-memory aperture. Becomes GPUATTACHARG.instPhysAddr/instLength,
 * matching osinit.c:708 (nv->bars[NV_GPU_BAR_INDEX_IMEM]); required for the
 * BAR2 MMU self-test (kbusVerifyBar2) during gpuStateInit.
 */
static NV_STATUS _eclipse_rm_attach_gpu_body(
    NvU32 domain,
    NvU8  bus,
    NvU8  device,
    NvU64 bar0_phys,
    void *bar0_virt,
    NvU64 bar0_len,
    NvU64 bar1_phys,
    NvU64 bar1_len,
    NvU64 bar2_phys,
    NvU64 bar2_len,
    NvU32 *pDeviceInstance)
{
    NvU32 gpuInstance = 0;
    NvU32 deviceInstance = 0;
    NvU32 gpuId;
    NvU64 gpuDomainBusDevice;
    GPUATTACHARG gpuAttachArg;
    NV_STATUS status;

    /*
     * gpumgrAttachGpu's first line is
     * NV_ASSERT_OR_RETURN(rmapiLockIsOwner(), NV_ERR_INVALID_LOCK_STATE)
     * (gpu_mgr.c) -- the real driver always attaches with the RM API lock
     * held, so take it the same way sysConstruct's own internal callers
     * do (API_LOCK_FLAGS_NONE = exclusive write).
     */
    ECLIPSE_TRACE("attach_gpu: before rmapiLockAcquire");
    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    ECLIPSE_TRACE("attach_gpu: after rmapiLockAcquire");
    if (status != NV_OK)
    {
        return status;
    }

    /*
     * Register the GPU in OBJGPUMGR's probedGpus table BEFORE attach, the
     * same thing the real Linux probe (RmInitGpuInfoWithRmApi in
     * arch/nvalloc/unix/src/osinit.c) does. Without this, gpumgrAttachGpu's
     * very first child call (_gpumgrCreateGpu -> gpumgrGetRegisteredIds)
     * finds no probedGpus entry matching this DBDF and returns 0x57
     * (NV_ERR_OBJECT_NOT_FOUND) before OBJGPU is even constructed -- which
     * is exactly the error real hardware just reported. gpuId and
     * gpuDomainBusDevice are derived from the same (domain,bus,device) as
     * the attach arg's nvDomainBusDeviceFunc, matching osinit.c's use of
     * gpuGenerate32BitId / gpuEncodeDomainBusDevice.
     */
    gpuId              = gpuGenerate32BitId(domain, bus, device);
    gpuDomainBusDevice = gpuEncodeDomainBusDevice(domain, bus, device);
    ECLIPSE_TRACE("attach_gpu: before gpumgrRegisterGpuId");
    status = gpumgrRegisterGpuId(gpuId, gpuDomainBusDevice);
    ECLIPSE_TRACE("attach_gpu: after gpumgrRegisterGpuId");
    if (status != NV_OK)
    {
        rmapiLockRelease();
        return status;
    }

    /*
     * The whole attach path needs "expanded GPU visibility" on this thread,
     * exactly as RmInitAdapter enables it around osInitNvMapping
     * (osinit.c:2039, comment: "Initialization path requires expanded GPU
     * visibility in GPUMGR in order to access the GPU undergoing
     * initialization"). Without it, gpumgrGetGpu (gpu_mgr.c:500) refuses to
     * return a GPU whose PDB_PROP_GPU_STATE_INITIALIZED is still false --
     * which it is during attach -- so gpumgrAttachGpu's own
     * `pGpu = gpumgrGetGpu(...)` (gpu_mgr.c:1538) returns NULL and the very
     * next line dereferences it (pGpu->gpuInstance), the real
     * [KERNEL PAGE FAULT] READ @ 0x7cc (offset of gpuInstance in a NULL
     * OBJGPU) hardware just reported at gpu_mgr.c:1540.
     */
    ECLIPSE_TRACE("attach_gpu: before gpumgrThreadEnableExpandedGpuVisibility");
    status = gpumgrThreadEnableExpandedGpuVisibility();
    ECLIPSE_TRACE("attach_gpu: after gpumgrThreadEnableExpandedGpuVisibility");
    if (status != NV_OK)
    {
        gpumgrUnregisterGpuId(gpuId);
        rmapiLockRelease();
        return status;
    }

    ECLIPSE_TRACE("attach_gpu: before gpumgrAllocGpuInstance");
    status = gpumgrAllocGpuInstance(&gpuInstance);
    ECLIPSE_TRACE("attach_gpu: after gpumgrAllocGpuInstance");
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        gpumgrUnregisterGpuId(gpuId);
        rmapiLockRelease();
        return status;
    }

    ECLIPSE_TRACE("attach_gpu: before rmGpuLockAlloc");
    status = rmGpuLockAlloc(gpuInstance);
    ECLIPSE_TRACE("attach_gpu: after rmGpuLockAlloc");
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        gpumgrUnregisterGpuId(gpuId);
        rmapiLockRelease();
        return status;
    }

    ECLIPSE_TRACE("attach_gpu: before gpumgrCreateDevice");
    status = gpumgrCreateDevice(&deviceInstance, NVBIT(gpuInstance), NULL);
    ECLIPSE_TRACE("attach_gpu: after gpumgrCreateDevice");
    if (status != NV_OK)
    {
        rmGpuLockFree(gpuInstance);
        gpumgrThreadDisableExpandedGpuVisibility();
        gpumgrUnregisterGpuId(gpuId);
        rmapiLockRelease();
        return status;
    }

    portMemSet(&gpuAttachArg, 0, sizeof(gpuAttachArg));
    gpuAttachArg.devPhysAddr           = bar0_phys;
    gpuAttachArg.regBaseAddr           = (GPUHWREG *)bar0_virt;
    gpuAttachArg.regLength             = bar0_len;
    gpuAttachArg.fbPhysAddr            = bar1_phys;
    gpuAttachArg.fbBaseAddr            = (GPUHWREG *)0; /* not mapped, same as real driver */
    gpuAttachArg.fbLength              = bar1_len;
    /*
     * BAR2 / IMEM instance-memory aperture. osinit.c:708 sets
     * instPhysAddr = nv->bars[NV_GPU_BAR_INDEX_IMEM].cpu_address,
     * instBaseAddr = 0 (not mapped), instLength = that BAR's size. RM
     * programs BAR2 from instPhysAddr; without it kbusVerifyBar2_GM107's
     * MMU self-test write cannot reach VRAM and gpuStateInit fails 0x72.
     */
    gpuAttachArg.instPhysAddr          = bar2_phys;
    gpuAttachArg.instBaseAddr          = (GPUHWREG *)0; /* not mapped, same as real driver */
    gpuAttachArg.instLength            = bar2_len;
    nv_printf(0, "[eclipse-rm-trace] attach_gpu: BAR2/IMEM instPhysAddr=0x%llx instLength=0x%llx\n",
              (unsigned long long)bar2_phys, (unsigned long long)bar2_len);
    gpuAttachArg.intLine               = 0;
    gpuAttachArg.nvDomainBusDeviceFunc = gpuEncodeDomainBusDevice(domain, bus, device);
    // NV_IOVA_DOMAIN_NONE/NV0000_CTRL_NO_NUMA_NODE live in headers this
    // build doesn't otherwise need to pull in (nv.h is Linux-specific;
    // the ctrl0000 NUMA sentinel wasn't worth a whole extra header for
    // two fields) -- both are just "not applicable" sentinels for a
    // plain PCIe GPU with no SMMU/NUMA, using this codebase's universal
    // NvU32/NvS32 "none" conventions (all-ones / -1) instead.
    gpuAttachArg.iovaspaceId           = 0xFFFFFFFFu;
    gpuAttachArg.cpuNumaNodeId         = -1;
    gpuAttachArg.pOsAttachArg          = NULL;

    /*
     * Run as a GSP firmware client, NOT monolithic RM. This is the single
     * most important attach flag for a Turing GPU on the open kernel
     * modules: the real Linux driver sets nv->request_fw_client_rm =
     * NV_TRUE whenever the GSP firmware image loads successfully
     * (osinit.c:1946, RmInitGspFirmware) -- which for our TU106 it does,
     * gsp.bin is present -- and then copies it straight into the attach
     * arg (osinit.c:884: `gpuAttachArg->bRequestFwClientRm =
     * nv->request_fw_client_rm`). From there gpumgrGetGpuHalFactor ->
     * gpumgrCheckRmFirmwarePolicy turns it into isFwClient=TRUE (TU106 is
     * >= TU100 so _gpumgrIsRmFirmwareCapableChip passes), which selects
     * RM_RUNTIME_VARIANT_PF_KERNEL_ONLY and makes IS_GSP_CLIENT(pGpu) true
     * for the whole engine topology.
     *
     * Leaving this NV_FALSE (the portMemSet default) selected
     * RM_RUNTIME_VARIANT_PF_MONOLITHIC -- a full-fat monolithic RM that
     * the open kernel modules do not support on Turing+ (GSP is enabled by
     * default for every TU100+ chip per gpumgrIsDeviceRmFirmwareCapable),
     * so gpuPostConstruct built the wrong, unsupported engine topology.
     * That mismatch is the prime suspect for the graceful
     * NV_ERR_INVALID_STATE (0x40) gpumgrAttachGpu returns on real
     * hardware. GSP is only *constructed* during attach here; it is not
     * booted until eclipse_rm_init_gsp (gpustep6, kgspInitRm), so this
     * does not itself touch GSP hardware.
     */
    gpuAttachArg.bRequestFwClientRm    = NV_TRUE;

    /*
     * Pass gpuInstance (from gpumgrAllocGpuInstance), NOT deviceInstance
     * (from gpumgrCreateDevice) -- osinit.c:889 does exactly this:
     * `gpumgrAttachGpu(*pDeviceReference, ...)` where *pDeviceReference is
     * the gpuInstance filled by gpumgrAllocGpuInstance at osinit.c:793.
     * gpumgrAttachGpu constructs the OBJGPU at index gpuInstance and looks
     * it back up by the same gpuInstance (gpu_mgr.c:1531/1538), so the
     * argument must be the gpuInstance, not the device grouping index.
     */
    ECLIPSE_TRACE("attach_gpu: before gpumgrAttachGpu");
    status = gpumgrAttachGpu(gpuInstance, &gpuAttachArg);
    ECLIPSE_TRACE("attach_gpu: after gpumgrAttachGpu");
    if (status != NV_OK)
    {
        gpumgrDestroyDevice(deviceInstance);
        rmGpuLockFree(gpuInstance);
        gpumgrThreadDisableExpandedGpuVisibility();
        gpumgrUnregisterGpuId(gpuId);
        rmapiLockRelease();
        return status;
    }

    /*
     * Return gpuInstance (not deviceInstance): the GSP-boot path
     * (eclipse_rm_init_gsp) uses this value with gpumgrGetGpu, which is
     * keyed by gpuInstance.
     */
    if (pDeviceInstance != NULL)
    {
        *pDeviceInstance = gpuInstance;
    }

    gpumgrThreadDisableExpandedGpuVisibility();
    rmapiLockRelease();
    ECLIPSE_TRACE("attach_gpu: done, OK");
    return NV_OK;
}

/*
 * Public attach entry: registers a THREAD_STATE_NODE around the body, like
 * every real RM entry point (see the identical bracket and rationale in
 * eclipse_rm_init_gsp -- without a node, threadStateGetCurrent() returns
 * NV_ERR_OBJECT_NOT_FOUND and explicit-timeout RM waits abort on their first
 * gpuCheckTimeout instead of waiting).
 */
NV_STATUS eclipse_rm_attach_gpu(
    NvU32 domain,
    NvU8  bus,
    NvU8  device,
    NvU64 bar0_phys,
    void *bar0_virt,
    NvU64 bar0_len,
    NvU64 bar1_phys,
    NvU64 bar1_len,
    NvU64 bar2_phys,
    NvU64 bar2_len,
    NvU32 *pDeviceInstance)
{
    THREAD_STATE_NODE threadState;
    NV_STATUS status;

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);
    status = _eclipse_rm_attach_gpu_body(domain, bus, device,
                                         bar0_phys, bar0_virt, bar0_len,
                                         bar1_phys, bar1_len,
                                         bar2_phys, bar2_len, pDeviceInstance);
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return status;
}

/*
 * Boots GSP-RM via the vendored, unmodified kgspInitRm (kernel_gsp.c).
 * Only the raw gsp.bin bytes are supplied: RM parses the image/signature
 * sections out of pBuf itself (_kgspPrepareGspRmBinaryImage), and
 * self-allocates the Booter Load/Unload ucodes and the RISC-V bootloader
 * stub from BINDATA_ARCHIVE blobs already compiled into this vendored
 * core (kernel_gsp_booter.c's kgspGetBinArchiveBooterLoadUcode_HAL /
 * kernel_gsp.c's kgspGetGspRmBootUcodeStorage_HAL) rather than from any
 * file Eclipse provides -- confirmed those are NOT external-file reads.
 *
 * pGpu->isGspClient defaults to NV_TRUE for any non-Tegra chip under the
 * PF_KERNEL_ONLY RM variant (__nvoc_init_dataField_OBJGPU in
 * g_gpu_nvoc.c), which is what a discrete-GPU build like this one
 * selects, so Eclipse doesn't need to force it on.
 */
NV_STATUS eclipse_rm_init_gsp(NvU32 gpuInstance, const void *pBuf, NvU32 size)
{
    OBJGPU *pGpu;
    KernelGsp *pKernelGsp;
    GSP_FIRMWARE gspFw;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;

    /*
     * Register a THREAD_STATE_NODE for this bring-up call, exactly like every
     * real RM entry point does (rmapi/*.c: threadStateInit(&threadState,
     * THREAD_STATE_FLAGS_NONE) around the work). Without it,
     * threadStateGetCurrent() fails with NV_ERR_OBJECT_NOT_FOUND (0x57), and
     * any RM wait that uses an EXPLICIT timeout (not GPU_TIMEOUT_DEFAULT)
     * aborts on its FIRST gpuCheckTimeout call instead of waiting:
     * timeoutCheck always consults threadStateCheckTimeout in addition to the
     * local timer, and only falls back to the local timer when
     * USE_THREAD_STATE was set (i.e. only for GPU_TIMEOUT_DEFAULT waits).
     * Real-hardware consequence: GspStatusQueueInit's 4-second wait for
     * GSP-RM to link the status queue died at retries=0 with 0x57 whenever
     * GSP-RM was not ALREADY ready on the first poll -- a boot-to-boot race
     * (one boot hit it, the previous one did not).
     */
    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);

    /*
     * Same expanded-visibility requirement as the attach path: the GPU is
     * attached but not yet PDB_PROP_GPU_STATE_INITIALIZED at this point, so
     * gpumgrGetGpu (gpu_mgr.c:500) would return NULL without it. gpuInstance
     * is the value eclipse_rm_attach_gpu returned (it is keyed by
     * gpuInstance, not the device grouping index).
     */
    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return NV_ERR_INVALID_ARGUMENT;
    }

    pKernelGsp = GPU_GET_KERNEL_GSP(pGpu);
    if (pKernelGsp == NULL)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return NV_ERR_NOT_SUPPORTED;
    }

    portMemSet(&gspFw, 0, sizeof(gspFw));
    gspFw.pBuf = pBuf;
    gspFw.size = size;

    /*
     * Guaranteed-visible bracket around the vendored kgspInitRm. ECLIPSE_TRACE
     * goes straight to nv_printf -> log::warn! -> the graphic console, bypassing
     * the RM's LEVEL_ERROR release filter, so this line lands on the monitor
     * synchronously the instant it runs. If the box hard-hangs inside GSP boot
     * (kgspBootstrap polling a FALCON/Booter register that never settles, IRQs
     * off, no timeout possible), the "before kgspInitRm" line is the proof the
     * live console path works and the freeze is inside kgspInitRm; the
     * finer-grained "NVRM: ..." lines that follow (enabled via the kernel_gsp/
     * kernel_falcon RmMsg rule in eclipse_rm_init_core) then pinpoint the exact
     * step. The captured /proc/gpustep6 buffer cannot show any of this on a
     * hang because it is only returned once the read completes.
     */
    ECLIPSE_TRACE("init_gsp: before kgspInitRm (GSP bootstrap begins)");

    /*
     * The GSP bootstrap hard-hangs the whole box in kgspExecuteSequencerCommand
     * _TU102's SEC2 GSP-RM resume: gpuTimeoutCondWait polls a scratch register
     * that never handshakes, and the loop is only bounded if pGpu->timeoutData
     * carries a real timer-source flag (GPU_TIMEOUT_FLAGS_OSTIMER) -- otherwise
     * _checkTimeout returns NV_OK forever and the CPU spins with interrupts off
     * (verified: the box stays frozen indefinitely, never printing the SEC2
     * timeout error). timeoutData is normally armed by timeoutInitializeGpuDefault
     * inside gpuPostConstruct, which Eclipse's minimal attach may not run -- so
     * arm it explicitly here, right before the boot, and narrate the before/after
     * so we can see whether it was uninitialised (defaultFlags == 0) on this box.
     * timeoutInitializeGpuDefault just recomputes from osGetTimeoutParams (which
     * now returns OSTIMER + 4s), so calling it again is harmless if it already ran.
     */
    nv_printf(0, "[eclipse-rm-trace] init_gsp: timeoutData BEFORE arm: defaultFlags=0x%x defaultus=%u\n",
              pGpu->timeoutData.defaultFlags, pGpu->timeoutData.defaultus);
    timeoutInitializeGpuDefault(&pGpu->timeoutData, pGpu);
    nv_printf(0, "[eclipse-rm-trace] init_gsp: timeoutData AFTER  arm: defaultFlags=0x%x defaultus=%u\n",
              pGpu->timeoutData.defaultFlags, pGpu->timeoutData.defaultus);

    /*
     * kgspInitRm's lock contract (matching Linux's RmInitAdapter, which always
     * calls it with the RM API lock held): _kgspBootGspRm's relaxed-init path
     * (default-ON for Unix, _kgspShouldRelaxGspInitLocking) unconditionally
     * does rmapiLockRelease() before kgspBootstrap and rmapiLockAcquire()
     * afterwards (_kgspBootReacquireLocks). Calling kgspInitRm WITHOUT owning
     * the API lock made that release corrupt the rwlock state, and the
     * post-boot reacquire then spun forever inside portSync -- silently: no
     * MMIO (no GSPF probe lines) and no osSpinLoop (no heartbeats). That was
     * the machine freeze right after "GSP FW RM ready." on real hardware,
     * three boots in a row. The GPU locks need no action here: kgspInitRm
     * acquires them itself into gpusLockedMask and releases them at its own
     * exit.
     */
    ECLIPSE_TRACE("init_gsp: acquiring RM API lock (kgspInitRm contract)");
    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    status = kgspInitRm(pGpu, pKernelGsp, &gspFw);
    ECLIPSE_TRACE("init_gsp: after kgspInitRm (GSP bootstrap returned)");
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return status;
}

/*
 * osPackageRegistry -- OUR implementation (the real one lives in the excluded
 * arch/nvalloc/unix/src/registry.c and depends on the Linux nv_state_t
 * registry chain Eclipse does not have). The GSP SET_REGISTRY RPC
 * (rpcSetRegistry_v17_00, kernel_gsp.c:3376 via kgspQueueAsyncInitRpcs) hands
 * GSP-RM the set of driver registry-key overrides. Eclipse sets none, so we
 * emit a well-formed *empty* PACKED_REGISTRY_TABLE (header only, zero
 * entries): GSP-RM reads numEntries == 0 and simply applies its own built-in
 * defaults. We must return NV_OK here rather than the NV_WARN_NOTHING_TO_DO
 * the Linux implementation returns for an empty table, because the RPC caller
 * treats any status != NV_OK as failure and would abort GSP boot.
 */
NV_STATUS osPackageRegistry(
    OBJGPU                *pGpu,
    PACKED_REGISTRY_TABLE *pRegTable,
    NvU32                 *pSize
)
{
    const NvU32 headerSize = (NvU32)NV_OFFSETOF(PACKED_REGISTRY_TABLE, entries);

    (void)pGpu;

    if (pSize == NULL)
        return NV_ERR_INVALID_ARGUMENT;

    /* Sizing pass: report the size of the (empty) table. */
    if (pRegTable == NULL)
    {
        *pSize = headerSize;
        return NV_OK;
    }

    /* Fill pass: caller-provided buffer must hold at least the header. */
    if (*pSize < headerSize)
        return NV_ERR_BUFFER_TOO_SMALL;

    pRegTable->size       = headerSize;
    pRegTable->numEntries = 0;
    *pSize                = headerSize;

    return NV_OK;
}

/*
 * osMapGPU / osUnmapGPU -- OUR implementations (the real ones live in the
 * excluded arch/nvalloc/unix/src/os.c:1281). This was the step-9 gpuStateLoad
 * killer on real hardware: rmapiMapToCpu's ADDR_REGMEM branch
 * (mapping_cpu.c:645) maps a BAR0 register region through osMapGPU, and the
 * old Rust auto-stub had the wrong ABI -- it returned `*mut c_void` NULL
 * (which reads back as NV_STATUS 0 = NV_OK) and never wrote the
 * `NvP64 *pAddress` out-param. During kfifoStatePostLoad's
 * post-scheduling-enable callbacks, the internal CE scrubber/CeUtils channel
 * setup (memmgrScrubMapDoorbellRegion_GV100, mem_mgr_gv100.c) maps the
 * VOLTA_USERMODE_A doorbell region with exactly that path; the "successful"
 * stub left pChannel->pDoorbellRegion NULL, after which
 * pDoorbellRegisterOffset = NULL + NVC361_NOTIFY_CHANNEL_PENDING (0x90) and
 * bUseDoorbellRegister = NV_TRUE. The first CE work submission
 * (channelFillGpFifo, channel_utils.c:581) then did
 * MEM_WR32(pDoorbellRegisterOffset, token) -- the observed deterministic
 * [KERNEL PAGE FAULT] vaddr=0x90 flags=WRITE.
 *
 * Kernel-privilege mappings are, per NVIDIA's own implementation, just an
 * offset into the persistent BAR0 kernel mapping:
 * `portSafeAddU64((NvUPtr)pGpu->deviceMappings[0].gpuNvAddr, offset, pAddress)`
 * -- and Eclipse populates deviceMappings[0].gpuNvAddr with bar0_virt via
 * GPUATTACHARG.regBaseAddr (gpu_mgr.c:1783), so the same one-liner is fully
 * real here. User-privilege mappings (osMapPciMemoryUser on Linux) have no
 * Eclipse equivalent -- no userspace can reach RM yet -- so that branch
 * returns NV_ERR_NOT_SUPPORTED instead of pretending.
 */
NV_STATUS osMapGPU(
    OBJGPU        *pGpu,
    RS_PRIV_LEVEL  privLevel,
    NvU64          offset,
    NvU64          length,
    NvU32          Protect,
    NvP64         *pAddress,
    NvP64         *pPriv
)
{
    NV_STATUS rmStatus = NV_OK;

    (void)length;
    (void)Protect;
    (void)pPriv;

    if (pGpu == NULL || pAddress == NULL)
        return NV_ERR_INVALID_ARGUMENT;

    if (privLevel >= RS_PRIV_LEVEL_KERNEL)
    {
        if (pGpu->deviceMappings[0].gpuNvAddr == NULL)
        {
            rmStatus = NV_ERR_INVALID_STATE;
        }
        else if (!portSafeAddU64((NvUPtr)pGpu->deviceMappings[0].gpuNvAddr,
                                 offset, (NvU64*)pAddress))
        {
            rmStatus = NV_ERR_INVALID_LIMIT;
        }
    }
    else
    {
        rmStatus = NV_ERR_NOT_SUPPORTED;
    }

    return rmStatus;
}

void osUnmapGPU(
    OS_GPU_INFO   *pOsGpuInfo,
    RS_PRIV_LEVEL  privLevel,
    NvP64          address,
    NvU64          length,
    NvP64          priv
)
{
    /*
     * Kernel mappings from osMapGPU above are windows into the persistent
     * BAR0 map -- nothing to unmap, exactly like the real driver (os.c:1322
     * only unmaps for privLevel < RS_PRIV_LEVEL_KERNEL, which osMapGPU here
     * never grants).
     */
    (void)pOsGpuInfo;
    (void)privLevel;
    (void)address;
    (void)length;
    (void)priv;
}

/*
 * Step-7 readback: copy out the GspStaticConfigInfo the live GSP-RM returned
 * during kgspInitRm's GET_GSP_STATIC_INFO RPC. This is data generated BY the
 * firmware (GPU marketing name, VRAM geometry, VBIOS IDs), so displaying it
 * proves the CPU<->GSP RPC channel end-to-end beyond boot. Plain fixed-layout
 * out-struct (mirrored exactly in Rust, rm_init.rs) instead of formatting
 * here, so all string building stays on the Rust side.
 */
typedef struct EclipseGspInfo
{
    NvU8  gpuNameString[64];
    NvU8  gpuShortNameString[64];
    NvU64 fbLength;
    NvU32 fbBusWidth;
    NvU32 fbRamType;
    NvU32 l2CacheSize;
    NvU8  bVbiosValid;
    NvU32 vbiosSubVendor;
    NvU32 vbiosSubDevice;
} EclipseGspInfo;

NV_STATUS eclipse_rm_get_gsp_info(NvU32 gpuInstance, EclipseGspInfo *pInfo)
{
    OBJGPU *pGpu;
    GspStaticConfigInfo *pGSCI;
    NV_STATUS status;

    if (pInfo == NULL)
    {
        return NV_ERR_INVALID_ARGUMENT;
    }

    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        return status;
    }

    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        return NV_ERR_INVALID_ARGUMENT;
    }

    pGSCI = GPU_GET_GSP_STATIC_INFO(pGpu);
    if (pGSCI == NULL)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        return NV_ERR_INVALID_STATE;
    }

    portMemSet(pInfo, 0, sizeof(*pInfo));
    portMemCopy(pInfo->gpuNameString, sizeof(pInfo->gpuNameString) - 1,
                pGSCI->gpuNameString, sizeof(pInfo->gpuNameString) - 1);
    portMemCopy(pInfo->gpuShortNameString, sizeof(pInfo->gpuShortNameString) - 1,
                pGSCI->gpuShortNameString, sizeof(pInfo->gpuShortNameString) - 1);
    pInfo->fbLength       = pGSCI->fb_length;
    pInfo->fbBusWidth     = pGSCI->fb_bus_width;
    pInfo->fbRamType      = pGSCI->fb_ram_type;
    pInfo->l2CacheSize    = pGSCI->l2_cache_size;
    pInfo->bVbiosValid    = pGSCI->bVbiosValid ? 1 : 0;
    pInfo->vbiosSubVendor = pGSCI->vbiosSubVendor;
    pInfo->vbiosSubDevice = pGSCI->vbiosSubDevice;

    gpumgrThreadDisableExpandedGpuVisibility();
    return NV_OK;
}

/*
 * Step-8: real RM API controls served by the live GSP-RM's own resource
 * server. On a GSP client, GPU_GET_PHYSICAL_RMAPI(pGpu)->Control was
 * replaced at attach time (initRpcObject -> rpcRmApiSetup, rpc_common.c)
 * with rpcRmApiControl_GSP -- a direct NV_VGPU_MSG_FUNCTION_GSP_RM_CONTROL
 * RPC that GSP-RM executes against ITS resource-server objects. The client/
 * subdevice handles for that are the GSP-side internal handles GSP-RM
 * advertised in GspStaticConfigInfo -- normally adopted into pGpu by
 * _gpuAllocateInternalObjects during gpuStatePreInit (gpu.c), which
 * Eclipse's minimal bring-up has not run, so adopt them here the same way.
 *
 * Three read-only controls, chosen to be served entirely by the firmware:
 * GPU name string, GPU GID/UUID (GSP computes it), and FB heap total/free
 * (dynamic data straight out of GSP-RM's live heap bookkeeping).
 */
typedef struct EclipseRmApiDemo
{
    NvU32 nameStatus;
    NvU8  name[64];
    NvU32 gidStatus;
    NvU32 gidLength;
    NvU8  gid[136];
    NvU32 fbStatus;
    NvU32 heapSizeKb;
    NvU32 heapFreeKb;
    NvU32 busWidth;
} EclipseRmApiDemo;

NV_STATUS eclipse_rm_step8(NvU32 gpuInstance, EclipseRmApiDemo *pOut)
{
    OBJGPU *pGpu;
    RM_API *pRmApi;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;
    GPU_MASK gpusLockedMask = 0;

    if (pOut == NULL)
    {
        return NV_ERR_INVALID_ARGUMENT;
    }
    portMemSet(pOut, 0, sizeof(*pOut));
    pOut->nameStatus = NV_ERR_NOT_READY;
    pOut->gidStatus  = NV_ERR_NOT_READY;
    pOut->fbStatus   = NV_ERR_NOT_READY;

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);

    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }

    /*
     * Hold the API lock and this GPU's group lock across the RPC controls:
     * rpcWriteCommonHeader asserts rmDeviceGpuLockIsOwner, and
     * rpcRmApiControl_GSP only self-acquires (with a warning) as a fallback.
     */
    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    status = rmGpuGroupLockAcquire(pGpu->gpuInstance, GPU_LOCK_GRP_SUBDEVICE,
                                   GPUS_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT,
                                   &gpusLockedMask);
    if (status != NV_OK)
    {
        rmapiLockRelease();
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    /* Adopt the GSP-side internal handles (gpuStatePreInit's GSP-client
     * branch of _gpuAllocateInternalObjects, which we never ran). */
    if (pGpu->hInternalClient == 0)
    {
        GspStaticConfigInfo *pGSCI = GPU_GET_GSP_STATIC_INFO(pGpu);
        if (pGSCI == NULL || pGSCI->hInternalClient == 0)
        {
            status = NV_ERR_INVALID_STATE;
            goto unlock;
        }
        pGpu->hInternalClient = pGSCI->hInternalClient;
        pGpu->hInternalDevice = pGSCI->hInternalDevice;
        pGpu->hInternalSubdevice = pGSCI->hInternalSubdevice;
        rmapiControlCacheSetGpuAttrForObject(pGpu->hInternalClient,
                                             pGpu->hInternalSubdevice, pGpu);
        rmapiControlCacheSetGpuAttrForObject(pGpu->hInternalClient,
                                             pGpu->hInternalDevice, pGpu);
        nv_printf(0, "[eclipse-rm-trace] step8: adopted GSP internal handles client=0x%x subdevice=0x%x\n",
                  pGpu->hInternalClient, pGpu->hInternalSubdevice);
    }

    pRmApi = GPU_GET_PHYSICAL_RMAPI(pGpu);

    {
        NV2080_CTRL_GPU_GET_NAME_STRING_PARAMS nameParams;
        portMemSet(&nameParams, 0, sizeof(nameParams));
        nameParams.gpuNameStringFlags = NV2080_CTRL_GPU_GET_NAME_STRING_FLAGS_TYPE_ASCII;
        pOut->nameStatus = pRmApi->Control(pRmApi, pGpu->hInternalClient,
                                           pGpu->hInternalSubdevice,
                                           NV2080_CTRL_CMD_GPU_GET_NAME_STRING,
                                           &nameParams, sizeof(nameParams));
        nv_printf(0, "[eclipse-rm-trace] step8: GET_NAME_STRING -> 0x%x\n", pOut->nameStatus);
        if (pOut->nameStatus == NV_OK)
        {
            portMemCopy(pOut->name, sizeof(pOut->name) - 1,
                        nameParams.gpuNameString.ascii, sizeof(pOut->name) - 1);
        }
    }

    {
        NV2080_CTRL_GPU_GET_GID_INFO_PARAMS gidParams;
        portMemSet(&gidParams, 0, sizeof(gidParams));
        gidParams.index = 0;
        gidParams.flags = NV2080_GPU_CMD_GPU_GET_GID_FLAGS_FORMAT_ASCII;
        pOut->gidStatus = pRmApi->Control(pRmApi, pGpu->hInternalClient,
                                          pGpu->hInternalSubdevice,
                                          NV2080_CTRL_CMD_GPU_GET_GID_INFO,
                                          &gidParams, sizeof(gidParams));
        nv_printf(0, "[eclipse-rm-trace] step8: GET_GID_INFO -> 0x%x (len %u)\n",
                  pOut->gidStatus, gidParams.length);
        if (pOut->gidStatus == NV_OK)
        {
            NvU32 n = gidParams.length;
            if (n > sizeof(pOut->gid) - 1)
            {
                n = sizeof(pOut->gid) - 1;
            }
            portMemCopy(pOut->gid, n, gidParams.data, n);
            pOut->gidLength = n;
        }
    }

    {
        NV2080_CTRL_FB_GET_INFO_V2_PARAMS fbParams;
        portMemSet(&fbParams, 0, sizeof(fbParams));
        fbParams.fbInfoListSize = 3;
        fbParams.fbInfoList[0].index = NV2080_CTRL_FB_INFO_INDEX_HEAP_SIZE;
        fbParams.fbInfoList[1].index = NV2080_CTRL_FB_INFO_INDEX_HEAP_FREE;
        fbParams.fbInfoList[2].index = NV2080_CTRL_FB_INFO_INDEX_BUS_WIDTH;
        pOut->fbStatus = pRmApi->Control(pRmApi, pGpu->hInternalClient,
                                         pGpu->hInternalSubdevice,
                                         NV2080_CTRL_CMD_FB_GET_INFO_V2,
                                         &fbParams, sizeof(fbParams));
        nv_printf(0, "[eclipse-rm-trace] step8: FB_GET_INFO_V2 -> 0x%x\n", pOut->fbStatus);
        if (pOut->fbStatus == NV_OK)
        {
            pOut->heapSizeKb = fbParams.fbInfoList[0].data;
            pOut->heapFreeKb = fbParams.fbInfoList[1].data;
            pOut->busWidth   = fbParams.fbInfoList[2].data;
        }
    }

    status = NV_OK;

unlock:
    rmGpuGroupLockRelease(gpusLockedMask, GPUS_LOCK_FLAGS_NONE);
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return status;
}

/*
 * Step-15: probe the graphics/compute (GR) engine's shader config on the
 * state-loaded GPU, over the live GSP's resource server (GSP_RM_CONTROL RPC),
 * the same read-only Control path as step-8. Uses the mask controls
 * (GR_GET_GPC_MASK, GR_GET_TPC_MASK) rather than GR_GET_INFO: the mask params
 * are flat scalars (no embedded pointer list to marshal across the RPC), so
 * this stays low-risk. Turing packs TWO SMs per TPC (Volta+ layout), so the
 * usable SM count is twice the enabled TPC count -- the first real read of the
 * shader array the compute engine will run on. Proves the GR subsystem is live
 * and queryable end-to-end; groundwork for a future real compute launch.
 */
typedef struct EclipseGrProbe
{
    NvU32 gpcMaskStatus;
    NvU32 gpcMask;
    NvU32 numGpc;
    NvU32 tpcMaskStatus; /* first non-OK per-GPC status, else NV_OK */
    NvU32 totalTpc;      /* usable SMs = 2 * totalTpc on Turing (2 SMs/TPC) */
    NvU32 perGpcTpc[8];  /* enabled TPCs per GPC index */
} EclipseGrProbe;

/* Local bit-population count (nvPopCount32 is not part of the vendored public
 * boundary we link against here). */
static NvU32 eclipse_popcount32(NvU32 v)
{
    NvU32 c = 0;
    while (v != 0)
    {
        v &= (v - 1);
        c++;
    }
    return c;
}

NV_STATUS eclipse_rm_step15(NvU32 gpuInstance, EclipseGrProbe *pOut)
{
    OBJGPU *pGpu;
    RM_API *pRmApi;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;
    GPU_MASK gpusLockedMask = 0;

    if (pOut == NULL)
    {
        return NV_ERR_INVALID_ARGUMENT;
    }
    portMemSet(pOut, 0, sizeof(*pOut));
    pOut->gpcMaskStatus = NV_ERR_NOT_READY;
    pOut->tpcMaskStatus = NV_ERR_NOT_READY;

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);

    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }

    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    status = rmGpuGroupLockAcquire(pGpu->gpuInstance, GPU_LOCK_GRP_SUBDEVICE,
                                   GPUS_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT,
                                   &gpusLockedMask);
    if (status != NV_OK)
    {
        rmapiLockRelease();
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    if (pGpu->hInternalClient == 0)
    {
        GspStaticConfigInfo *pGSCI = GPU_GET_GSP_STATIC_INFO(pGpu);
        if (pGSCI == NULL || pGSCI->hInternalClient == 0)
        {
            status = NV_ERR_INVALID_STATE;
            goto unlock;
        }
        pGpu->hInternalClient = pGSCI->hInternalClient;
        pGpu->hInternalDevice = pGSCI->hInternalDevice;
        pGpu->hInternalSubdevice = pGSCI->hInternalSubdevice;
        rmapiControlCacheSetGpuAttrForObject(pGpu->hInternalClient,
                                             pGpu->hInternalSubdevice, pGpu);
        rmapiControlCacheSetGpuAttrForObject(pGpu->hInternalClient,
                                             pGpu->hInternalDevice, pGpu);
    }

    pRmApi = GPU_GET_PHYSICAL_RMAPI(pGpu);

    {
        NV2080_CTRL_GR_GET_GPC_MASK_PARAMS gpcParams;
        portMemSet(&gpcParams, 0, sizeof(gpcParams));
        pOut->gpcMaskStatus = pRmApi->Control(pRmApi, pGpu->hInternalClient,
                                              pGpu->hInternalSubdevice,
                                              NV2080_CTRL_CMD_GR_GET_GPC_MASK,
                                              &gpcParams, sizeof(gpcParams));
        nv_printf(0, "[eclipse-rm-trace] step15: GR_GET_GPC_MASK -> 0x%x mask=0x%x\n",
                  pOut->gpcMaskStatus, gpcParams.gpcMask);
        if (pOut->gpcMaskStatus == NV_OK)
        {
            NvU32 gpcId;
            pOut->gpcMask = gpcParams.gpcMask;
            pOut->numGpc = eclipse_popcount32(gpcParams.gpcMask);
            pOut->tpcMaskStatus = NV_OK;
            for (gpcId = 0; gpcId < 8; gpcId++)
            {
                NV2080_CTRL_GR_GET_TPC_MASK_PARAMS tpcParams;
                NV_STATUS ts;
                if ((gpcParams.gpcMask & NVBIT32(gpcId)) == 0)
                {
                    continue;
                }
                portMemSet(&tpcParams, 0, sizeof(tpcParams));
                tpcParams.gpcId = gpcId;
                ts = pRmApi->Control(pRmApi, pGpu->hInternalClient,
                                     pGpu->hInternalSubdevice,
                                     NV2080_CTRL_CMD_GR_GET_TPC_MASK,
                                     &tpcParams, sizeof(tpcParams));
                nv_printf(0, "[eclipse-rm-trace] step15: GR_GET_TPC_MASK gpc=%u -> 0x%x mask=0x%x\n",
                          gpcId, ts, tpcParams.tpcMask);
                if (ts != NV_OK)
                {
                    pOut->tpcMaskStatus = ts;
                    continue;
                }
                pOut->perGpcTpc[gpcId] = eclipse_popcount32(tpcParams.tpcMask);
                pOut->totalTpc += pOut->perGpcTpc[gpcId];
            }
        }
    }

    status = NV_OK;

unlock:
    rmGpuGroupLockRelease(gpusLockedMask, GPUS_LOCK_FLAGS_NONE);
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return status;
}

/*
 * Interrupt kernel table dump: ask the live GSP-RM for its authoritative
 * vector->engine map via NV2080_CTRL_CMD_INTERNAL_INTR_GET_KERNEL_TABLE --
 * the exact control kernel RM's intrInitInterruptTable uses on a GSP client
 * (intr.c:1044, ROUTE_TO_PHYSICAL|INTERNAL, served by GSP firmware). Each
 * entry names an engine (MC_ENGINE_IDX_*), its legacy PMC mask, and its
 * stall/nonstall vectors in the Turing+ CPU_INTR tree. This settles, from the
 * GPU's own table, which engine owns CPU vector 156 (the LEAF[4] bit28 level
 * source observed pending before the console GPU's SEC2 STARTCPU wedge) and
 * which engine drives legacy PMC_INTR_0 bit 28 (mask 0x10000000). Read-only.
 */
typedef struct EclipseIntrTableEntry
{
    NvU32 engineIdx;
    NvU32 pmcIntrMask;
    NvU32 vectorStall;
    NvU32 vectorNonStall;
} EclipseIntrTableEntry;

typedef struct EclipseIntrTable
{
    NvU32 ctrlStatus;
    NvU32 tableLen;
    EclipseIntrTableEntry entries[NV2080_CTRL_INTERNAL_INTR_MAX_TABLE_SIZE];
} EclipseIntrTable;

NV_STATUS eclipse_rm_intr_table(NvU32 gpuInstance, EclipseIntrTable *pOut)
{
    OBJGPU *pGpu;
    RM_API *pRmApi;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;
    GPU_MASK gpusLockedMask = 0;
    NV2080_CTRL_INTERNAL_INTR_GET_KERNEL_TABLE_PARAMS *pParams = NULL;

    if (pOut == NULL)
    {
        return NV_ERR_INVALID_ARGUMENT;
    }
    portMemSet(pOut, 0, sizeof(*pOut));
    pOut->ctrlStatus = NV_ERR_NOT_READY;

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);

    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }

    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    status = rmGpuGroupLockAcquire(pGpu->gpuInstance, GPU_LOCK_GRP_SUBDEVICE,
                                   GPUS_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT,
                                   &gpusLockedMask);
    if (status != NV_OK)
    {
        rmapiLockRelease();
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    if (pGpu->hInternalClient == 0)
    {
        GspStaticConfigInfo *pGSCI = GPU_GET_GSP_STATIC_INFO(pGpu);
        if (pGSCI == NULL || pGSCI->hInternalClient == 0)
        {
            status = NV_ERR_INVALID_STATE;
            goto unlock;
        }
        pGpu->hInternalClient = pGSCI->hInternalClient;
        pGpu->hInternalDevice = pGSCI->hInternalDevice;
        pGpu->hInternalSubdevice = pGSCI->hInternalSubdevice;
        rmapiControlCacheSetGpuAttrForObject(pGpu->hInternalClient,
                                             pGpu->hInternalSubdevice, pGpu);
        rmapiControlCacheSetGpuAttrForObject(pGpu->hInternalClient,
                                             pGpu->hInternalDevice, pGpu);
    }

    pRmApi = GPU_GET_PHYSICAL_RMAPI(pGpu);

    /* ~2 KiB params: heap-allocate exactly like intrInitInterruptTable does
     * (intr.c:1041, portMemAllocNonPaged) -- too big for comfort on stack. */
    pParams = portMemAllocNonPaged(sizeof(*pParams));
    if (pParams == NULL)
    {
        status = NV_ERR_NO_MEMORY;
        goto unlock;
    }
    portMemSet(pParams, 0, sizeof(*pParams));

    pOut->ctrlStatus = pRmApi->Control(pRmApi, pGpu->hInternalClient,
                                       pGpu->hInternalSubdevice,
                                       NV2080_CTRL_CMD_INTERNAL_INTR_GET_KERNEL_TABLE,
                                       pParams, sizeof(*pParams));
    nv_printf(0, "[eclipse-rm-trace] intr_table: INTR_GET_KERNEL_TABLE -> 0x%x len=%u\n",
              pOut->ctrlStatus, pParams->tableLen);
    if (pOut->ctrlStatus == NV_OK)
    {
        NvU32 i;
        NvU32 n = pParams->tableLen;
        if (n > NV2080_CTRL_INTERNAL_INTR_MAX_TABLE_SIZE)
        {
            n = NV2080_CTRL_INTERNAL_INTR_MAX_TABLE_SIZE;
        }
        pOut->tableLen = n;
        for (i = 0; i < n; i++)
        {
            pOut->entries[i].engineIdx      = pParams->table[i].engineIdx;
            pOut->entries[i].pmcIntrMask    = pParams->table[i].pmcIntrMask;
            pOut->entries[i].vectorStall    = pParams->table[i].vectorStall;
            pOut->entries[i].vectorNonStall = pParams->table[i].vectorNonStall;
        }
    }
    portMemFree(pParams);

    status = NV_OK;

unlock:
    rmGpuGroupLockRelease(gpusLockedMask, GPUS_LOCK_FLAGS_NONE);
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return status;
}

/*
 * Step-16: the graphics/compute allocation ladder -- the first resource-
 * server allocations Eclipse makes itself (everything before this adopted
 * the GSP's internal handles). Allocates, on a state-loaded GPU:
 *
 *   NV01_ROOT client -> NV01_DEVICE_0 -> NV20_SUBDEVICE_0 ->
 *   FERMI_VASPACE_A -> KEPLER_CHANNEL_GROUP_A (TSG, engineType GRAPHICS) ->
 *   FERMI_CONTEXT_SHARE_A (SYNC subcontext)
 *
 * This is the exact front half of what any user-mode driver does to run
 * compute, and every step exercises real vendored RM machinery against the
 * live GSP (engine validation, VAS construction, TSG runlist selection,
 * subcontext/VEID bookkeeping). The recipe (rmapiGetInterface with
 * RMAPI_GPU_LOCK_INTERNAL under the API lock only, client handle generator
 * setup, clientGenResourceHandle for child handles) is transliterated from
 * channelAllocSubdevice/ceutilsConstruct (channel_utils.c/ce_utils.c) --
 * the same code that already ran successfully in THIS environment when
 * step-9's state-load constructed CeUtils. Handles are kept alive and
 * cached in statics for step-17 (GPFIFO channel + TURING_COMPUTE_A object
 * -> golden-context creation); a repeat call returns the cached result so
 * the /proc read is idempotent. On any failure the client (and thus the
 * whole child tree) is freed and the per-stage NV_STATUS pinpoints the
 * failing allocation. 0xFFFFFFFF = stage not reached.
 */
typedef struct EclipseGrAlloc
{
    NvU32 clientStatus;
    NvU32 deviceStatus;
    NvU32 subdevStatus;
    NvU32 vasStatus;
    NvU32 tsgStatus;
    NvU32 ctxshareStatus;
    NvU32 hClient;
    NvU32 hDevice;
    NvU32 hSubdevice;
    NvU32 hVas;
    NvU32 hTsg;
    NvU32 hCtxShare;
} EclipseGrAlloc;

static EclipseGrAlloc g_grAllocCache;
static NvBool g_grAllocDone = NV_FALSE;

NV_STATUS eclipse_rm_step16(NvU32 gpuInstance, EclipseGrAlloc *pOut)
{
    OBJGPU *pGpu;
    RM_API *pRmApi;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;
    RsClient *pRsClient = NULL;

    if (pOut == NULL)
    {
        return NV_ERR_INVALID_ARGUMENT;
    }
    if (g_grAllocDone)
    {
        portMemCopy(pOut, sizeof(*pOut), &g_grAllocCache, sizeof(g_grAllocCache));
        return NV_OK;
    }
    portMemSet(pOut, 0, sizeof(*pOut));
    pOut->clientStatus   = 0xFFFFFFFF;
    pOut->deviceStatus   = 0xFFFFFFFF;
    pOut->subdevStatus   = 0xFFFFFFFF;
    pOut->vasStatus      = 0xFFFFFFFF;
    pOut->tsgStatus      = 0xFFFFFFFF;
    pOut->ctxshareStatus = 0xFFFFFFFF;

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);

    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }

    /*
     * API lock ONLY, exactly like eclipse_rm_state_init: the allocation
     * path (RMAPI_GPU_LOCK_INTERNAL) acquires GPU locks itself per call --
     * this is the lock state CeUtils' own AllocWithHandle sequence ran
     * under during step-9's state-load in this environment.
     */
    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pRmApi = rmapiGetInterface(RMAPI_GPU_LOCK_INTERNAL);

    /* 1. Client (RM assigns the handle). */
    pOut->hClient = NV01_NULL_OBJECT;
    pOut->clientStatus = pRmApi->AllocWithHandle(pRmApi, NV01_NULL_OBJECT,
                                                 NV01_NULL_OBJECT, NV01_NULL_OBJECT,
                                                 NV01_ROOT, &pOut->hClient,
                                                 sizeof(pOut->hClient));
    nv_printf(0, "[eclipse-rm-trace] step16: NV01_ROOT client -> 0x%x hClient=0x%x\n",
              pOut->clientStatus, pOut->hClient);
    if (pOut->clientStatus != NV_OK)
    {
        status = NV_OK; /* per-stage status carries the failure */
        goto unlock;
    }

    status = serverGetClientUnderLock(&g_resServ, pOut->hClient, &pRsClient);
    if (status != NV_OK)
    {
        goto free_client;
    }
    status = clientSetHandleGenerator(pRsClient, 1U, ~0U - 1U);
    if (status != NV_OK)
    {
        goto free_client;
    }

    /* 2. Device. */
    {
        NV0080_ALLOC_PARAMETERS params;
        portMemSet(&params, 0, sizeof(params));
        params.deviceId = gpuGetDeviceInstance(pGpu);
        params.hClientShare = pOut->hClient;
        status = clientGenResourceHandle(pRsClient, &pOut->hDevice);
        if (status != NV_OK)
        {
            goto free_client;
        }
        pOut->deviceStatus = pRmApi->AllocWithHandle(pRmApi, pOut->hClient,
                                                     pOut->hClient, pOut->hDevice,
                                                     NV01_DEVICE_0,
                                                     &params, sizeof(params));
        nv_printf(0, "[eclipse-rm-trace] step16: NV01_DEVICE_0 -> 0x%x hDevice=0x%x\n",
                  pOut->deviceStatus, pOut->hDevice);
        if (pOut->deviceStatus != NV_OK)
        {
            status = NV_OK;
            goto free_client;
        }
    }

    /* 3. Subdevice. */
    {
        NV2080_ALLOC_PARAMETERS params;
        portMemSet(&params, 0, sizeof(params));
        params.subDeviceId = gpumgrGetSubDeviceInstanceFromGpu(pGpu);
        status = clientGenResourceHandle(pRsClient, &pOut->hSubdevice);
        if (status != NV_OK)
        {
            goto free_client;
        }
        pOut->subdevStatus = pRmApi->AllocWithHandle(pRmApi, pOut->hClient,
                                                     pOut->hDevice, pOut->hSubdevice,
                                                     NV20_SUBDEVICE_0,
                                                     &params, sizeof(params));
        nv_printf(0, "[eclipse-rm-trace] step16: NV20_SUBDEVICE_0 -> 0x%x hSubdevice=0x%x\n",
                  pOut->subdevStatus, pOut->hSubdevice);
        if (pOut->subdevStatus != NV_OK)
        {
            status = NV_OK;
            goto free_client;
        }
    }

    /* 4. VA space (fresh, default geometry). */
    {
        NV_VASPACE_ALLOCATION_PARAMETERS params;
        portMemSet(&params, 0, sizeof(params));
        params.index = NV_VASPACE_ALLOCATION_INDEX_GPU_NEW;
        status = clientGenResourceHandle(pRsClient, &pOut->hVas);
        if (status != NV_OK)
        {
            goto free_client;
        }
        pOut->vasStatus = pRmApi->AllocWithHandle(pRmApi, pOut->hClient,
                                                  pOut->hDevice, pOut->hVas,
                                                  FERMI_VASPACE_A,
                                                  &params, sizeof(params));
        nv_printf(0, "[eclipse-rm-trace] step16: FERMI_VASPACE_A -> 0x%x hVas=0x%x\n",
                  pOut->vasStatus, pOut->hVas);
        if (pOut->vasStatus != NV_OK)
        {
            status = NV_OK;
            goto free_client;
        }
    }

    /* 5. TSG bound to the GRAPHICS engine -- the first GR-side allocation. */
    {
        NV_CHANNEL_GROUP_ALLOCATION_PARAMETERS params;
        portMemSet(&params, 0, sizeof(params));
        params.hVASpace = pOut->hVas;
        params.engineType = NV2080_ENGINE_TYPE_GRAPHICS;
        status = clientGenResourceHandle(pRsClient, &pOut->hTsg);
        if (status != NV_OK)
        {
            goto free_client;
        }
        pOut->tsgStatus = pRmApi->AllocWithHandle(pRmApi, pOut->hClient,
                                                  pOut->hDevice, pOut->hTsg,
                                                  KEPLER_CHANNEL_GROUP_A,
                                                  &params, sizeof(params));
        nv_printf(0, "[eclipse-rm-trace] step16: KEPLER_CHANNEL_GROUP_A(GR) -> 0x%x hTsg=0x%x\n",
                  pOut->tsgStatus, pOut->hTsg);
        if (pOut->tsgStatus != NV_OK)
        {
            status = NV_OK;
            goto free_client;
        }
    }

    /* 6. Context share (SYNC subcontext / VEID 0) on the TSG. */
    {
        NV_CTXSHARE_ALLOCATION_PARAMETERS params;
        portMemSet(&params, 0, sizeof(params));
        params.hVASpace = pOut->hVas;
        params.flags = NV_CTXSHARE_ALLOCATION_FLAGS_SUBCONTEXT_SYNC;
        status = clientGenResourceHandle(pRsClient, &pOut->hCtxShare);
        if (status != NV_OK)
        {
            goto free_client;
        }
        pOut->ctxshareStatus = pRmApi->AllocWithHandle(pRmApi, pOut->hClient,
                                                       pOut->hTsg, pOut->hCtxShare,
                                                       FERMI_CONTEXT_SHARE_A,
                                                       &params, sizeof(params));
        nv_printf(0, "[eclipse-rm-trace] step16: FERMI_CONTEXT_SHARE_A -> 0x%x hCtxShare=0x%x\n",
                  pOut->ctxshareStatus, pOut->hCtxShare);
        if (pOut->ctxshareStatus != NV_OK)
        {
            status = NV_OK;
            goto free_client;
        }
    }

    /* Full ladder allocated: keep it alive for step-17 and cache the result. */
    portMemCopy(&g_grAllocCache, sizeof(g_grAllocCache), pOut, sizeof(*pOut));
    g_grAllocDone = NV_TRUE;
    status = NV_OK;
    goto unlock;

free_client:
    /* Freeing the client tears down the whole child tree. */
    if (pOut->hClient != NV01_NULL_OBJECT)
    {
        pRmApi->Free(pRmApi, pOut->hClient, pOut->hClient);
    }

unlock:
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return status;
}

/*
 * Step-17: GPFIFO channel + TURING_COMPUTE_A on the step-16 ladder -- the
 * back half of a compute-capable channel. Builds on the cached step-16
 * handles (client/device/subdevice/VAS/TSG/ctxshare, kept alive):
 *
 *   USERD (vidmem, kfifoGetUserdSizeAlign) ->
 *   64 KiB sysmem buffer (future pushbuffer + GPFIFO ring) ->
 *   64 KiB virtual alloc in OUR VA space -> Map (GPU VA) ->
 *   4 KiB sysmem error notifier ->
 *   GPFIFO channel (kfifoGetChannelClassId => TURING_CHANNEL_GPFIFO_A on
 *   TU106) allocated INSIDE the TSG with our ctxshare ->
 *   TURING_COMPUTE_A object on the channel (binds the compute class to the
 *   channel's engine context; the golden-context machinery this exercises
 *   already ran on this silicon during state-load, per the step-9 narration:
 *   kgrctxMapGlobalCtxBuffer + "Class 0xc597 allocated") ->
 *   NVA06F_CTRL_CMD_GPFIFO_SCHEDULE (bEnable=TRUE): channel on the runlist.
 *
 * Memory/USERD recipes transliterated from the in-environment-proven CeUtils
 * path (_memUtilsAllocateUserD / _memUtilsAllocateReductionSema,
 * mem_utils_gm107.c): same owner/type/attr/flags, same Alloc->Alloc->Map
 * ordering, RMAPI_GPU_LOCK_INTERNAL under the API lock only (USERD alloc
 * itself asserts GPU locks are NOT held -- our lock state exactly).
 * After this step the channel is schedulable: step-18 writes a QMD + SASS
 * kernel into the pushbuffer, bumps GP_PUT and rings the doorbell -- the
 * first Eclipse-authored compute launch. Per-stage NV_STATUS + handles
 * reported; 0xFFFFFFFF = not reached. Idempotent via cache. On a stage
 * failure the NEW handles are freed (the step-16 ladder stays alive) and
 * the failing stage pinpoints itself.
 */
typedef struct EclipseGrChannel
{
    NvU32 userdStatus;
    NvU32 bufStatus;
    NvU32 virtStatus;
    NvU32 mapStatus;
    NvU32 notifStatus;
    NvU32 chanStatus;
    NvU32 computeStatus;
    NvU32 schedStatus;
    NvU32 hUserd;
    NvU32 hPhysBuf;
    NvU32 hVirtBuf;
    NvU32 hNotifier;
    NvU32 hChannel;
    NvU32 hCompute;
    NvU32 channelClass;
    NvU32 userdSize;
    NvU64 bufGpuVA;
} EclipseGrChannel;

static EclipseGrChannel g_grChanCache;
static NvBool g_grChanDone = NV_FALSE;

#define ECLIPSE_CHAN_BUF_SIZE    0x10000  /* 64 KiB: pushbuffer + ring */
#define ECLIPSE_CHAN_GPFIFO_OFF  0xC000   /* ring lives at +48 KiB */
#define ECLIPSE_CHAN_GPFIFO_ENTRIES 128   /* 128 * 8B = 1 KiB */

NV_STATUS eclipse_rm_step17(NvU32 gpuInstance, EclipseGrChannel *pOut)
{
    OBJGPU *pGpu;
    RM_API *pRmApi;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;
    RsClient *pRsClient = NULL;
    NvBool failed = NV_FALSE;

    if (pOut == NULL)
    {
        return NV_ERR_INVALID_ARGUMENT;
    }
    if (g_grChanDone)
    {
        portMemCopy(pOut, sizeof(*pOut), &g_grChanCache, sizeof(g_grChanCache));
        return NV_OK;
    }
    portMemSet(pOut, 0, sizeof(*pOut));
    pOut->userdStatus   = 0xFFFFFFFF;
    pOut->bufStatus     = 0xFFFFFFFF;
    pOut->virtStatus    = 0xFFFFFFFF;
    pOut->mapStatus     = 0xFFFFFFFF;
    pOut->notifStatus   = 0xFFFFFFFF;
    pOut->chanStatus    = 0xFFFFFFFF;
    pOut->computeStatus = 0xFFFFFFFF;
    pOut->schedStatus   = 0xFFFFFFFF;

    if (!g_grAllocDone)
    {
        return NV_ERR_INVALID_STATE; /* run step16 first */
    }

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);
    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }
    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pRmApi = rmapiGetInterface(RMAPI_GPU_LOCK_INTERNAL);
    status = serverGetClientUnderLock(&g_resServ, g_grAllocCache.hClient, &pRsClient);
    if (status != NV_OK)
    {
        goto unlock;
    }

    /* 1. USERD in vidmem (Volta+ requires client-allocated USERD). */
    {
        NV_MEMORY_ALLOCATION_PARAMS params;
        KernelFifo *pKernelFifo = GPU_GET_KERNEL_FIFO(pGpu);
        portMemSet(&params, 0, sizeof(params));
        params.owner = HEAP_OWNER_RM_CLIENT_GENERIC;
        kfifoGetUserdSizeAlign_HAL(pKernelFifo, &pOut->userdSize, NULL);
        params.size = pOut->userdSize;
        params.type = NVOS32_TYPE_IMAGE;
        params.internalflags = NVOS32_ALLOC_INTERNAL_FLAGS_SKIP_SCRUB;
        params.attr = DRF_DEF(OS32, _ATTR, _LOCATION, _VIDMEM) |
                      DRF_DEF(OS32, _ATTR, _ALLOCATE_FROM_RESERVED_HEAP, _YES);
        params.flags = NVOS32_ALLOC_FLAGS_PERSISTENT_VIDMEM;
        status = clientGenResourceHandle(pRsClient, &pOut->hUserd);
        if (status != NV_OK) goto unlock;
        pOut->userdStatus = pRmApi->AllocWithHandle(pRmApi, g_grAllocCache.hClient,
                                                    g_grAllocCache.hDevice, pOut->hUserd,
                                                    NV01_MEMORY_LOCAL_USER,
                                                    &params, sizeof(params));
        nv_printf(0, "[eclipse-rm-trace] step17: USERD vidmem (%u B) -> 0x%x hUserd=0x%x\n",
                  pOut->userdSize, pOut->userdStatus, pOut->hUserd);
        if (pOut->userdStatus != NV_OK) { failed = NV_TRUE; goto done; }
    }

    /* 2. 64 KiB sysmem buffer (pushbuffer area + GPFIFO ring). */
    {
        NV_MEMORY_ALLOCATION_PARAMS params;
        portMemSet(&params, 0, sizeof(params));
        params.owner = HEAP_OWNER_RM_CLIENT_GENERIC;
        params.type = NVOS32_TYPE_IMAGE;
        params.size = ECLIPSE_CHAN_BUF_SIZE;
        params.attr = DRF_DEF(OS32, _ATTR, _LOCATION, _PCI);
        params.attr2 = NVOS32_ATTR2_NONE;
        status = clientGenResourceHandle(pRsClient, &pOut->hPhysBuf);
        if (status != NV_OK) goto unlock;
        pOut->bufStatus = pRmApi->AllocWithHandle(pRmApi, g_grAllocCache.hClient,
                                                  g_grAllocCache.hDevice, pOut->hPhysBuf,
                                                  NV01_MEMORY_SYSTEM,
                                                  &params, sizeof(params));
        nv_printf(0, "[eclipse-rm-trace] step17: sysmem buf 64K -> 0x%x hPhysBuf=0x%x\n",
                  pOut->bufStatus, pOut->hPhysBuf);
        if (pOut->bufStatus != NV_OK) { failed = NV_TRUE; goto done; }
    }

    /* 3. Matching virtual range in OUR VA space. */
    {
        NV_MEMORY_ALLOCATION_PARAMS params;
        portMemSet(&params, 0, sizeof(params));
        params.owner = HEAP_OWNER_RM_CLIENT_GENERIC;
        params.type = NVOS32_TYPE_IMAGE;
        params.size = ECLIPSE_CHAN_BUF_SIZE;
        params.attr = DRF_DEF(OS32, _ATTR, _LOCATION, _PCI);
        params.attr2 = NVOS32_ATTR2_NONE;
        params.flags = NVOS32_ALLOC_FLAGS_VIRTUAL;
        params.hVASpace = g_grAllocCache.hVas;
        status = clientGenResourceHandle(pRsClient, &pOut->hVirtBuf);
        if (status != NV_OK) goto unlock;
        pOut->virtStatus = pRmApi->AllocWithHandle(pRmApi, g_grAllocCache.hClient,
                                                   g_grAllocCache.hDevice, pOut->hVirtBuf,
                                                   NV50_MEMORY_VIRTUAL,
                                                   &params, sizeof(params));
        nv_printf(0, "[eclipse-rm-trace] step17: virtual 64K in hVas -> 0x%x hVirtBuf=0x%x\n",
                  pOut->virtStatus, pOut->hVirtBuf);
        if (pOut->virtStatus != NV_OK) { failed = NV_TRUE; goto done; }
    }

    /* 4. Map physical into virtual: the buffer's GPU VA. */
    {
        pOut->mapStatus = pRmApi->Map(pRmApi, g_grAllocCache.hClient,
                                      g_grAllocCache.hDevice,
                                      pOut->hVirtBuf, pOut->hPhysBuf,
                                      0, ECLIPSE_CHAN_BUF_SIZE,
                                      NV04_MAP_MEMORY_FLAGS_NONE,
                                      &pOut->bufGpuVA);
        nv_printf(0, "[eclipse-rm-trace] step17: Map -> 0x%x GPU VA=0x%llx\n",
                  pOut->mapStatus, pOut->bufGpuVA);
        if (pOut->mapStatus != NV_OK) { failed = NV_TRUE; goto done; }
    }

    /* 5. 4 KiB sysmem error notifier. */
    {
        NV_MEMORY_ALLOCATION_PARAMS params;
        portMemSet(&params, 0, sizeof(params));
        params.owner = HEAP_OWNER_RM_CLIENT_GENERIC;
        params.type = NVOS32_TYPE_IMAGE;
        params.size = 0x1000;
        params.attr = DRF_DEF(OS32, _ATTR, _LOCATION, _PCI);
        params.attr2 = NVOS32_ATTR2_NONE;
        status = clientGenResourceHandle(pRsClient, &pOut->hNotifier);
        if (status != NV_OK) goto unlock;
        pOut->notifStatus = pRmApi->AllocWithHandle(pRmApi, g_grAllocCache.hClient,
                                                    g_grAllocCache.hDevice, pOut->hNotifier,
                                                    NV01_MEMORY_SYSTEM,
                                                    &params, sizeof(params));
        nv_printf(0, "[eclipse-rm-trace] step17: notifier 4K -> 0x%x hNotifier=0x%x\n",
                  pOut->notifStatus, pOut->hNotifier);
        if (pOut->notifStatus != NV_OK) { failed = NV_TRUE; goto done; }
    }

    /* 6. The GPFIFO channel, inside the TSG, with our context share. */
    {
        NV_CHANNEL_ALLOC_PARAMS params;
        pOut->channelClass = kfifoGetChannelClassId(pGpu, GPU_GET_KERNEL_FIFO(pGpu));
        portMemSet(&params, 0, sizeof(params));
        params.hObjectError  = pOut->hNotifier;
        params.hObjectBuffer = pOut->hPhysBuf;
        params.gpFifoOffset  = pOut->bufGpuVA + ECLIPSE_CHAN_GPFIFO_OFF;
        params.gpFifoEntries = ECLIPSE_CHAN_GPFIFO_ENTRIES;
        params.hContextShare = g_grAllocCache.hCtxShare;
        /* hVASpace MUST be null when hContextShare is given: the channel
         * inherits the VAS from the context share (which carries hVas).
         * Real hardware said it verbatim: "kchannelConstruct_IMPL: Both
         * context share and vaspace handles can't be valid at the same
         * time" (NV_ERR_INVALID_ARGUMENT 0x1f). */
        params.hVASpace      = NV01_NULL_OBJECT;
        params.hUserdMemory[0] = pOut->hUserd;
        params.userdOffset[0]  = 0;
        params.engineType    = NV2080_ENGINE_TYPE_GRAPHICS;
        params.flags         = DRF_DEF(OS04, _FLAGS, _CHANNEL_SKIP_SCRUBBER, _TRUE);
        status = clientGenResourceHandle(pRsClient, &pOut->hChannel);
        if (status != NV_OK) goto unlock;
        pOut->chanStatus = pRmApi->AllocWithHandle(pRmApi, g_grAllocCache.hClient,
                                                   g_grAllocCache.hTsg, pOut->hChannel,
                                                   pOut->channelClass,
                                                   &params, sizeof(params));
        nv_printf(0, "[eclipse-rm-trace] step17: GPFIFO channel class=0x%x -> 0x%x hChannel=0x%x\n",
                  pOut->channelClass, pOut->chanStatus, pOut->hChannel);
        if (pOut->chanStatus != NV_OK) { failed = NV_TRUE; goto done; }
    }

    /* 7. TURING_COMPUTE_A on the channel (engine-context class bind). */
    {
        NV_GR_ALLOCATION_PARAMETERS params;
        portMemSet(&params, 0, sizeof(params));
        params.version = 2;
        params.size = sizeof(params);
        status = clientGenResourceHandle(pRsClient, &pOut->hCompute);
        if (status != NV_OK) goto unlock;
        pOut->computeStatus = pRmApi->AllocWithHandle(pRmApi, g_grAllocCache.hClient,
                                                      pOut->hChannel, pOut->hCompute,
                                                      TURING_COMPUTE_A,
                                                      &params, sizeof(params));
        nv_printf(0, "[eclipse-rm-trace] step17: TURING_COMPUTE_A -> 0x%x hCompute=0x%x\n",
                  pOut->computeStatus, pOut->hCompute);
        if (pOut->computeStatus != NV_OK) { failed = NV_TRUE; goto done; }
    }

    /* 8. Put the channel on the runlist. */
    {
        NVA06F_CTRL_GPFIFO_SCHEDULE_PARAMS params;
        portMemSet(&params, 0, sizeof(params));
        params.bEnable = NV_TRUE;
        pOut->schedStatus = pRmApi->Control(pRmApi, g_grAllocCache.hClient,
                                            pOut->hChannel,
                                            NVA06F_CTRL_CMD_GPFIFO_SCHEDULE,
                                            &params, sizeof(params));
        nv_printf(0, "[eclipse-rm-trace] step17: GPFIFO_SCHEDULE -> 0x%x\n",
                  pOut->schedStatus);
        if (pOut->schedStatus != NV_OK) { failed = NV_TRUE; goto done; }
    }

    portMemCopy(&g_grChanCache, sizeof(g_grChanCache), pOut, sizeof(*pOut));
    g_grChanDone = NV_TRUE;
    status = NV_OK;
    goto unlock;

done:
    if (failed)
    {
        /* Free the NEW handles (reverse order); the step-16 ladder stays. */
        if (pOut->hCompute != 0)
            pRmApi->Free(pRmApi, g_grAllocCache.hClient, pOut->hCompute);
        if (pOut->hChannel != 0)
            pRmApi->Free(pRmApi, g_grAllocCache.hClient, pOut->hChannel);
        if (pOut->hNotifier != 0)
            pRmApi->Free(pRmApi, g_grAllocCache.hClient, pOut->hNotifier);
        if (pOut->hVirtBuf != 0)
            pRmApi->Free(pRmApi, g_grAllocCache.hClient, pOut->hVirtBuf);
        if (pOut->hPhysBuf != 0)
            pRmApi->Free(pRmApi, g_grAllocCache.hClient, pOut->hPhysBuf);
        if (pOut->hUserd != 0)
            pRmApi->Free(pRmApi, g_grAllocCache.hClient, pOut->hUserd);
    }
    status = NV_OK; /* per-stage statuses carry the failure */

unlock:
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return status;
}

/*
 * Step-18: the first Eclipse-authored GPU execution. Everything before this
 * point allocated and scheduled; nothing ever made the GPU *fetch and run*
 * methods we wrote. This step does, on the live step-17 channel:
 *
 *   CPU-map the 64 KiB channel buffer (sysmem) and the USERD (vidmem via
 *   BAR1, exactly channelFillGpFifo's memmgrMemDescBeginTransfer recipe) ->
 *   write a method stream at +0x0:
 *     - NVC46F host semaphore RELEASE (SEM_ADDR/PAYLOAD/EXECUTE) to
 *       GPU VA buf+0x8000: proves ESCHED/PBDMA fetched and executed our
 *       pushbuffer (no engine involved), payload 0x0EC11B5E
 *     - SET_OBJECT(subch 1) = TURING_COMPUTE_A, then NVC5C0
 *       SET_REPORT_SEMAPHORE_A..D RELEASE to GPU VA buf+0x8040: proves the
 *       GR/compute engine context-switched into OUR channel (golden context
 *       load) and processed class methods, payload 0x600DC0DE
 *   -> GP_ENTRY0/1 (NV906F format) at ring slot 0 (buf+0xC000) ->
 *   USERD->GPPut = 1 -> work-submit token (kfifoGenerateWorkSubmitToken,
 *   runlistId | chId per kernel_fifo_tu102.c) -> usermode doorbell
 *   (kfifoUpdateUsermodeDoorbell_HAL = NV_VIRTUAL_FUNCTION_DOORBELL write)
 *   -> CPU-poll both semaphores in the sysmem buffer.
 *
 * Every piece is the RM's own submit path (channel_utils.c channelFillGpFifo
 * / channelPushMethod encoding), transliterated for our raw RMAPI channel.
 * Both semaphore observations are reported with poll iteration counts, so a
 * hardware run distinguishes "PBDMA never fetched" (host sem timeout) from
 * "engine never switched in" (host OK, engine timeout). Idempotent via
 * cache; requires step17 in the same boot.
 */
typedef struct EclipseGrLaunch
{
    NvU32 lookupStatus;   /* KernelChannel + memdescs located */
    NvU32 mapStatus;      /* CPU mappings of channel buffer + USERD */
    NvU32 tokenStatus;    /* work-submit token generation */
    NvU32 submitStatus;   /* methods + GP entry + GPPut + doorbell */
    NvU32 hostSemStatus;  /* NV_OK = host semaphore landed, 0x65 = timeout */
    NvU32 engSemStatus;   /* NV_OK = compute report semaphore landed */
    NvU32 workToken;
    NvU32 runlistId;
    NvU32 hostSemValue;   /* CPU readback (expect 0x0EC11B5E) */
    NvU32 engSemValue;    /* CPU readback (expect 0x600DC0DE) */
    NvU32 hostPollIters;  /* 1 ms units until seen */
    NvU32 engPollIters;
    NvU32 pushDwords;     /* method stream length actually submitted */
} EclipseGrLaunch;

static EclipseGrLaunch g_grLaunchCache;
static NvBool g_grLaunchDone = NV_FALSE;

#define ECLIPSE_LAUNCH_HOST_SEM_OFF 0x8000
#define ECLIPSE_LAUNCH_ENG_SEM_OFF  0x8040
#define ECLIPSE_LAUNCH_HOST_PAYLOAD 0x0EC11B5E
#define ECLIPSE_LAUNCH_ENG_PAYLOAD  0x600DC0DE
#define ECLIPSE_LAUNCH_POLL_MS      3000

#define ECLIPSE_PUSH_HDR(subch, mthd, count)                       \
    (DRF_DEF(906F, _DMA, _SEC_OP, _INC_METHOD) |                   \
     DRF_NUM(906F, _DMA, _METHOD_ADDRESS, (mthd) >> 2) |           \
     DRF_NUM(906F, _DMA, _METHOD_SUBCHANNEL, (subch)) |            \
     DRF_NUM(906F, _DMA, _METHOD_COUNT, (count)))

/* Eclipse-side (os_interface.rs) calibrated microsecond delay. */
extern NV_STATUS os_delay_us(NvU32 microseconds);

NV_STATUS eclipse_rm_step18(NvU32 gpuInstance, EclipseGrLaunch *pOut)
{
    OBJGPU *pGpu;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;
    RsClient *pRsClient = NULL;
    KernelChannel *pKernelChannel = NULL;
    KernelFifo *pKernelFifo;
    MemoryManager *pMemoryManager;
    Memory *pBufMemory = NULL;
    MEMORY_DESCRIPTOR *pBufMemDesc = NULL;
    MEMORY_DESCRIPTOR *pUserdMemDesc = NULL;
    NvU8 *pBufCpu = NULL;
    NvU8 *pUserdCpu = NULL;
    NvU32 userdFlags = TRANSFER_FLAGS_USE_BAR1 |
                       TRANSFER_FLAGS_SHADOW_ALLOC |
                       TRANSFER_FLAGS_SHADOW_INIT_MEM;

    if (pOut == NULL)
    {
        return NV_ERR_INVALID_ARGUMENT;
    }
    if (g_grLaunchDone)
    {
        portMemCopy(pOut, sizeof(*pOut), &g_grLaunchCache, sizeof(g_grLaunchCache));
        return NV_OK;
    }
    portMemSet(pOut, 0, sizeof(*pOut));
    pOut->lookupStatus  = 0xFFFFFFFF;
    pOut->mapStatus     = 0xFFFFFFFF;
    pOut->tokenStatus   = 0xFFFFFFFF;
    pOut->submitStatus  = 0xFFFFFFFF;
    pOut->hostSemStatus = 0xFFFFFFFF;
    pOut->engSemStatus  = 0xFFFFFFFF;

    if (!g_grChanDone)
    {
        return NV_ERR_INVALID_STATE; /* run step17 first */
    }

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);
    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }
    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    /*
     * Unlike step-16/17 (allocation, which asserts the GPU lock is NOT
     * held), step-18 does no allocation but DOES CPU-map the USERD out of
     * vidmem via the FB aperture (kbusMapFbAperture_HAL), which asserts
     * rmDeviceGpuLockIsOwner. So we additionally hold the GPU lock across
     * the whole map/submit/poll -- exactly like RM's own CeUtils submit
     * path. API lock first, then GPU locks (standard RM lock order).
     */
    status = rmGpuLocksAcquire(GPUS_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        rmapiLockRelease();
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pKernelFifo = GPU_GET_KERNEL_FIFO(pGpu);
    pMemoryManager = GPU_GET_MEMORY_MANAGER(pGpu);

    /* 1. Locate the live objects behind the step-17 handles. */
    {
        NvU32 subdevInst;
        status = serverGetClientUnderLock(&g_resServ, g_grAllocCache.hClient, &pRsClient);
        if (status == NV_OK)
            status = CliGetKernelChannel(pRsClient, g_grChanCache.hChannel, &pKernelChannel);
        if (status == NV_OK)
            status = memGetByHandle(pRsClient, g_grChanCache.hPhysBuf, &pBufMemory);
        if (status == NV_OK)
        {
            pBufMemDesc = pBufMemory->pMemDesc;
            subdevInst = gpumgrGetSubDeviceInstanceFromGpu(pGpu);
            pUserdMemDesc = pKernelChannel->pUserdSubDeviceMemDesc[subdevInst];
            if (pBufMemDesc == NULL || pUserdMemDesc == NULL)
                status = NV_ERR_INVALID_STATE;
        }
        pOut->lookupStatus = status;
        nv_printf(0, "[eclipse-rm-trace] step18: lookup (chan/buf/userd) -> 0x%x\n",
                  pOut->lookupStatus);
        if (status != NV_OK) goto report;
    }

    /* 2. CPU mappings: sysmem buffer direct, USERD via BAR1 (channel_utils
     *    channelFillGpFifo's exact transfer flags). */
    {
        pBufCpu = memmgrMemDescBeginTransfer(pMemoryManager, pBufMemDesc,
                                             TRANSFER_FLAGS_NONE);
        pUserdCpu = memmgrMemDescBeginTransfer(pMemoryManager, pUserdMemDesc,
                                               userdFlags);
        pOut->mapStatus = (pBufCpu != NULL && pUserdCpu != NULL)
                              ? NV_OK : NV_ERR_GENERIC;
        nv_printf(0, "[eclipse-rm-trace] step18: CPU map buf=%s userd=%s -> 0x%x\n",
                  pBufCpu ? "ok" : "NULL", pUserdCpu ? "ok" : "NULL",
                  pOut->mapStatus);
        if (pOut->mapStatus != NV_OK) goto report;
    }

    /* 3. Work-submit token (kernel_fifo_tu102: runlistId | chId). */
    {
        pOut->tokenStatus = kfifoGenerateWorkSubmitToken(pGpu, pKernelFifo,
                                                         pKernelChannel,
                                                         &pOut->workToken,
                                                         NV_TRUE);
        pOut->runlistId = kchannelGetRunlistId(pKernelChannel);
        nv_printf(0, "[eclipse-rm-trace] step18: work token -> 0x%x token=0x%x runlist=%u\n",
                  pOut->tokenStatus, pOut->workToken, pOut->runlistId);
        if (pOut->tokenStatus != NV_OK) goto report;
    }

    /* 4. Method stream + GP entry + GPPut + doorbell. */
    {
        volatile NvU32 *pb = (volatile NvU32 *)pBufCpu;
        volatile NvU32 *gp = (volatile NvU32 *)(pBufCpu + ECLIPSE_CHAN_GPFIFO_OFF);
        volatile Nvc46fControl *pUserd = (volatile Nvc46fControl *)pUserdCpu;
        NvU64 semHostVA = g_grChanCache.bufGpuVA + ECLIPSE_LAUNCH_HOST_SEM_OFF;
        NvU64 semEngVA  = g_grChanCache.bufGpuVA + ECLIPSE_LAUNCH_ENG_SEM_OFF;
        NvU32 n = 0;
        NvU32 gpEntry0, gpEntry1;

        /* Clear the semaphore landing zones first. */
        pb[ECLIPSE_LAUNCH_HOST_SEM_OFF / 4] = 0;
        pb[ECLIPSE_LAUNCH_ENG_SEM_OFF / 4]  = 0;

        /* Host semaphore RELEASE: SEM_ADDR_LO..SEM_EXECUTE (0x5c..0x6c). */
        pb[n++] = ECLIPSE_PUSH_HDR(0, NVC46F_SEM_ADDR_LO, 5);
        pb[n++] = NvU64_LO32(semHostVA);
        pb[n++] = DRF_NUM(C46F, _SEM_ADDR_HI, _OFFSET, NvU64_HI32(semHostVA));
        pb[n++] = ECLIPSE_LAUNCH_HOST_PAYLOAD;
        pb[n++] = 0; /* PAYLOAD_HI */
        pb[n++] = DRF_DEF(C46F, _SEM_EXECUTE, _OPERATION, _RELEASE) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _RELEASE_WFI, _DIS) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _PAYLOAD_SIZE, _32BIT) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _RELEASE_TIMESTAMP, _DIS);

        /* Bind TURING_COMPUTE_A on subchannel 1, then engine report
         * semaphore RELEASE through the compute FE. */
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC46F_SET_OBJECT, 1);
        pb[n++] = TURING_COMPUTE_A;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_REPORT_SEMAPHORE_A, 4);
        pb[n++] = DRF_NUM(C5C0, _SET_REPORT_SEMAPHORE_A, _OFFSET_UPPER,
                          NvU64_HI32(semEngVA));
        pb[n++] = NvU64_LO32(semEngVA);
        pb[n++] = ECLIPSE_LAUNCH_ENG_PAYLOAD;
        pb[n++] = DRF_DEF(C5C0, _SET_REPORT_SEMAPHORE_D, _OPERATION, _RELEASE) |
                  DRF_DEF(C5C0, _SET_REPORT_SEMAPHORE_D, _STRUCTURE_SIZE, _ONE_WORD);
        pOut->pushDwords = n;

        /* GP entry 0 (NV906F format, channelFillGpFifo verbatim). */
        gpEntry0 = DRF_DEF(906F, _GP_ENTRY0, _NO_CONTEXT_SWITCH, _FALSE) |
                   DRF_NUM(906F, _GP_ENTRY0, _GET, NvU64_LO32(g_grChanCache.bufGpuVA) >> 2);
        gpEntry1 = DRF_NUM(906F, _GP_ENTRY1, _GET_HI, NvU64_HI32(g_grChanCache.bufGpuVA)) |
                   DRF_NUM(906F, _GP_ENTRY1, _LENGTH, n) |
                   DRF_DEF(906F, _GP_ENTRY1, _LEVEL, _MAIN);
        gp[0] = gpEntry0;
        gp[1] = gpEntry1;
        osFlushCpuWriteCombineBuffer();

        pUserd->GPPut = 1;
        osFlushCpuWriteCombineBuffer();

        status = kbusFlushPcieForBar0Doorbell_HAL(pGpu, GPU_GET_KERNEL_BUS(pGpu));
        if (status == NV_OK)
        {
            status = kfifoUpdateUsermodeDoorbell_HAL(pGpu, pKernelFifo,
                                                     pOut->workToken,
                                                     pOut->runlistId);
        }
        pOut->submitStatus = status;
        nv_printf(0, "[eclipse-rm-trace] step18: submit (%u dwords, GPPut=1, doorbell) -> 0x%x\n",
                  n, pOut->submitStatus);
        if (status != NV_OK) goto report;
    }

    /* 5+6. CPU-poll both semaphores (1 ms ticks). */
    {
        volatile NvU32 *pHostSem = (volatile NvU32 *)(pBufCpu + ECLIPSE_LAUNCH_HOST_SEM_OFF);
        volatile NvU32 *pEngSem  = (volatile NvU32 *)(pBufCpu + ECLIPSE_LAUNCH_ENG_SEM_OFF);
        NvU32 i;
        for (i = 0; i < ECLIPSE_LAUNCH_POLL_MS; i++)
        {
            if (pOut->hostSemStatus != NV_OK && *pHostSem == ECLIPSE_LAUNCH_HOST_PAYLOAD)
            {
                pOut->hostSemStatus = NV_OK;
                pOut->hostPollIters = i;
            }
            if (pOut->engSemStatus != NV_OK && *pEngSem == ECLIPSE_LAUNCH_ENG_PAYLOAD)
            {
                pOut->engSemStatus = NV_OK;
                pOut->engPollIters = i;
            }
            if (pOut->hostSemStatus == NV_OK && pOut->engSemStatus == NV_OK)
                break;
            os_delay_us(1000);
        }
        pOut->hostSemValue = *pHostSem;
        pOut->engSemValue  = *pEngSem;
        if (pOut->hostSemStatus != NV_OK)
        {
            pOut->hostSemStatus = NV_ERR_TIMEOUT;
            pOut->hostPollIters = i;
        }
        if (pOut->engSemStatus != NV_OK)
        {
            pOut->engSemStatus = NV_ERR_TIMEOUT;
            pOut->engPollIters = i;
        }
        nv_printf(0, "[eclipse-rm-trace] step18: host sem 0x%x (val=0x%x @%u ms) eng sem 0x%x (val=0x%x @%u ms)\n",
                  pOut->hostSemStatus, pOut->hostSemValue, pOut->hostPollIters,
                  pOut->engSemStatus, pOut->engSemValue, pOut->engPollIters);
    }

report:
    if (pBufCpu != NULL)
        memmgrMemDescEndTransfer(pMemoryManager, pBufMemDesc, TRANSFER_FLAGS_NONE);
    if (pUserdCpu != NULL)
        memmgrMemDescEndTransfer(pMemoryManager, pUserdMemDesc, userdFlags);

    /* Cache only a fully-successful launch so a failed attempt can be
     * retried on the next read without a reboot. */
    if (pOut->hostSemStatus == NV_OK && pOut->engSemStatus == NV_OK)
    {
        portMemCopy(&g_grLaunchCache, sizeof(g_grLaunchCache), pOut, sizeof(*pOut));
        g_grLaunchDone = NV_TRUE;
    }

    rmGpuLocksRelease(GPUS_LOCK_FLAGS_NONE, NULL);
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return NV_OK; /* per-stage statuses carry any failure */
}

/*
 * Step-19: the first real compute LAUNCH -- a minimal SASS kernel executing
 * on the TU106 SMs, driven by a QMD we build and submit through the live
 * step-17/18 channel. Everything before ran host/CE methods; this makes the
 * Streaming Multiprocessors fetch and run instructions we authored.
 *
 * QMD layout: Turing (TURING_COMPUTE_A / C5C0) consumes the VOLTA QMD
 * V02_02 (clc3c0 QMDV02_02), NOT the clc5c0 V02_03 header. Verified against
 * mesa's NAK compiler, which runs on real Turing: nak_fill_qmd dispatches
 * `cls_compute >= VOLTA_COMPUTE_A && < AMPERE_COMPUTE_A => Qmd2_2`
 * (clc3c0 QMDV02_02). The field bit positions below are from NVIDIA's
 * open-gpu-doc clc3c0qmd.h; the field VALUES and which-fields-to-set follow
 * NAK's Qmd2_2::new()/fill_qmd (version=(2,2), API_VISIBLE_CALL_LIMIT=
 * NO_CHECK, SAMPLER_INDEX=INDEPENDENTLY, SM_GLOBAL_CACHING_ENABLE=1, plus
 * dims/prog-addr/register-count/smem-config). The launch method sequence
 * (shared/local memory windows, SKED-cache invalidate, SEND_PCAS_A +
 * SEND_SIGNALING_PCAS_B) follows nvk's nvk_push_dispatch_state_init /
 * nvk_cmd_dispatch. QMD is 64 dwords (256 B), 256-B aligned for SEND_PCAS_A
 * (qmd>>8).
 *
 * Kernel: the minimal Turing (SM75) program -- a single EXIT (operandless,
 * invariant encoding) followed by NOP padding to a 128-B fetch block. It
 * uses no registers, shared, local, or barriers; it launches, every lane
 * hits EXIT, the CTA and 1x1x1 grid complete, and the QMD's RELEASE0
 * semaphore is written to sysmem where we CPU-poll it. That write is proof
 * the SMs ran OUR program. A store-from-kernel + params is step20.
 *
 * Diagnostics separate "QMD rejected by SKED" (submit path) from "grid
 * launched but SM never released" (kernel/exec), so a single boot pinpoints
 * whether anything is still off and, if so, whether it's the QMD or the
 * SASS. Idempotent once the RELEASE semaphore lands.
 */

/* Volta V02_02 QMD field positions (hi,lo), from open-gpu-doc clc3c0qmd.h. */
#define QMDF_SM_GLOBAL_CACHING_ENABLE   134,134
#define QMDF_SEMAPHORE_RELEASE_ENABLE0  138,138
#define QMDF_INVAL_TEX_HEADER_CACHE     186,186
#define QMDF_INVAL_TEX_SAMPLER_CACHE    187,187
#define QMDF_INVAL_TEX_DATA_CACHE       188,188
#define QMDF_INVAL_SHADER_DATA_CACHE    189,189
#define QMDF_INVAL_INSTRUCTION_CACHE    190,190
#define QMDF_INVAL_SHADER_CONST_CACHE   191,191
#define QMDF_RELEASE_MEMBAR_TYPE        366,366
#define QMDF_CWD_MEMBAR_TYPE            369,368
#define QMDF_API_VISIBLE_CALL_LIMIT     378,378
#define QMDF_SAMPLER_INDEX              382,382
#define QMDF_CTA_RASTER_WIDTH           415,384
#define QMDF_CTA_RASTER_HEIGHT          431,416
#define QMDF_CTA_RASTER_DEPTH           463,448
#define QMDF_MIN_SM_CONFIG_SHMEM        568,562
#define QMDF_MAX_SM_CONFIG_SHMEM        575,569
#define QMDF_QMD_VERSION                579,576
#define QMDF_QMD_MAJOR_VERSION          583,580
#define QMDF_CTA_THREAD_DIMENSION0      607,592
#define QMDF_CTA_THREAD_DIMENSION1      623,608
#define QMDF_CTA_THREAD_DIMENSION2      639,624
#define QMDF_REGISTER_COUNT_V           656,648
#define QMDF_TARGET_SM_CONFIG_SHMEM     663,657
#define QMDF_RELEASE0_ADDRESS_LOWER     767,736
#define QMDF_RELEASE0_ADDRESS_UPPER     775,768
#define QMDF_RELEASE0_STRUCTURE_SIZE    799,799
#define QMDF_RELEASE0_PAYLOAD           831,800
#define QMDF_PROGRAM_ADDRESS_LOWER      1567,1536
#define QMDF_PROGRAM_ADDRESS_UPPER      1584,1568

/* Set bits [hi:lo] of the packed QMD to val (handles any range/crossing). */
static void eclipse_qmd_set_field(NvU32 *qmd, NvU32 hi, NvU32 lo, NvU32 val)
{
    NvU32 b;
    for (b = lo; b <= hi; b++)
    {
        NvU32 bit = (val >> (b - lo)) & 1u;
        qmd[b >> 5] = (qmd[b >> 5] & ~(1u << (b & 31))) | (bit << (b & 31));
    }
}
#define QMD_SET(q, field, val) eclipse_qmd_set_field((q), field, (val))

/* Minimal Turing SM75 kernel: EXIT + NOP padding (128 B, one fetch block).
 * Encodings are verbatim sm_75 cuobjdump/nvdisasm pairs (low qword, then
 * high qword), dword order little-endian:
 *   EXIT ;  0x000000000000794d / 0x000fea0003800000
 *   NOP ;   0x0000000000007918 / 0x000fc00000000000
 * First hardware run confirmed the harness end-to-end and timed out only
 * on grid release -- the EXIT then lacked the 0x03800000 word (control-
 * flow instructions carry it; a malformed EXIT = illegal instruction =
 * trapped warp = grid never releases). */
static const NvU32 g_sm75ExitKernel[32] = {
    0x0000794d, 0x00000000, 0x03800000, 0x000fea00, /* EXIT */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* NOP  */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* NOP  */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* NOP  */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* NOP  */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* NOP  */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* NOP  */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* NOP  */
};

#define ECLIPSE_LAUNCH_KERNEL_OFF  0x1000   /* SASS program (>=0x80 aligned) */
#define ECLIPSE_LAUNCH_QMD_OFF     0x2000   /* QMD (256 B aligned for >>8)   */
#define ECLIPSE_LAUNCH_QMD_SEM_OFF 0x8080   /* QMD RELEASE0 landing zone     */
#define ECLIPSE_LAUNCH_QMD_PAYLOAD 0x5A55C0DE
#define ECLIPSE_LAUNCH_FENCE_OFF   0x80C0   /* post-PCAS host-fence zone     */
#define ECLIPSE_LAUNCH_FENCE_PAYLOAD 0xFE7C4ED0
#define ECLIPSE_LAUNCH_REG_COUNT   16       /* >= kernel usage (0); granular */

typedef struct EclipseGrCompute
{
    NvU32 lookupStatus;
    NvU32 mapStatus;
    NvU32 tokenStatus;
    NvU32 submitStatus;
    NvU32 fenceStatus;    /* NV_OK = host fence AFTER SEND_PCAS landed:
                           * proves the PBDMA consumed the compute stream */
    NvU32 semStatus;      /* NV_OK = QMD RELEASE0 landed, else timeout */
    NvU32 workToken;
    NvU32 runlistId;
    NvU32 fenceValue;
    NvU32 fenceIters;
    NvU32 semValue;       /* CPU readback (expect ECLIPSE_LAUNCH_QMD_PAYLOAD) */
    NvU32 pollIters;      /* 1 ms units until seen */
    NvU32 pushDwords;
    NvU32 reservedPad;    /* keep the NvU64s 8-byte aligned on both sides */
    NvU64 kernelVA;
    NvU64 qmdVA;
} EclipseGrCompute;

static EclipseGrCompute g_grComputeCache;
static NvBool g_grComputeDone = NV_FALSE;

#define ECLIPSE_LAUNCH_POLL_MS2 3000

NV_STATUS eclipse_rm_step19(NvU32 gpuInstance, EclipseGrCompute *pOut)
{
    OBJGPU *pGpu;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;
    RsClient *pRsClient = NULL;
    KernelChannel *pKernelChannel = NULL;
    KernelFifo *pKernelFifo;
    MemoryManager *pMemoryManager;
    Memory *pBufMemory = NULL;
    MEMORY_DESCRIPTOR *pBufMemDesc = NULL;
    MEMORY_DESCRIPTOR *pUserdMemDesc = NULL;
    NvU8 *pBufCpu = NULL;
    NvU8 *pUserdCpu = NULL;
    NvU32 userdFlags = TRANSFER_FLAGS_USE_BAR1 |
                       TRANSFER_FLAGS_SHADOW_ALLOC |
                       TRANSFER_FLAGS_SHADOW_INIT_MEM;

    if (pOut == NULL)
    {
        return NV_ERR_INVALID_ARGUMENT;
    }
    if (g_grComputeDone)
    {
        portMemCopy(pOut, sizeof(*pOut), &g_grComputeCache, sizeof(g_grComputeCache));
        return NV_OK;
    }
    portMemSet(pOut, 0, sizeof(*pOut));
    pOut->lookupStatus = 0xFFFFFFFF;
    pOut->mapStatus    = 0xFFFFFFFF;
    pOut->tokenStatus  = 0xFFFFFFFF;
    pOut->submitStatus = 0xFFFFFFFF;
    pOut->fenceStatus  = 0xFFFFFFFF;
    pOut->semStatus    = 0xFFFFFFFF;

    if (!g_grChanDone)
    {
        return NV_ERR_INVALID_STATE; /* run step17 first */
    }

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);
    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }
    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    /* GPU lock: same as step18 -- we CPU-map the vidmem USERD via the FB
     * aperture, which asserts rmDeviceGpuLockIsOwner. */
    status = rmGpuLocksAcquire(GPUS_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        rmapiLockRelease();
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pKernelFifo = GPU_GET_KERNEL_FIFO(pGpu);
    pMemoryManager = GPU_GET_MEMORY_MANAGER(pGpu);

    /* 1. Locate the live channel + buffer + USERD (step-17 handles). */
    {
        NvU32 subdevInst;
        status = serverGetClientUnderLock(&g_resServ, g_grAllocCache.hClient, &pRsClient);
        if (status == NV_OK)
            status = CliGetKernelChannel(pRsClient, g_grChanCache.hChannel, &pKernelChannel);
        if (status == NV_OK)
            status = memGetByHandle(pRsClient, g_grChanCache.hPhysBuf, &pBufMemory);
        if (status == NV_OK)
        {
            pBufMemDesc = pBufMemory->pMemDesc;
            subdevInst = gpumgrGetSubDeviceInstanceFromGpu(pGpu);
            pUserdMemDesc = pKernelChannel->pUserdSubDeviceMemDesc[subdevInst];
            if (pBufMemDesc == NULL || pUserdMemDesc == NULL)
                status = NV_ERR_INVALID_STATE;
        }
        pOut->lookupStatus = status;
        nv_printf(0, "[eclipse-rm-trace] step19: lookup -> 0x%x\n", pOut->lookupStatus);
        if (status != NV_OK) goto report;
    }

    /* 2. CPU-map channel buffer (sysmem) + USERD (BAR1). */
    {
        pBufCpu = memmgrMemDescBeginTransfer(pMemoryManager, pBufMemDesc, TRANSFER_FLAGS_NONE);
        pUserdCpu = memmgrMemDescBeginTransfer(pMemoryManager, pUserdMemDesc, userdFlags);
        pOut->mapStatus = (pBufCpu != NULL && pUserdCpu != NULL) ? NV_OK : NV_ERR_GENERIC;
        nv_printf(0, "[eclipse-rm-trace] step19: CPU map buf=%s userd=%s -> 0x%x\n",
                  pBufCpu ? "ok" : "NULL", pUserdCpu ? "ok" : "NULL", pOut->mapStatus);
        if (pOut->mapStatus != NV_OK) goto report;
    }

    pOut->kernelVA = g_grChanCache.bufGpuVA + ECLIPSE_LAUNCH_KERNEL_OFF;
    pOut->qmdVA    = g_grChanCache.bufGpuVA + ECLIPSE_LAUNCH_QMD_OFF;

    /* 3. Write the SASS kernel and build the QMD in the channel buffer. */
    {
        NvU32 *qmd = (NvU32 *)(pBufCpu + ECLIPSE_LAUNCH_QMD_OFF);
        NvU64 semVA = g_grChanCache.bufGpuVA + ECLIPSE_LAUNCH_QMD_SEM_OFF;
        portMemCopy(pBufCpu + ECLIPSE_LAUNCH_KERNEL_OFF, sizeof(g_sm75ExitKernel),
                    g_sm75ExitKernel, sizeof(g_sm75ExitKernel));
        portMemSet(qmd, 0, 256);
        /* Clear the RELEASE and post-PCAS fence landing zones. */
        *(volatile NvU32 *)(pBufCpu + ECLIPSE_LAUNCH_QMD_SEM_OFF) = 0;
        *(volatile NvU32 *)(pBufCpu + ECLIPSE_LAUNCH_FENCE_OFF) = 0;

        /* Constant init (NAK Qmd2_2::new). */
        QMD_SET(qmd, QMDF_QMD_MAJOR_VERSION, 2);
        QMD_SET(qmd, QMDF_QMD_VERSION, 2);
        QMD_SET(qmd, QMDF_API_VISIBLE_CALL_LIMIT, 1);   /* NO_CHECK */
        QMD_SET(qmd, QMDF_SAMPLER_INDEX, 0);            /* INDEPENDENTLY */
        QMD_SET(qmd, QMDF_SM_GLOBAL_CACHING_ENABLE, 1);
        /* Grid 1x1x1, block 32x1x1 (one warp). */
        QMD_SET(qmd, QMDF_CTA_RASTER_WIDTH, 1);
        QMD_SET(qmd, QMDF_CTA_RASTER_HEIGHT, 1);
        QMD_SET(qmd, QMDF_CTA_RASTER_DEPTH, 1);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION0, 32);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION1, 1);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION2, 1);
        /* Program address (raw VA, no shift). */
        QMD_SET(qmd, QMDF_PROGRAM_ADDRESS_LOWER, (NvU32)(pOut->kernelVA & 0xFFFFFFFFu));
        QMD_SET(qmd, QMDF_PROGRAM_ADDRESS_UPPER, (NvU32)(pOut->kernelVA >> 32));
        QMD_SET(qmd, QMDF_REGISTER_COUNT_V, ECLIPSE_LAUNCH_REG_COUNT);
        /* Shared-mem config for TU106 (carveouts {32,64} KiB, 0 requested):
         * min=target=32KiB=hw9, max=64KiB=hw17 (NAK gv100_smem_size_to_hw). */
        QMD_SET(qmd, QMDF_MIN_SM_CONFIG_SHMEM, 9);
        QMD_SET(qmd, QMDF_TARGET_SM_CONFIG_SHMEM, 9);
        QMD_SET(qmd, QMDF_MAX_SM_CONFIG_SHMEM, 17);
        QMD_SET(qmd, QMDF_CWD_MEMBAR_TYPE, 1);          /* L1_SYSMEMBAR */
        QMD_SET(qmd, QMDF_RELEASE_MEMBAR_TYPE, 1);      /* FE_SYSMEMBAR */
        /* Completion semaphore: plain 1-word payload write on grid done. */
        QMD_SET(qmd, QMDF_SEMAPHORE_RELEASE_ENABLE0, 1);
        QMD_SET(qmd, QMDF_RELEASE0_ADDRESS_LOWER, (NvU32)(semVA & 0xFFFFFFFFu));
        QMD_SET(qmd, QMDF_RELEASE0_ADDRESS_UPPER, (NvU32)(semVA >> 32));
        QMD_SET(qmd, QMDF_RELEASE0_STRUCTURE_SIZE, 1);  /* ONE_WORD */
        QMD_SET(qmd, QMDF_RELEASE0_PAYLOAD, ECLIPSE_LAUNCH_QMD_PAYLOAD);
        osFlushCpuWriteCombineBuffer();
    }

    /* 4. Work-submit token (same channel as step18). */
    {
        pOut->tokenStatus = kfifoGenerateWorkSubmitToken(pGpu, pKernelFifo,
                                                         pKernelChannel,
                                                         &pOut->workToken, NV_TRUE);
        pOut->runlistId = kchannelGetRunlistId(pKernelChannel);
        nv_printf(0, "[eclipse-rm-trace] step19: token -> 0x%x token=0x%x runlist=%u\n",
                  pOut->tokenStatus, pOut->workToken, pOut->runlistId);
        if (pOut->tokenStatus != NV_OK) goto report;
    }

    /* 5. Compute launch method stream + GP entry + GPPut + doorbell. */
    {
        volatile NvU32 *pb = (volatile NvU32 *)pBufCpu; /* pushbuffer @ +0 */
        volatile NvU32 *gp = (volatile NvU32 *)(pBufCpu + ECLIPSE_CHAN_GPFIFO_OFF);
        volatile Nvc46fControl *pUserd = (volatile Nvc46fControl *)pUserdCpu;
        NvU32 n = 0, put, gpEntry0, gpEntry1;
        NvU64 pbVA = g_grChanCache.bufGpuVA;

        /* Bind compute on subch 1, set shader windows, invalidate SKED,
         * then SEND_PCAS + SEND_SIGNALING_PCAS (nvk dispatch sequence). */
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_OBJECT, 1);
        pb[n++] = TURING_COMPUTE_A;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_SHADER_SHARED_MEMORY_WINDOW_A, 2);
        pb[n++] = 0;                 /* WINDOW_A: base>>32 = 0 */
        pb[n++] = 0xFE000000;        /* WINDOW_B: 0xfe << 24   */
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_SHADER_LOCAL_MEMORY_WINDOW_A, 2);
        pb[n++] = 0;
        pb[n++] = 0xFF000000;        /* 0xff << 24 */
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_INVALIDATE_SKED_CACHES, 1);
        pb[n++] = 0;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SEND_PCAS_A, 1);
        pb[n++] = (NvU32)(pOut->qmdVA >> 8);
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SEND_SIGNALING_PCAS_B, 1);
        pb[n++] = DRF_DEF(C5C0, _SEND_SIGNALING_PCAS_B, _INVALIDATE, _TRUE) |
                  DRF_DEF(C5C0, _SEND_SIGNALING_PCAS_B, _SCHEDULE, _TRUE);
        /* Host fence AFTER the PCAS methods: when this lands, the PBDMA
         * definitively consumed the whole compute stream (removes the
         * "did the methods even run?" ambiguity from grid-only timeouts). */
        {
            NvU64 fenceVA = g_grChanCache.bufGpuVA + ECLIPSE_LAUNCH_FENCE_OFF;
            pb[n++] = ECLIPSE_PUSH_HDR(0, NVC46F_SEM_ADDR_LO, 5);
            pb[n++] = NvU64_LO32(fenceVA);
            pb[n++] = DRF_NUM(C46F, _SEM_ADDR_HI, _OFFSET, NvU64_HI32(fenceVA));
            pb[n++] = ECLIPSE_LAUNCH_FENCE_PAYLOAD;
            pb[n++] = 0;
            pb[n++] = DRF_DEF(C46F, _SEM_EXECUTE, _OPERATION, _RELEASE) |
                      DRF_DEF(C46F, _SEM_EXECUTE, _RELEASE_WFI, _DIS) |
                      DRF_DEF(C46F, _SEM_EXECUTE, _PAYLOAD_SIZE, _32BIT);
        }
        pOut->pushDwords = n;

        put = pUserd->GPPut;         /* continue the ring after step18 */
        gpEntry0 = DRF_DEF(906F, _GP_ENTRY0, _NO_CONTEXT_SWITCH, _FALSE) |
                   DRF_NUM(906F, _GP_ENTRY0, _GET, NvU64_LO32(pbVA) >> 2);
        gpEntry1 = DRF_NUM(906F, _GP_ENTRY1, _GET_HI, NvU64_HI32(pbVA)) |
                   DRF_NUM(906F, _GP_ENTRY1, _LENGTH, n) |
                   DRF_DEF(906F, _GP_ENTRY1, _LEVEL, _MAIN);
        gp[(put % ECLIPSE_CHAN_GPFIFO_ENTRIES) * 2 + 0] = gpEntry0;
        gp[(put % ECLIPSE_CHAN_GPFIFO_ENTRIES) * 2 + 1] = gpEntry1;
        osFlushCpuWriteCombineBuffer();

        pUserd->GPPut = put + 1;
        osFlushCpuWriteCombineBuffer();

        status = kbusFlushPcieForBar0Doorbell_HAL(pGpu, GPU_GET_KERNEL_BUS(pGpu));
        if (status == NV_OK)
            status = kfifoUpdateUsermodeDoorbell_HAL(pGpu, pKernelFifo,
                                                     pOut->workToken, pOut->runlistId);
        pOut->submitStatus = status;
        nv_printf(0, "[eclipse-rm-trace] step19: launch (%u dw, GPPut=%u, doorbell) -> 0x%x qmd=0x%llx prog=0x%llx\n",
                  n, put + 1, pOut->submitStatus, pOut->qmdVA, pOut->kernelVA);
        if (status != NV_OK) goto report;
    }

    /* 6. CPU-poll the post-PCAS fence AND the QMD RELEASE0 semaphore. */
    {
        volatile NvU32 *pSem = (volatile NvU32 *)(pBufCpu + ECLIPSE_LAUNCH_QMD_SEM_OFF);
        volatile NvU32 *pFence = (volatile NvU32 *)(pBufCpu + ECLIPSE_LAUNCH_FENCE_OFF);
        NvU32 i;
        for (i = 0; i < ECLIPSE_LAUNCH_POLL_MS2; i++)
        {
            if (pOut->fenceStatus != NV_OK && *pFence == ECLIPSE_LAUNCH_FENCE_PAYLOAD)
            {
                pOut->fenceStatus = NV_OK;
                pOut->fenceIters = i;
            }
            if (pOut->semStatus != NV_OK && *pSem == ECLIPSE_LAUNCH_QMD_PAYLOAD)
            {
                pOut->semStatus = NV_OK;
                pOut->pollIters = i;
            }
            if (pOut->fenceStatus == NV_OK && pOut->semStatus == NV_OK)
                break;
            os_delay_us(1000);
        }
        pOut->semValue = *pSem;
        pOut->fenceValue = *pFence;
        if (pOut->fenceStatus != NV_OK)
        {
            pOut->fenceStatus = NV_ERR_TIMEOUT;
            pOut->fenceIters = i;
        }
        if (pOut->semStatus != NV_OK)
        {
            pOut->semStatus = NV_ERR_TIMEOUT;
            pOut->pollIters = i;
        }
        nv_printf(0, "[eclipse-rm-trace] step19: pcas fence 0x%x (val=0x%x @%u ms) QMD release sem 0x%x (val=0x%x @%u ms)\n",
                  pOut->fenceStatus, pOut->fenceValue, pOut->fenceIters,
                  pOut->semStatus, pOut->semValue, pOut->pollIters);
    }

report:
    if (pBufCpu != NULL)
        memmgrMemDescEndTransfer(pMemoryManager, pBufMemDesc, TRANSFER_FLAGS_NONE);
    if (pUserdCpu != NULL)
        memmgrMemDescEndTransfer(pMemoryManager, pUserdMemDesc, userdFlags);

    if (pOut->semStatus == NV_OK)
    {
        portMemCopy(&g_grComputeCache, sizeof(g_grComputeCache), pOut, sizeof(*pOut));
        g_grComputeDone = NV_TRUE;
    }

    rmGpuLocksRelease(GPUS_LOCK_FLAGS_NONE, NULL);
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return NV_OK; /* per-stage statuses carry any failure */
}

/*
 * Step-20: the first kernel that COMPUTES for Eclipse -- stores a value we
 * chose to an address we chose, from the SM. Same QMD/SEND_PCAS harness as
 * the hardware-proven step19, new program:
 *
 *   MOV R2, dest_lo ; MOV R3, dest_hi ; MOV R5, value ;
 *   STG.E.SYS [R2:R3], R5 ; EXIT ; (NOP pad)
 *
 * The destination VA and value are PATCHED into the instruction immediates
 * at runtime (no constant-buffer plumbing needed for this rung). Encodings
 * come from CuAssembler's sm_75 instruction repository -- a table solved
 * from real nvdisasm output -- and the extraction convention was validated
 * by reproducing our two hardware-proven pairs (EXIT 0x03800000/0x794d and
 * NOP 0x7918) plus two solver instances each for MOV-imm and STG.E.SYS:
 *   MOV Rd, imm32     low64 = imm<<32 | Rd<<16 | 0x7802, code-high 0xf00
 *   STG.E.SYS [Ra],Rd low64 = Rd<<32 | Ra<<24 | 0x7386, code-high 0x0010e900
 * Control words: quiet-NOP control with the stall field maxed (0x000fcf)
 * on every ALU/store instruction, so no scoreboard/latency assumptions;
 * EXIT/NOP keep their proven controls verbatim.
 *
 * Verification adds a third check beyond step19's fence + RELEASE0: after
 * the grid releases, CPU-read the destination dword and compare. RELEASE0
 * landing but the store missing would isolate a memory-path bug; both
 * landing = the SM executed a program that computed an effect we observe.
 */
#define ECLIPSE_STORE_KERNEL_OFF   0x1200   /* keep step19's kernel intact */
#define ECLIPSE_STORE_QMD_OFF      0x2100
#define ECLIPSE_STORE_DEST_OFF     0x8100
#define ECLIPSE_STORE_VALUE        0xEC0DE520
#define ECLIPSE_STORE_SEM_OFF      0x8180
#define ECLIPSE_STORE_SEM_PAYLOAD  0x5A55C0D2
#define ECLIPSE_STORE_FENCE_OFF    0x81C0
#define ECLIPSE_STORE_FENCE_PAYLOAD 0xFE7C4ED2

/* SM75 store kernel template; immediates at dword indices 1 (dest lo),
 * 5 (dest hi), 9 (value) are patched before submission. */
static const NvU32 g_sm75StoreKernel[32] = {
    0x00027802, 0xDEAD0000, 0x00000f00, 0x000fcf00, /* MOV R2, dest_lo   */
    0x00037802, 0x00000000, 0x00000f00, 0x000fcf00, /* MOV R3, dest_hi   */
    0x00057802, 0xEC0DE520, 0x00000f00, 0x000fcf00, /* MOV R5, value     */
    0x02007386, 0x00000005, 0x0010e900, 0x000fcf00, /* STG.E.SYS [R2],R5 */
    0x0000794d, 0x00000000, 0x03800000, 0x000fea00, /* EXIT              */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* NOP               */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* NOP               */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* NOP               */
};

typedef struct EclipseGrStore
{
    NvU32 lookupStatus;
    NvU32 mapStatus;
    NvU32 tokenStatus;
    NvU32 submitStatus;
    NvU32 fenceStatus;
    NvU32 semStatus;
    NvU32 storeStatus;   /* NV_OK = dest dword == ECLIPSE_STORE_VALUE */
    NvU32 workToken;
    NvU32 runlistId;
    NvU32 fenceValue;
    NvU32 fenceIters;
    NvU32 semValue;
    NvU32 semIters;
    NvU32 storeValue;    /* CPU readback of the kernel's destination */
    NvU32 pushDwords;
    NvU32 reservedPad;
    NvU64 kernelVA;
    NvU64 qmdVA;
    NvU64 destVA;
} EclipseGrStore;

static EclipseGrStore g_grStoreCache;
static NvBool g_grStoreDone = NV_FALSE;

NV_STATUS eclipse_rm_step20(NvU32 gpuInstance, EclipseGrStore *pOut)
{
    OBJGPU *pGpu;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;
    RsClient *pRsClient = NULL;
    KernelChannel *pKernelChannel = NULL;
    KernelFifo *pKernelFifo;
    MemoryManager *pMemoryManager;
    Memory *pBufMemory = NULL;
    MEMORY_DESCRIPTOR *pBufMemDesc = NULL;
    MEMORY_DESCRIPTOR *pUserdMemDesc = NULL;
    NvU8 *pBufCpu = NULL;
    NvU8 *pUserdCpu = NULL;
    NvU32 userdFlags = TRANSFER_FLAGS_USE_BAR1 |
                       TRANSFER_FLAGS_SHADOW_ALLOC |
                       TRANSFER_FLAGS_SHADOW_INIT_MEM;

    if (pOut == NULL)
    {
        return NV_ERR_INVALID_ARGUMENT;
    }
    if (g_grStoreDone)
    {
        portMemCopy(pOut, sizeof(*pOut), &g_grStoreCache, sizeof(g_grStoreCache));
        return NV_OK;
    }
    portMemSet(pOut, 0, sizeof(*pOut));
    pOut->lookupStatus = 0xFFFFFFFF;
    pOut->mapStatus    = 0xFFFFFFFF;
    pOut->tokenStatus  = 0xFFFFFFFF;
    pOut->submitStatus = 0xFFFFFFFF;
    pOut->fenceStatus  = 0xFFFFFFFF;
    pOut->semStatus    = 0xFFFFFFFF;
    pOut->storeStatus  = 0xFFFFFFFF;

    if (!g_grChanDone)
    {
        return NV_ERR_INVALID_STATE; /* run step17 first */
    }

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);
    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }
    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    status = rmGpuLocksAcquire(GPUS_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        rmapiLockRelease();
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pKernelFifo = GPU_GET_KERNEL_FIFO(pGpu);
    pMemoryManager = GPU_GET_MEMORY_MANAGER(pGpu);

    /* 1. Locate channel/buffer/USERD. */
    {
        NvU32 subdevInst;
        status = serverGetClientUnderLock(&g_resServ, g_grAllocCache.hClient, &pRsClient);
        if (status == NV_OK)
            status = CliGetKernelChannel(pRsClient, g_grChanCache.hChannel, &pKernelChannel);
        if (status == NV_OK)
            status = memGetByHandle(pRsClient, g_grChanCache.hPhysBuf, &pBufMemory);
        if (status == NV_OK)
        {
            pBufMemDesc = pBufMemory->pMemDesc;
            subdevInst = gpumgrGetSubDeviceInstanceFromGpu(pGpu);
            pUserdMemDesc = pKernelChannel->pUserdSubDeviceMemDesc[subdevInst];
            if (pBufMemDesc == NULL || pUserdMemDesc == NULL)
                status = NV_ERR_INVALID_STATE;
        }
        pOut->lookupStatus = status;
        nv_printf(0, "[eclipse-rm-trace] step20: lookup -> 0x%x\n", pOut->lookupStatus);
        if (status != NV_OK) goto report;
    }

    /* 2. CPU maps. */
    {
        pBufCpu = memmgrMemDescBeginTransfer(pMemoryManager, pBufMemDesc, TRANSFER_FLAGS_NONE);
        pUserdCpu = memmgrMemDescBeginTransfer(pMemoryManager, pUserdMemDesc, userdFlags);
        pOut->mapStatus = (pBufCpu != NULL && pUserdCpu != NULL) ? NV_OK : NV_ERR_GENERIC;
        nv_printf(0, "[eclipse-rm-trace] step20: CPU map -> 0x%x\n", pOut->mapStatus);
        if (pOut->mapStatus != NV_OK) goto report;
    }

    pOut->kernelVA = g_grChanCache.bufGpuVA + ECLIPSE_STORE_KERNEL_OFF;
    pOut->qmdVA    = g_grChanCache.bufGpuVA + ECLIPSE_STORE_QMD_OFF;
    pOut->destVA   = g_grChanCache.bufGpuVA + ECLIPSE_STORE_DEST_OFF;

    /* 3. Patch + write the kernel, build the QMD. */
    {
        NvU32 kern[32];
        NvU32 *qmd = (NvU32 *)(pBufCpu + ECLIPSE_STORE_QMD_OFF);
        NvU64 semVA = g_grChanCache.bufGpuVA + ECLIPSE_STORE_SEM_OFF;
        portMemCopy(kern, sizeof(kern), g_sm75StoreKernel, sizeof(g_sm75StoreKernel));
        kern[1] = NvU64_LO32(pOut->destVA);   /* MOV R2, dest_lo */
        kern[5] = NvU64_HI32(pOut->destVA);   /* MOV R3, dest_hi */
        kern[9] = ECLIPSE_STORE_VALUE;        /* MOV R5, value   */
        portMemCopy(pBufCpu + ECLIPSE_STORE_KERNEL_OFF, sizeof(kern), kern, sizeof(kern));
        /* Clear landing zones: dest, sem, fence. */
        *(volatile NvU32 *)(pBufCpu + ECLIPSE_STORE_DEST_OFF) = 0;
        *(volatile NvU32 *)(pBufCpu + ECLIPSE_STORE_SEM_OFF) = 0;
        *(volatile NvU32 *)(pBufCpu + ECLIPSE_STORE_FENCE_OFF) = 0;

        portMemSet(qmd, 0, 256);
        QMD_SET(qmd, QMDF_QMD_MAJOR_VERSION, 2);
        QMD_SET(qmd, QMDF_QMD_VERSION, 2);
        QMD_SET(qmd, QMDF_API_VISIBLE_CALL_LIMIT, 1);
        QMD_SET(qmd, QMDF_SAMPLER_INDEX, 0);
        QMD_SET(qmd, QMDF_SM_GLOBAL_CACHING_ENABLE, 1);
        QMD_SET(qmd, QMDF_CTA_RASTER_WIDTH, 1);
        QMD_SET(qmd, QMDF_CTA_RASTER_HEIGHT, 1);
        QMD_SET(qmd, QMDF_CTA_RASTER_DEPTH, 1);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION0, 32);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION1, 1);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION2, 1);
        QMD_SET(qmd, QMDF_PROGRAM_ADDRESS_LOWER, (NvU32)(pOut->kernelVA & 0xFFFFFFFFu));
        QMD_SET(qmd, QMDF_PROGRAM_ADDRESS_UPPER, (NvU32)(pOut->kernelVA >> 32));
        QMD_SET(qmd, QMDF_REGISTER_COUNT_V, ECLIPSE_LAUNCH_REG_COUNT);
        QMD_SET(qmd, QMDF_MIN_SM_CONFIG_SHMEM, 9);
        QMD_SET(qmd, QMDF_TARGET_SM_CONFIG_SHMEM, 9);
        QMD_SET(qmd, QMDF_MAX_SM_CONFIG_SHMEM, 17);
        QMD_SET(qmd, QMDF_CWD_MEMBAR_TYPE, 1);
        QMD_SET(qmd, QMDF_RELEASE_MEMBAR_TYPE, 1);
        QMD_SET(qmd, QMDF_SEMAPHORE_RELEASE_ENABLE0, 1);
        QMD_SET(qmd, QMDF_RELEASE0_ADDRESS_LOWER, (NvU32)(semVA & 0xFFFFFFFFu));
        QMD_SET(qmd, QMDF_RELEASE0_ADDRESS_UPPER, (NvU32)(semVA >> 32));
        QMD_SET(qmd, QMDF_RELEASE0_STRUCTURE_SIZE, 1);
        QMD_SET(qmd, QMDF_RELEASE0_PAYLOAD, ECLIPSE_STORE_SEM_PAYLOAD);
        osFlushCpuWriteCombineBuffer();
    }

    /* 4. Token. */
    {
        pOut->tokenStatus = kfifoGenerateWorkSubmitToken(pGpu, pKernelFifo,
                                                         pKernelChannel,
                                                         &pOut->workToken, NV_TRUE);
        pOut->runlistId = kchannelGetRunlistId(pKernelChannel);
        if (pOut->tokenStatus != NV_OK) goto report;
    }

    /* 5. Launch stream (step19 shape, step20 QMD) + fence. */
    {
        volatile NvU32 *pb = (volatile NvU32 *)pBufCpu;
        volatile NvU32 *gp = (volatile NvU32 *)(pBufCpu + ECLIPSE_CHAN_GPFIFO_OFF);
        volatile Nvc46fControl *pUserd = (volatile Nvc46fControl *)pUserdCpu;
        NvU32 n = 0, put, gpEntry0, gpEntry1;
        NvU64 pbVA = g_grChanCache.bufGpuVA;
        NvU64 fenceVA = g_grChanCache.bufGpuVA + ECLIPSE_STORE_FENCE_OFF;

        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_OBJECT, 1);
        pb[n++] = TURING_COMPUTE_A;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_SHADER_SHARED_MEMORY_WINDOW_A, 2);
        pb[n++] = 0;
        pb[n++] = 0xFE000000;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_SHADER_LOCAL_MEMORY_WINDOW_A, 2);
        pb[n++] = 0;
        pb[n++] = 0xFF000000;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_INVALIDATE_SKED_CACHES, 1);
        pb[n++] = 0;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SEND_PCAS_A, 1);
        pb[n++] = (NvU32)(pOut->qmdVA >> 8);
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SEND_SIGNALING_PCAS_B, 1);
        pb[n++] = DRF_DEF(C5C0, _SEND_SIGNALING_PCAS_B, _INVALIDATE, _TRUE) |
                  DRF_DEF(C5C0, _SEND_SIGNALING_PCAS_B, _SCHEDULE, _TRUE);
        pb[n++] = ECLIPSE_PUSH_HDR(0, NVC46F_SEM_ADDR_LO, 5);
        pb[n++] = NvU64_LO32(fenceVA);
        pb[n++] = DRF_NUM(C46F, _SEM_ADDR_HI, _OFFSET, NvU64_HI32(fenceVA));
        pb[n++] = ECLIPSE_STORE_FENCE_PAYLOAD;
        pb[n++] = 0;
        pb[n++] = DRF_DEF(C46F, _SEM_EXECUTE, _OPERATION, _RELEASE) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _RELEASE_WFI, _DIS) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _PAYLOAD_SIZE, _32BIT);
        pOut->pushDwords = n;

        put = pUserd->GPPut;
        gpEntry0 = DRF_DEF(906F, _GP_ENTRY0, _NO_CONTEXT_SWITCH, _FALSE) |
                   DRF_NUM(906F, _GP_ENTRY0, _GET, NvU64_LO32(pbVA) >> 2);
        gpEntry1 = DRF_NUM(906F, _GP_ENTRY1, _GET_HI, NvU64_HI32(pbVA)) |
                   DRF_NUM(906F, _GP_ENTRY1, _LENGTH, n) |
                   DRF_DEF(906F, _GP_ENTRY1, _LEVEL, _MAIN);
        gp[(put % ECLIPSE_CHAN_GPFIFO_ENTRIES) * 2 + 0] = gpEntry0;
        gp[(put % ECLIPSE_CHAN_GPFIFO_ENTRIES) * 2 + 1] = gpEntry1;
        osFlushCpuWriteCombineBuffer();

        pUserd->GPPut = put + 1;
        osFlushCpuWriteCombineBuffer();

        status = kbusFlushPcieForBar0Doorbell_HAL(pGpu, GPU_GET_KERNEL_BUS(pGpu));
        if (status == NV_OK)
            status = kfifoUpdateUsermodeDoorbell_HAL(pGpu, pKernelFifo,
                                                     pOut->workToken, pOut->runlistId);
        pOut->submitStatus = status;
        nv_printf(0, "[eclipse-rm-trace] step20: launch (%u dw, GPPut=%u) -> 0x%x qmd=0x%llx prog=0x%llx dest=0x%llx\n",
                  n, put + 1, pOut->submitStatus, pOut->qmdVA, pOut->kernelVA, pOut->destVA);
        if (status != NV_OK) goto report;
    }

    /* 6. Poll fence + RELEASE0, then read back the kernel's store. */
    {
        volatile NvU32 *pSem = (volatile NvU32 *)(pBufCpu + ECLIPSE_STORE_SEM_OFF);
        volatile NvU32 *pFence = (volatile NvU32 *)(pBufCpu + ECLIPSE_STORE_FENCE_OFF);
        volatile NvU32 *pDest = (volatile NvU32 *)(pBufCpu + ECLIPSE_STORE_DEST_OFF);
        NvU32 i;
        for (i = 0; i < ECLIPSE_LAUNCH_POLL_MS2; i++)
        {
            if (pOut->fenceStatus != NV_OK && *pFence == ECLIPSE_STORE_FENCE_PAYLOAD)
            {
                pOut->fenceStatus = NV_OK;
                pOut->fenceIters = i;
            }
            if (pOut->semStatus != NV_OK && *pSem == ECLIPSE_STORE_SEM_PAYLOAD)
            {
                pOut->semStatus = NV_OK;
                pOut->semIters = i;
            }
            if (pOut->fenceStatus == NV_OK && pOut->semStatus == NV_OK)
                break;
            os_delay_us(1000);
        }
        pOut->semValue = *pSem;
        pOut->fenceValue = *pFence;
        pOut->storeValue = *pDest;
        if (pOut->fenceStatus != NV_OK) { pOut->fenceStatus = NV_ERR_TIMEOUT; pOut->fenceIters = i; }
        if (pOut->semStatus != NV_OK)   { pOut->semStatus = NV_ERR_TIMEOUT;   pOut->semIters = i; }
        pOut->storeStatus = (pOut->storeValue == ECLIPSE_STORE_VALUE)
                                ? NV_OK : NV_ERR_INVALID_DATA;
        nv_printf(0, "[eclipse-rm-trace] step20: fence 0x%x (@%u ms) sem 0x%x (@%u ms) store 0x%x (val=0x%x)\n",
                  pOut->fenceStatus, pOut->fenceIters, pOut->semStatus, pOut->semIters,
                  pOut->storeStatus, pOut->storeValue);
    }

report:
    if (pBufCpu != NULL)
        memmgrMemDescEndTransfer(pMemoryManager, pBufMemDesc, TRANSFER_FLAGS_NONE);
    if (pUserdCpu != NULL)
        memmgrMemDescEndTransfer(pMemoryManager, pUserdMemDesc, userdFlags);

    if (pOut->semStatus == NV_OK && pOut->storeStatus == NV_OK)
    {
        portMemCopy(&g_grStoreCache, sizeof(g_grStoreCache), pOut, sizeof(*pOut));
        g_grStoreDone = NV_TRUE;
    }

    rmGpuLocksRelease(GPUS_LOCK_FLAGS_NONE, NULL);
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return NV_OK; /* per-stage statuses carry any failure */
}

/*
 * Step-21: MULTI-THREAD computation -- 32 threads each compute their own
 * result and store it to their own slot: out[tid] = tid*3 + 7. This proves
 * per-thread execution (S2R thread-ID), integer math (IMAD), per-thread
 * address computation (IMAD.WIDE), per-thread stores, and -- critically --
 * real Volta+ scoreboarding: S2R is variable-latency, so it sets write-
 * barrier 0 and both consumers carry wait-mask bit 0 in their control
 * words. Control layout (delay[105:109), yield 109, wr_bar[110:113),
 * rd_bar[113:116), wt_mask[116:122)) is from mesa NAK's sm70_encode.rs and
 * was validated by exactly reproducing our two hardware-proven control
 * words (NOP delay=0/none, EXIT delay=5+yield).
 *
 * Instruction encodings are backed by CuAssembler's sm_75 corpus (3672
 * real nvdisasm records): S2R (SR_TID.X = idx 33) verbatim, IMAD form-4
 * (reg*imm+reg, signed bit 73) and IMAD.WIDE.U32 derived from corpus
 * instances and self-checked against them bit-exactly, STG.E.SYS already
 * proven on silicon in step20. No LDG on this rung: the corpus only
 * carries UR-addressed loads (useless for per-thread addresses), so loads
 * enter on the next rung via the corpus-verbatim ULDC+LDG[URn] pair.
 *
 * Verification: CPU zeroes out[32], the GPU fills it, the CPU checks all
 * 32 slots against 3*i+7 -- 32 distinct values only a correct per-thread
 * execution can produce. Reports match count + first mismatch.
 */
#define ECLIPSE_THREADS_KERNEL_OFF 0x1400
#define ECLIPSE_THREADS_QMD_OFF    0x2200
#define ECLIPSE_THREADS_OUT_OFF    0x8200   /* out[32] dwords = 128 B */
#define ECLIPSE_THREADS_SEM_OFF    0x8280
#define ECLIPSE_THREADS_SEM_PAYLOAD 0x5A55C0D3
#define ECLIPSE_THREADS_FENCE_OFF  0x82C0
#define ECLIPSE_THREADS_FENCE_PAYLOAD 0xFE7C4ED3

/* SM75, 12 instructions; patch dwords 5 (out_lo) and 9 (out_hi).
 *   S2R R0, SR_TID.X            (wr_bar 0)
 *   MOV R2, out_lo ; MOV R3, out_hi ; MOV R9, 7
 *   IMAD R8, R0, 3, R9          (wait b0)   R8 = tid*3 + 7
 *   IMAD.WIDE.U32 R2, R0, 4, R2 (wait b0)   R2:R3 = out + tid*4
 *   STG.E.SYS [R2], R8 ; EXIT ; NOP x4                          */
static const NvU32 g_sm75ThreadsKernel[48] = {
    0x00007919, 0x00000000, 0x00002100, 0x000e0200,
    0x00027802, 0xDEAD0000, 0x00000f00, 0x000fc400,
    0x00037802, 0x00000000, 0x00000f00, 0x000fc400,
    0x00097802, 0x00000007, 0x00000f00, 0x000fc400,
    0x00087824, 0x00000003, 0x078e0209, 0x001fca00,
    0x00027825, 0x00000004, 0x078e0002, 0x001fca00,
    0x02007386, 0x00000008, 0x0010e900, 0x000fc400,
    0x0000794d, 0x00000000, 0x03800000, 0x000fea00,
    0x00007918, 0x00000000, 0x00000000, 0x000fc000,
    0x00007918, 0x00000000, 0x00000000, 0x000fc000,
    0x00007918, 0x00000000, 0x00000000, 0x000fc000,
    0x00007918, 0x00000000, 0x00000000, 0x000fc000,
};

typedef struct EclipseGrThreads
{
    NvU32 lookupStatus;
    NvU32 mapStatus;
    NvU32 tokenStatus;
    NvU32 submitStatus;
    NvU32 fenceStatus;
    NvU32 semStatus;
    NvU32 verifyStatus;  /* NV_OK = all 32 out[i] == 3*i+7 */
    NvU32 workToken;
    NvU32 runlistId;
    NvU32 fenceIters;
    NvU32 semIters;
    NvU32 matchCount;    /* how many of the 32 slots verified */
    NvU32 firstBadIdx;   /* 0xFFFFFFFF = none */
    NvU32 firstBadVal;
    NvU32 pushDwords;
    NvU32 reservedPad;
    NvU64 kernelVA;
    NvU64 qmdVA;
    NvU64 outVA;
    /* MMU-fault telemetry (step23): filled from NV906F_CTRL_CMD_GET_MMU_
     * FAULT_INFO after a failed poll, so the fault address/type reach the
     * /proc formatter even when the nv_printf capture drops lines.
     * faultCtrlStatus = 0xFFFFFFFF means "not queried". */
    NvU32 faultCtrlStatus;
    NvU32 faultAddrHi;
    NvU32 faultAddrLo;
    NvU32 faultType;
} EclipseGrThreads;

static EclipseGrThreads g_grThreadsCache;
static NvBool g_grThreadsDone = NV_FALSE;

NV_STATUS eclipse_rm_step21(NvU32 gpuInstance, EclipseGrThreads *pOut)
{
    OBJGPU *pGpu;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;
    RsClient *pRsClient = NULL;
    KernelChannel *pKernelChannel = NULL;
    KernelFifo *pKernelFifo;
    MemoryManager *pMemoryManager;
    Memory *pBufMemory = NULL;
    MEMORY_DESCRIPTOR *pBufMemDesc = NULL;
    MEMORY_DESCRIPTOR *pUserdMemDesc = NULL;
    NvU8 *pBufCpu = NULL;
    NvU8 *pUserdCpu = NULL;
    NvU32 userdFlags = TRANSFER_FLAGS_USE_BAR1 |
                       TRANSFER_FLAGS_SHADOW_ALLOC |
                       TRANSFER_FLAGS_SHADOW_INIT_MEM;

    if (pOut == NULL)
    {
        return NV_ERR_INVALID_ARGUMENT;
    }
    if (g_grThreadsDone)
    {
        portMemCopy(pOut, sizeof(*pOut), &g_grThreadsCache, sizeof(g_grThreadsCache));
        return NV_OK;
    }
    portMemSet(pOut, 0, sizeof(*pOut));
    pOut->lookupStatus = 0xFFFFFFFF;
    pOut->mapStatus    = 0xFFFFFFFF;
    pOut->tokenStatus  = 0xFFFFFFFF;
    pOut->submitStatus = 0xFFFFFFFF;
    pOut->fenceStatus  = 0xFFFFFFFF;
    pOut->semStatus    = 0xFFFFFFFF;
    pOut->verifyStatus = 0xFFFFFFFF;
    pOut->firstBadIdx  = 0xFFFFFFFF;

    if (!g_grChanDone)
    {
        return NV_ERR_INVALID_STATE; /* run step17 first */
    }

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);
    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }
    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    status = rmGpuLocksAcquire(GPUS_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        rmapiLockRelease();
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pKernelFifo = GPU_GET_KERNEL_FIFO(pGpu);
    pMemoryManager = GPU_GET_MEMORY_MANAGER(pGpu);

    /* 1. Locate channel/buffer/USERD. */
    {
        NvU32 subdevInst;
        status = serverGetClientUnderLock(&g_resServ, g_grAllocCache.hClient, &pRsClient);
        if (status == NV_OK)
            status = CliGetKernelChannel(pRsClient, g_grChanCache.hChannel, &pKernelChannel);
        if (status == NV_OK)
            status = memGetByHandle(pRsClient, g_grChanCache.hPhysBuf, &pBufMemory);
        if (status == NV_OK)
        {
            pBufMemDesc = pBufMemory->pMemDesc;
            subdevInst = gpumgrGetSubDeviceInstanceFromGpu(pGpu);
            pUserdMemDesc = pKernelChannel->pUserdSubDeviceMemDesc[subdevInst];
            if (pBufMemDesc == NULL || pUserdMemDesc == NULL)
                status = NV_ERR_INVALID_STATE;
        }
        pOut->lookupStatus = status;
        if (status != NV_OK) goto report;
    }

    /* 2. CPU maps. */
    {
        pBufCpu = memmgrMemDescBeginTransfer(pMemoryManager, pBufMemDesc, TRANSFER_FLAGS_NONE);
        pUserdCpu = memmgrMemDescBeginTransfer(pMemoryManager, pUserdMemDesc, userdFlags);
        pOut->mapStatus = (pBufCpu != NULL && pUserdCpu != NULL) ? NV_OK : NV_ERR_GENERIC;
        if (pOut->mapStatus != NV_OK) goto report;
    }

    pOut->kernelVA = g_grChanCache.bufGpuVA + ECLIPSE_THREADS_KERNEL_OFF;
    pOut->qmdVA    = g_grChanCache.bufGpuVA + ECLIPSE_THREADS_QMD_OFF;
    pOut->outVA    = g_grChanCache.bufGpuVA + ECLIPSE_THREADS_OUT_OFF;

    /* 3. Patch + write the kernel; zero out[]; build the QMD. */
    {
        NvU32 kern[48];
        NvU32 *qmd = (NvU32 *)(pBufCpu + ECLIPSE_THREADS_QMD_OFF);
        NvU64 semVA = g_grChanCache.bufGpuVA + ECLIPSE_THREADS_SEM_OFF;
        NvU32 i;
        portMemCopy(kern, sizeof(kern), g_sm75ThreadsKernel, sizeof(g_sm75ThreadsKernel));
        kern[5] = NvU64_LO32(pOut->outVA);   /* MOV R2, out_lo */
        kern[9] = NvU64_HI32(pOut->outVA);   /* MOV R3, out_hi */
        portMemCopy(pBufCpu + ECLIPSE_THREADS_KERNEL_OFF, sizeof(kern), kern, sizeof(kern));
        for (i = 0; i < 32; i++)
            ((volatile NvU32 *)(pBufCpu + ECLIPSE_THREADS_OUT_OFF))[i] = 0;
        *(volatile NvU32 *)(pBufCpu + ECLIPSE_THREADS_SEM_OFF) = 0;
        *(volatile NvU32 *)(pBufCpu + ECLIPSE_THREADS_FENCE_OFF) = 0;

        portMemSet(qmd, 0, 256);
        QMD_SET(qmd, QMDF_QMD_MAJOR_VERSION, 2);
        QMD_SET(qmd, QMDF_QMD_VERSION, 2);
        QMD_SET(qmd, QMDF_API_VISIBLE_CALL_LIMIT, 1);
        QMD_SET(qmd, QMDF_SAMPLER_INDEX, 0);
        QMD_SET(qmd, QMDF_SM_GLOBAL_CACHING_ENABLE, 1);
        QMD_SET(qmd, QMDF_CTA_RASTER_WIDTH, 1);
        QMD_SET(qmd, QMDF_CTA_RASTER_HEIGHT, 1);
        QMD_SET(qmd, QMDF_CTA_RASTER_DEPTH, 1);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION0, 32);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION1, 1);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION2, 1);
        QMD_SET(qmd, QMDF_PROGRAM_ADDRESS_LOWER, (NvU32)(pOut->kernelVA & 0xFFFFFFFFu));
        QMD_SET(qmd, QMDF_PROGRAM_ADDRESS_UPPER, (NvU32)(pOut->kernelVA >> 32));
        QMD_SET(qmd, QMDF_REGISTER_COUNT_V, ECLIPSE_LAUNCH_REG_COUNT);
        QMD_SET(qmd, QMDF_MIN_SM_CONFIG_SHMEM, 9);
        QMD_SET(qmd, QMDF_TARGET_SM_CONFIG_SHMEM, 9);
        QMD_SET(qmd, QMDF_MAX_SM_CONFIG_SHMEM, 17);
        QMD_SET(qmd, QMDF_CWD_MEMBAR_TYPE, 1);
        QMD_SET(qmd, QMDF_RELEASE_MEMBAR_TYPE, 1);
        QMD_SET(qmd, QMDF_SEMAPHORE_RELEASE_ENABLE0, 1);
        QMD_SET(qmd, QMDF_RELEASE0_ADDRESS_LOWER, (NvU32)(semVA & 0xFFFFFFFFu));
        QMD_SET(qmd, QMDF_RELEASE0_ADDRESS_UPPER, (NvU32)(semVA >> 32));
        QMD_SET(qmd, QMDF_RELEASE0_STRUCTURE_SIZE, 1);
        QMD_SET(qmd, QMDF_RELEASE0_PAYLOAD, ECLIPSE_THREADS_SEM_PAYLOAD);
        osFlushCpuWriteCombineBuffer();
    }

    /* 4. Token. */
    {
        pOut->tokenStatus = kfifoGenerateWorkSubmitToken(pGpu, pKernelFifo,
                                                         pKernelChannel,
                                                         &pOut->workToken, NV_TRUE);
        pOut->runlistId = kchannelGetRunlistId(pKernelChannel);
        if (pOut->tokenStatus != NV_OK) goto report;
    }

    /* 5. Launch. */
    {
        volatile NvU32 *pb = (volatile NvU32 *)pBufCpu;
        volatile NvU32 *gp = (volatile NvU32 *)(pBufCpu + ECLIPSE_CHAN_GPFIFO_OFF);
        volatile Nvc46fControl *pUserd = (volatile Nvc46fControl *)pUserdCpu;
        NvU32 n = 0, put, gpEntry0, gpEntry1;
        NvU64 pbVA = g_grChanCache.bufGpuVA;
        NvU64 fenceVA = g_grChanCache.bufGpuVA + ECLIPSE_THREADS_FENCE_OFF;

        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_OBJECT, 1);
        pb[n++] = TURING_COMPUTE_A;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_SHADER_SHARED_MEMORY_WINDOW_A, 2);
        pb[n++] = 0;
        pb[n++] = 0xFE000000;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_SHADER_LOCAL_MEMORY_WINDOW_A, 2);
        pb[n++] = 0;
        pb[n++] = 0xFF000000;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_INVALIDATE_SKED_CACHES, 1);
        pb[n++] = 0;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SEND_PCAS_A, 1);
        pb[n++] = (NvU32)(pOut->qmdVA >> 8);
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SEND_SIGNALING_PCAS_B, 1);
        pb[n++] = DRF_DEF(C5C0, _SEND_SIGNALING_PCAS_B, _INVALIDATE, _TRUE) |
                  DRF_DEF(C5C0, _SEND_SIGNALING_PCAS_B, _SCHEDULE, _TRUE);
        pb[n++] = ECLIPSE_PUSH_HDR(0, NVC46F_SEM_ADDR_LO, 5);
        pb[n++] = NvU64_LO32(fenceVA);
        pb[n++] = DRF_NUM(C46F, _SEM_ADDR_HI, _OFFSET, NvU64_HI32(fenceVA));
        pb[n++] = ECLIPSE_THREADS_FENCE_PAYLOAD;
        pb[n++] = 0;
        pb[n++] = DRF_DEF(C46F, _SEM_EXECUTE, _OPERATION, _RELEASE) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _RELEASE_WFI, _DIS) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _PAYLOAD_SIZE, _32BIT);
        pOut->pushDwords = n;

        put = pUserd->GPPut;
        gpEntry0 = DRF_DEF(906F, _GP_ENTRY0, _NO_CONTEXT_SWITCH, _FALSE) |
                   DRF_NUM(906F, _GP_ENTRY0, _GET, NvU64_LO32(pbVA) >> 2);
        gpEntry1 = DRF_NUM(906F, _GP_ENTRY1, _GET_HI, NvU64_HI32(pbVA)) |
                   DRF_NUM(906F, _GP_ENTRY1, _LENGTH, n) |
                   DRF_DEF(906F, _GP_ENTRY1, _LEVEL, _MAIN);
        gp[(put % ECLIPSE_CHAN_GPFIFO_ENTRIES) * 2 + 0] = gpEntry0;
        gp[(put % ECLIPSE_CHAN_GPFIFO_ENTRIES) * 2 + 1] = gpEntry1;
        osFlushCpuWriteCombineBuffer();

        pUserd->GPPut = put + 1;
        osFlushCpuWriteCombineBuffer();

        status = kbusFlushPcieForBar0Doorbell_HAL(pGpu, GPU_GET_KERNEL_BUS(pGpu));
        if (status == NV_OK)
            status = kfifoUpdateUsermodeDoorbell_HAL(pGpu, pKernelFifo,
                                                     pOut->workToken, pOut->runlistId);
        pOut->submitStatus = status;
        nv_printf(0, "[eclipse-rm-trace] step21: launch (%u dw, GPPut=%u) -> 0x%x qmd=0x%llx prog=0x%llx out=0x%llx\n",
                  n, put + 1, pOut->submitStatus, pOut->qmdVA, pOut->kernelVA, pOut->outVA);
        if (status != NV_OK) goto report;
    }

    /* 6. Poll fence + RELEASE0, then verify all 32 per-thread results. */
    {
        volatile NvU32 *pSem = (volatile NvU32 *)(pBufCpu + ECLIPSE_THREADS_SEM_OFF);
        volatile NvU32 *pFence = (volatile NvU32 *)(pBufCpu + ECLIPSE_THREADS_FENCE_OFF);
        volatile NvU32 *pRes = (volatile NvU32 *)(pBufCpu + ECLIPSE_THREADS_OUT_OFF);
        NvU32 i;
        for (i = 0; i < ECLIPSE_LAUNCH_POLL_MS2; i++)
        {
            if (pOut->fenceStatus != NV_OK && *pFence == ECLIPSE_THREADS_FENCE_PAYLOAD)
            {
                pOut->fenceStatus = NV_OK;
                pOut->fenceIters = i;
            }
            if (pOut->semStatus != NV_OK && *pSem == ECLIPSE_THREADS_SEM_PAYLOAD)
            {
                pOut->semStatus = NV_OK;
                pOut->semIters = i;
            }
            if (pOut->fenceStatus == NV_OK && pOut->semStatus == NV_OK)
                break;
            os_delay_us(1000);
        }
        if (pOut->fenceStatus != NV_OK) { pOut->fenceStatus = NV_ERR_TIMEOUT; pOut->fenceIters = i; }
        if (pOut->semStatus != NV_OK)   { pOut->semStatus = NV_ERR_TIMEOUT;   pOut->semIters = i; }

        pOut->matchCount = 0;
        for (i = 0; i < 32; i++)
        {
            NvU32 v = pRes[i];
            if (v == 3u * i + 7u)
            {
                pOut->matchCount++;
            }
            else if (pOut->firstBadIdx == 0xFFFFFFFF)
            {
                pOut->firstBadIdx = i;
                pOut->firstBadVal = v;
            }
        }
        pOut->verifyStatus = (pOut->matchCount == 32) ? NV_OK : NV_ERR_INVALID_DATA;
        nv_printf(0, "[eclipse-rm-trace] step21: fence 0x%x (@%u ms) sem 0x%x (@%u ms) verify 0x%x (%u/32, firstBad=%u val=0x%x)\n",
                  pOut->fenceStatus, pOut->fenceIters, pOut->semStatus, pOut->semIters,
                  pOut->verifyStatus, pOut->matchCount, pOut->firstBadIdx, pOut->firstBadVal);
    }

report:
    if (pBufCpu != NULL)
        memmgrMemDescEndTransfer(pMemoryManager, pBufMemDesc, TRANSFER_FLAGS_NONE);
    if (pUserdCpu != NULL)
        memmgrMemDescEndTransfer(pMemoryManager, pUserdMemDesc, userdFlags);

    if (pOut->semStatus == NV_OK && pOut->verifyStatus == NV_OK)
    {
        portMemCopy(&g_grThreadsCache, sizeof(g_grThreadsCache), pOut, sizeof(*pOut));
        g_grThreadsDone = NV_TRUE;
    }

    rmGpuLocksRelease(GPUS_LOCK_FLAGS_NONE, NULL);
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return NV_OK; /* per-stage statuses carry any failure */
}

/*
 * Step-22: CHIP-SCALE parallel compute -- 68 CTAs x 32 threads = 2176
 * threads across all 34 SMs (two waves), each computing its global ID and
 * storing its own result: out[gid] = gid*3 + 7, gid = ctaid*32 + tid.
 * Zero new instruction forms vs step21: adds a second S2R (SR_CTAID.X =
 * idx 37, corpus-verbatim) on write-barrier 1 and one more IMAD that
 * waits on BOTH barriers (mask 0b11). The QMD is the proven step21 QMD
 * with CTA_RASTER_WIDTH = 68. CPU verifies all 2176 output dwords --
 * results only a correct chip-wide dispatch can produce (SM/CTA
 * scheduling order is irrelevant: each thread owns its slot).
 */
#define ECLIPSE_GRID_KERNEL_OFF 0x1600
#define ECLIPSE_GRID_QMD_OFF    0x2400
#define ECLIPSE_GRID_OUT_OFF    0x4000   /* 68*32 dwords = 8704 B */
#define ECLIPSE_GRID_CTAS       68
#define ECLIPSE_GRID_THREADS    (ECLIPSE_GRID_CTAS * 32)
#define ECLIPSE_GRID_SEM_OFF    0x8300
#define ECLIPSE_GRID_SEM_PAYLOAD 0x5A55C0D4
#define ECLIPSE_GRID_FENCE_OFF  0x8340
#define ECLIPSE_GRID_FENCE_PAYLOAD 0xFE7C4ED4

/* SM75, 16 instructions; patch dwords 9 (out_lo) and 13 (out_hi).
 *   S2R R0, SR_TID.X (wr0) ; S2R R1, SR_CTAID.X (wr1)
 *   MOV R2/R3, out ; MOV R9, 7
 *   IMAD R0, R1, 32, R0   (wait b0+b1)  gid
 *   IMAD R8, R0, 3, R9 ; IMAD.WIDE.U32 R2, R0, 4, R2
 *   STG.E.SYS [R2], R8 ; EXIT ; NOP pad                    */
static const NvU32 g_sm75GridKernel[64] = {
    0x00007919, 0x00000000, 0x00002100, 0x000e0200,
    0x00017919, 0x00000000, 0x00002500, 0x000e4200,
    0x00027802, 0xDEAD0000, 0x00000f00, 0x000fc400,
    0x00037802, 0x00000000, 0x00000f00, 0x000fc400,
    0x00097802, 0x00000007, 0x00000f00, 0x000fc400,
    0x01007824, 0x00000020, 0x078e0200, 0x003fca00,
    0x00087824, 0x00000003, 0x078e0209, 0x000fca00,
    0x00027825, 0x00000004, 0x078e0002, 0x000fca00,
    0x02007386, 0x00000008, 0x0010e900, 0x000fc400,
    0x0000794d, 0x00000000, 0x03800000, 0x000fea00,
    0x00007918, 0x00000000, 0x00000000, 0x000fc000,
    0x00007918, 0x00000000, 0x00000000, 0x000fc000,
    0x00007918, 0x00000000, 0x00000000, 0x000fc000,
    0x00007918, 0x00000000, 0x00000000, 0x000fc000,
    0x00007918, 0x00000000, 0x00000000, 0x000fc000,
    0x00007918, 0x00000000, 0x00000000, 0x000fc000,
};

static EclipseGrThreads g_grGridCache;
static NvBool g_grGridDone = NV_FALSE;

NV_STATUS eclipse_rm_step22(NvU32 gpuInstance, EclipseGrThreads *pOut)
{
    OBJGPU *pGpu;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;
    RsClient *pRsClient = NULL;
    KernelChannel *pKernelChannel = NULL;
    KernelFifo *pKernelFifo;
    MemoryManager *pMemoryManager;
    Memory *pBufMemory = NULL;
    MEMORY_DESCRIPTOR *pBufMemDesc = NULL;
    MEMORY_DESCRIPTOR *pUserdMemDesc = NULL;
    NvU8 *pBufCpu = NULL;
    NvU8 *pUserdCpu = NULL;
    NvU32 userdFlags = TRANSFER_FLAGS_USE_BAR1 |
                       TRANSFER_FLAGS_SHADOW_ALLOC |
                       TRANSFER_FLAGS_SHADOW_INIT_MEM;

    if (pOut == NULL)
    {
        return NV_ERR_INVALID_ARGUMENT;
    }
    if (g_grGridDone)
    {
        portMemCopy(pOut, sizeof(*pOut), &g_grGridCache, sizeof(g_grGridCache));
        return NV_OK;
    }
    portMemSet(pOut, 0, sizeof(*pOut));
    pOut->lookupStatus = 0xFFFFFFFF;
    pOut->mapStatus    = 0xFFFFFFFF;
    pOut->tokenStatus  = 0xFFFFFFFF;
    pOut->submitStatus = 0xFFFFFFFF;
    pOut->fenceStatus  = 0xFFFFFFFF;
    pOut->semStatus    = 0xFFFFFFFF;
    pOut->verifyStatus = 0xFFFFFFFF;
    pOut->firstBadIdx  = 0xFFFFFFFF;

    if (!g_grChanDone)
    {
        return NV_ERR_INVALID_STATE; /* run step17 first */
    }

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);
    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }
    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    status = rmGpuLocksAcquire(GPUS_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        rmapiLockRelease();
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pKernelFifo = GPU_GET_KERNEL_FIFO(pGpu);
    pMemoryManager = GPU_GET_MEMORY_MANAGER(pGpu);

    /* 1. Locate channel/buffer/USERD. */
    {
        NvU32 subdevInst;
        status = serverGetClientUnderLock(&g_resServ, g_grAllocCache.hClient, &pRsClient);
        if (status == NV_OK)
            status = CliGetKernelChannel(pRsClient, g_grChanCache.hChannel, &pKernelChannel);
        if (status == NV_OK)
            status = memGetByHandle(pRsClient, g_grChanCache.hPhysBuf, &pBufMemory);
        if (status == NV_OK)
        {
            pBufMemDesc = pBufMemory->pMemDesc;
            subdevInst = gpumgrGetSubDeviceInstanceFromGpu(pGpu);
            pUserdMemDesc = pKernelChannel->pUserdSubDeviceMemDesc[subdevInst];
            if (pBufMemDesc == NULL || pUserdMemDesc == NULL)
                status = NV_ERR_INVALID_STATE;
        }
        pOut->lookupStatus = status;
        if (status != NV_OK) goto report;
    }

    /* 2. CPU maps. */
    {
        pBufCpu = memmgrMemDescBeginTransfer(pMemoryManager, pBufMemDesc, TRANSFER_FLAGS_NONE);
        pUserdCpu = memmgrMemDescBeginTransfer(pMemoryManager, pUserdMemDesc, userdFlags);
        pOut->mapStatus = (pBufCpu != NULL && pUserdCpu != NULL) ? NV_OK : NV_ERR_GENERIC;
        if (pOut->mapStatus != NV_OK) goto report;
    }

    pOut->kernelVA = g_grChanCache.bufGpuVA + ECLIPSE_GRID_KERNEL_OFF;
    pOut->qmdVA    = g_grChanCache.bufGpuVA + ECLIPSE_GRID_QMD_OFF;
    pOut->outVA    = g_grChanCache.bufGpuVA + ECLIPSE_GRID_OUT_OFF;

    /* 3. Patch + write the kernel; zero out[]; build the QMD (width 68). */
    {
        NvU32 kern[64];
        NvU32 *qmd = (NvU32 *)(pBufCpu + ECLIPSE_GRID_QMD_OFF);
        NvU64 semVA = g_grChanCache.bufGpuVA + ECLIPSE_GRID_SEM_OFF;
        NvU32 i;
        portMemCopy(kern, sizeof(kern), g_sm75GridKernel, sizeof(g_sm75GridKernel));
        kern[9]  = NvU64_LO32(pOut->outVA);
        kern[13] = NvU64_HI32(pOut->outVA);
        portMemCopy(pBufCpu + ECLIPSE_GRID_KERNEL_OFF, sizeof(kern), kern, sizeof(kern));
        for (i = 0; i < ECLIPSE_GRID_THREADS; i++)
            ((volatile NvU32 *)(pBufCpu + ECLIPSE_GRID_OUT_OFF))[i] = 0;
        *(volatile NvU32 *)(pBufCpu + ECLIPSE_GRID_SEM_OFF) = 0;
        *(volatile NvU32 *)(pBufCpu + ECLIPSE_GRID_FENCE_OFF) = 0;

        portMemSet(qmd, 0, 256);
        QMD_SET(qmd, QMDF_QMD_MAJOR_VERSION, 2);
        QMD_SET(qmd, QMDF_QMD_VERSION, 2);
        QMD_SET(qmd, QMDF_API_VISIBLE_CALL_LIMIT, 1);
        QMD_SET(qmd, QMDF_SAMPLER_INDEX, 0);
        QMD_SET(qmd, QMDF_SM_GLOBAL_CACHING_ENABLE, 1);
        QMD_SET(qmd, QMDF_CTA_RASTER_WIDTH, ECLIPSE_GRID_CTAS);
        QMD_SET(qmd, QMDF_CTA_RASTER_HEIGHT, 1);
        QMD_SET(qmd, QMDF_CTA_RASTER_DEPTH, 1);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION0, 32);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION1, 1);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION2, 1);
        QMD_SET(qmd, QMDF_PROGRAM_ADDRESS_LOWER, (NvU32)(pOut->kernelVA & 0xFFFFFFFFu));
        QMD_SET(qmd, QMDF_PROGRAM_ADDRESS_UPPER, (NvU32)(pOut->kernelVA >> 32));
        QMD_SET(qmd, QMDF_REGISTER_COUNT_V, ECLIPSE_LAUNCH_REG_COUNT);
        QMD_SET(qmd, QMDF_MIN_SM_CONFIG_SHMEM, 9);
        QMD_SET(qmd, QMDF_TARGET_SM_CONFIG_SHMEM, 9);
        QMD_SET(qmd, QMDF_MAX_SM_CONFIG_SHMEM, 17);
        QMD_SET(qmd, QMDF_CWD_MEMBAR_TYPE, 1);
        QMD_SET(qmd, QMDF_RELEASE_MEMBAR_TYPE, 1);
        QMD_SET(qmd, QMDF_SEMAPHORE_RELEASE_ENABLE0, 1);
        QMD_SET(qmd, QMDF_RELEASE0_ADDRESS_LOWER, (NvU32)(semVA & 0xFFFFFFFFu));
        QMD_SET(qmd, QMDF_RELEASE0_ADDRESS_UPPER, (NvU32)(semVA >> 32));
        QMD_SET(qmd, QMDF_RELEASE0_STRUCTURE_SIZE, 1);
        QMD_SET(qmd, QMDF_RELEASE0_PAYLOAD, ECLIPSE_GRID_SEM_PAYLOAD);
        osFlushCpuWriteCombineBuffer();
    }

    /* 4. Token. */
    {
        pOut->tokenStatus = kfifoGenerateWorkSubmitToken(pGpu, pKernelFifo,
                                                         pKernelChannel,
                                                         &pOut->workToken, NV_TRUE);
        pOut->runlistId = kchannelGetRunlistId(pKernelChannel);
        if (pOut->tokenStatus != NV_OK) goto report;
    }

    /* 5. Launch. */
    {
        volatile NvU32 *pb = (volatile NvU32 *)pBufCpu;
        volatile NvU32 *gp = (volatile NvU32 *)(pBufCpu + ECLIPSE_CHAN_GPFIFO_OFF);
        volatile Nvc46fControl *pUserd = (volatile Nvc46fControl *)pUserdCpu;
        NvU32 n = 0, put, gpEntry0, gpEntry1;
        NvU64 pbVA = g_grChanCache.bufGpuVA;
        NvU64 fenceVA = g_grChanCache.bufGpuVA + ECLIPSE_GRID_FENCE_OFF;

        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_OBJECT, 1);
        pb[n++] = TURING_COMPUTE_A;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_SHADER_SHARED_MEMORY_WINDOW_A, 2);
        pb[n++] = 0;
        pb[n++] = 0xFE000000;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_SHADER_LOCAL_MEMORY_WINDOW_A, 2);
        pb[n++] = 0;
        pb[n++] = 0xFF000000;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_INVALIDATE_SKED_CACHES, 1);
        pb[n++] = 0;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SEND_PCAS_A, 1);
        pb[n++] = (NvU32)(pOut->qmdVA >> 8);
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SEND_SIGNALING_PCAS_B, 1);
        pb[n++] = DRF_DEF(C5C0, _SEND_SIGNALING_PCAS_B, _INVALIDATE, _TRUE) |
                  DRF_DEF(C5C0, _SEND_SIGNALING_PCAS_B, _SCHEDULE, _TRUE);
        pb[n++] = ECLIPSE_PUSH_HDR(0, NVC46F_SEM_ADDR_LO, 5);
        pb[n++] = NvU64_LO32(fenceVA);
        pb[n++] = DRF_NUM(C46F, _SEM_ADDR_HI, _OFFSET, NvU64_HI32(fenceVA));
        pb[n++] = ECLIPSE_GRID_FENCE_PAYLOAD;
        pb[n++] = 0;
        pb[n++] = DRF_DEF(C46F, _SEM_EXECUTE, _OPERATION, _RELEASE) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _RELEASE_WFI, _DIS) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _PAYLOAD_SIZE, _32BIT);
        pOut->pushDwords = n;

        put = pUserd->GPPut;
        gpEntry0 = DRF_DEF(906F, _GP_ENTRY0, _NO_CONTEXT_SWITCH, _FALSE) |
                   DRF_NUM(906F, _GP_ENTRY0, _GET, NvU64_LO32(pbVA) >> 2);
        gpEntry1 = DRF_NUM(906F, _GP_ENTRY1, _GET_HI, NvU64_HI32(pbVA)) |
                   DRF_NUM(906F, _GP_ENTRY1, _LENGTH, n) |
                   DRF_DEF(906F, _GP_ENTRY1, _LEVEL, _MAIN);
        gp[(put % ECLIPSE_CHAN_GPFIFO_ENTRIES) * 2 + 0] = gpEntry0;
        gp[(put % ECLIPSE_CHAN_GPFIFO_ENTRIES) * 2 + 1] = gpEntry1;
        osFlushCpuWriteCombineBuffer();

        pUserd->GPPut = put + 1;
        osFlushCpuWriteCombineBuffer();

        status = kbusFlushPcieForBar0Doorbell_HAL(pGpu, GPU_GET_KERNEL_BUS(pGpu));
        if (status == NV_OK)
            status = kfifoUpdateUsermodeDoorbell_HAL(pGpu, pKernelFifo,
                                                     pOut->workToken, pOut->runlistId);
        pOut->submitStatus = status;
        nv_printf(0, "[eclipse-rm-trace] step22: launch (%u dw, GPPut=%u) -> 0x%x qmd=0x%llx prog=0x%llx out=0x%llx (%u CTAs)\n",
                  n, put + 1, pOut->submitStatus, pOut->qmdVA, pOut->kernelVA, pOut->outVA,
                  (NvU32)ECLIPSE_GRID_CTAS);
        if (status != NV_OK) goto report;
    }

    /* 6. Poll fence + RELEASE0, then verify all 2176 results. */
    {
        volatile NvU32 *pSem = (volatile NvU32 *)(pBufCpu + ECLIPSE_GRID_SEM_OFF);
        volatile NvU32 *pFence = (volatile NvU32 *)(pBufCpu + ECLIPSE_GRID_FENCE_OFF);
        volatile NvU32 *pRes = (volatile NvU32 *)(pBufCpu + ECLIPSE_GRID_OUT_OFF);
        NvU32 i;
        for (i = 0; i < ECLIPSE_LAUNCH_POLL_MS2; i++)
        {
            if (pOut->fenceStatus != NV_OK && *pFence == ECLIPSE_GRID_FENCE_PAYLOAD)
            {
                pOut->fenceStatus = NV_OK;
                pOut->fenceIters = i;
            }
            if (pOut->semStatus != NV_OK && *pSem == ECLIPSE_GRID_SEM_PAYLOAD)
            {
                pOut->semStatus = NV_OK;
                pOut->semIters = i;
            }
            if (pOut->fenceStatus == NV_OK && pOut->semStatus == NV_OK)
                break;
            os_delay_us(1000);
        }
        if (pOut->fenceStatus != NV_OK) { pOut->fenceStatus = NV_ERR_TIMEOUT; pOut->fenceIters = i; }
        if (pOut->semStatus != NV_OK)   { pOut->semStatus = NV_ERR_TIMEOUT;   pOut->semIters = i; }

        pOut->matchCount = 0;
        for (i = 0; i < ECLIPSE_GRID_THREADS; i++)
        {
            NvU32 v = pRes[i];
            if (v == 3u * i + 7u)
            {
                pOut->matchCount++;
            }
            else if (pOut->firstBadIdx == 0xFFFFFFFF)
            {
                pOut->firstBadIdx = i;
                pOut->firstBadVal = v;
            }
        }
        pOut->verifyStatus = (pOut->matchCount == ECLIPSE_GRID_THREADS)
                                 ? NV_OK : NV_ERR_INVALID_DATA;
        nv_printf(0, "[eclipse-rm-trace] step22: fence 0x%x (@%u ms) sem 0x%x (@%u ms) verify 0x%x (%u/%u, firstBad=%u val=0x%x)\n",
                  pOut->fenceStatus, pOut->fenceIters, pOut->semStatus, pOut->semIters,
                  pOut->verifyStatus, pOut->matchCount, (NvU32)ECLIPSE_GRID_THREADS,
                  pOut->firstBadIdx, pOut->firstBadVal);
    }

report:
    if (pBufCpu != NULL)
        memmgrMemDescEndTransfer(pMemoryManager, pBufMemDesc, TRANSFER_FLAGS_NONE);
    if (pUserdCpu != NULL)
        memmgrMemDescEndTransfer(pMemoryManager, pUserdMemDesc, userdFlags);

    if (pOut->semStatus == NV_OK && pOut->verifyStatus == NV_OK)
    {
        portMemCopy(&g_grGridCache, sizeof(g_grGridCache), pOut, sizeof(*pOut));
        g_grGridDone = NV_TRUE;
    }

    rmGpuLocksRelease(GPUS_LOCK_FLAGS_NONE, NULL);
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return NV_OK; /* per-stage statuses carry any failure */
}

/*
 * Step-23: the load-compute-store canon -- integer SAXPY. Each of 32
 * threads loads x[tid] and y[tid] from two GPU arrays, computes
 * y[tid] = a*x[tid] + y[tid], and stores it back. This is the first
 * kernel that READS from memory (global loads) rather than only writing:
 * the missing third of the fundamental GPU compute pattern.
 *
 * The one new instruction form vs step22 is LDG.E.SYS Rd,[Ra] (global
 * load, GPR-addressed). Its low word is validated bit-exactly against
 * three CuAssembler sm_75 corpus instances (opcode 0x381 -- the form real
 * nvdisasm emits for [Rn] addressing, not NAK's 0x981 uniform-base path);
 * the high/control word carries the .E (64-bit address, bit 72) and .SYS
 * scope bits, derived from the corpus CONSTANT/SYS/width deltas. Loads are
 * variable-latency, so each LDG sets a write barrier and the consuming
 * IMAD waits on both. IMAD Rd,Ra,Rb,Rc (register form) is corpus-verbatim;
 * everything else (S2R, IMAD.WIDE, STG, MOV-imm) is silicon-proven.
 *
 * Params (a, x base, y base) are patched into the SASS immediates (proven
 * in step20); CPU prefills x[i]=i and y[i]=100+i, so the expected result
 * is y[i] = 3*i + 100 + i = 4*i + 100 -- values only a correct
 * load->compute->store roundtrip produces. All 32 verified.
 */
#define ECLIPSE_SAXPY_KERNEL_OFF 0x1800
#define ECLIPSE_SAXPY_QMD_OFF    0x2600
#define ECLIPSE_SAXPY_X_OFF      0x5000   /* x[32] */
#define ECLIPSE_SAXPY_Y_OFF      0x5100   /* y[32] (also the output) */
#define ECLIPSE_SAXPY_DBGX_OFF   0x5200   /* debug: loaded x per thread */
#define ECLIPSE_SAXPY_DBGY_OFF   0x5300   /* debug: loaded y per thread */
#define ECLIPSE_SAXPY_A          3
#define ECLIPSE_SAXPY_SEM_OFF    0x8400
#define ECLIPSE_SAXPY_SEM_PAYLOAD 0x5A55C0D5
#define ECLIPSE_SAXPY_FENCE_OFF  0x8440
#define ECLIPSE_SAXPY_FENCE_PAYLOAD 0xFE7C4ED5

/* SM75 -- RUN #1 REPLAY (byte-exact), the empirical anchor.
 *
 * Three probe kernels in a row hung at their first LDG with mutually
 * inconsistent fault reports (VA 0x1_00000000 then 0x0), while stores
 * and ifetch always work. The ONLY kernel whose loads ever COMPLETED on
 * this silicon is run #1 (5b131eb7): fence+RELEASE0 fired, y[] was
 * written (zeros). So: replay run #1's 16 instructions VERBATIM -- same
 * bytes, same patch slots, same QMD -- and only make the CPU seeds more
 * diagnostic. Outcomes: (a) completes with y=3x+y -> SAXPY done;
 * (b) completes with zeros -> loads execute but sysmem reads are
 * incoherent -> stage data in VRAM next; (c) hangs -> the environment
 * (not the kernel) changed since run #1 and the hunt moves there.
 *
 * y[i] signatures (x seeded 0x1000+i, y seeded 100+i, a=3):
 *   0x3064+4i = loads+IMAD+store all correct
 *   0         = loads executed but read zero (run #1''s result)
 *   0x1000+i  = store used stale R8 (pre-IMAD load value)
 *   100+i     = store never landed
 * Patched dwords: 5=a, 9=x_lo, 13=x_hi, 17=y_lo, 21=y_hi.
 *   S2R R0 (wr0); MOV R4,a; MOV R2,x_lo; MOV R3,x_hi; MOV R6,y_lo;
 *   MOV R7,y_hi; IMAD.WIDE R2,R0,4,R2 (wait0); IMAD.WIDE R6,R0,4,R6
 *   (wait0); LDG.E.SYS R8,[R2] (wr1); LDG.E.SYS R9,[R6] (wr2);
 *   IMAD R8,R8,R4,R9 (wait1+2); STG.E.SYS [R6],R8; EXIT; NOP pad     */
static const NvU32 g_sm75SaxpyKernel[80] = {
    0x00007919, 0x00000000, 0x00002100, 0x000e0200, /* 0  S2R R0,SR_TID.X (wr0)         */
    0x00047802, 0xAAAA0000, 0x00000f00, 0x000fc400, /* 1  MOV R4, a      [patch 5]      */
    0x00027802, 0xBBBB0000, 0x00000f00, 0x000fc400, /* 2  MOV R2, x_lo   [patch 9]      */
    0x00037802, 0x00000000, 0x00000f00, 0x000fc400, /* 3  MOV R3, x_hi   [patch 13]     */
    0x00067802, 0xCCCC0000, 0x00000f00, 0x000fc400, /* 4  MOV R6, y_lo   [patch 17]     */
    0x00077802, 0x00000000, 0x00000f00, 0x000fc400, /* 5  MOV R7, y_hi   [patch 21]     */
    0x00027825, 0x00000004, 0x078e0002, 0x001fc800, /* 6  IMAD.WIDE R2,R0,4,R2 (wait 0) */
    0x00067825, 0x00000004, 0x078e0006, 0x001fc800, /* 7  IMAD.WIDE R6,R0,4,R6 (wait 0) */
    0x02087381, 0x00000000, 0x001ee900, 0x000e4400, /* 8  LDG.E.SYS R8,[R2] (wr1)       */
    0x06097381, 0x00000000, 0x001ee900, 0x000e8400, /* 9  LDG.E.SYS R9,[R6] (wr2)       */
    0x08087224, 0x00000004, 0x078e0209, 0x006fd000, /* 10 IMAD R8,R8,R4,R9 (wait 1+2, DELAY 8)
        THE FINAL FIX: the replay boot proved the loads read correct
        sysmem data (y[i] came back = x[i] exactly) and the only failure
        was this IMAD's result not being in R8 when the STG read it 4
        cycles later -- the SM75 6-cycle integer-ALU latency law, in its
        true home. run #1's famous "loads returned zero" was x[0]=0 seed
        + this same stale-R8 store all along; no coherence problem ever
        existed. Delay 4 -> 8.                                          */
    0x06007386, 0x00000008, 0x0010e900, 0x000fc400, /* 11 STG.E.SYS [R6],R8             */
    0x0000794d, 0x00000000, 0x03800000, 0x000fea00, /* 12 EXIT                          */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* 13 NOP                           */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* 14 NOP                           */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* 15 NOP                           */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* 16 NOP                           */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* 17 NOP                           */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* 18 NOP                           */
    0x00007918, 0x00000000, 0x00000000, 0x000fc000, /* 19 NOP                           */
};

static EclipseGrThreads g_grSaxpyCache;
static NvBool g_grSaxpyDone = NV_FALSE;

/* A dedicated GPU_CACHEABLE=YES sysmem buffer the SM can actually READ.
 * The default sysmem alloc is GPU_CACHEABLE=NO (mem_mgr_gm107.c: "the GPU
 * cache is not sysmem coherent"), which is why SM data loads from the main
 * channel buffer fault/hang while stores and ifetch work. Allocated once,
 * under the API lock only (allocation asserts the GPU lock is NOT held),
 * and mapped into the compute VAS. */
static NvU64  g_saxpyCacheableVA = 0;
static NvU32  g_saxpyCacheableHandle = 0;
static NvBool g_saxpyCacheableDone = NV_FALSE;
#define ECLIPSE_SAXPY_CACHEABLE_SIZE 0x1000

NV_STATUS eclipse_rm_step23(NvU32 gpuInstance, EclipseGrThreads *pOut)
{
    OBJGPU *pGpu;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;
    RsClient *pRsClient = NULL;
    KernelChannel *pKernelChannel = NULL;
    KernelFifo *pKernelFifo;
    MemoryManager *pMemoryManager;
    Memory *pBufMemory = NULL;
    MEMORY_DESCRIPTOR *pBufMemDesc = NULL;
    MEMORY_DESCRIPTOR *pUserdMemDesc = NULL;
    Memory *pCacheMemory = NULL;
    MEMORY_DESCRIPTOR *pCacheMemDesc = NULL;
    NvU8 *pBufCpu = NULL;
    NvU8 *pUserdCpu = NULL;
    NvU8 *pCacheCpu = NULL;
    NvU32 userdFlags = TRANSFER_FLAGS_USE_BAR1 |
                       TRANSFER_FLAGS_SHADOW_ALLOC |
                       TRANSFER_FLAGS_SHADOW_INIT_MEM;

    if (pOut == NULL)
    {
        return NV_ERR_INVALID_ARGUMENT;
    }
    if (g_grSaxpyDone)
    {
        portMemCopy(pOut, sizeof(*pOut), &g_grSaxpyCache, sizeof(g_grSaxpyCache));
        return NV_OK;
    }
    portMemSet(pOut, 0, sizeof(*pOut));
    pOut->faultCtrlStatus = 0xFFFFFFFF; /* not queried */
    pOut->lookupStatus = 0xFFFFFFFF;
    pOut->mapStatus    = 0xFFFFFFFF;
    pOut->tokenStatus  = 0xFFFFFFFF;
    pOut->submitStatus = 0xFFFFFFFF;
    pOut->fenceStatus  = 0xFFFFFFFF;
    pOut->semStatus    = 0xFFFFFFFF;
    pOut->verifyStatus = 0xFFFFFFFF;
    pOut->firstBadIdx  = 0xFFFFFFFF;

    if (!g_grChanDone)
    {
        return NV_ERR_INVALID_STATE; /* run step17 first */
    }

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);
    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }
    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    /* DISABLED (was the load-fault poison): mapping this buffer into the
     * LIVE compute VAS at runtime via NV50_MEMORY_VIRTUAL+Map, without a
     * GMMU TLB invalidate, left stale TLB state so EVERY subsequent SM
     * global LOAD faulted (MMU_FAULT_QUEUED) -- even loads from the main
     * channel buffer, which stores reach fine. run #1 (no such alloc)
     * loaded from the main buffer without faulting. Re-enable only paired
     * with an explicit GMMU invalidate. */
    if (0 && !g_saxpyCacheableDone)
    {
        RM_API *pAlloc = rmapiGetInterface(RMAPI_GPU_LOCK_INTERNAL);
        RsClient *pAllocClient = NULL;
        NvU32 hPhys = 0, hVirt = 0;
        NV_STATUS as = serverGetClientUnderLock(&g_resServ, g_grAllocCache.hClient, &pAllocClient);
        if (as == NV_OK)
        {
            NV_MEMORY_ALLOCATION_PARAMS mp;
            portMemSet(&mp, 0, sizeof(mp));
            mp.owner = HEAP_OWNER_RM_CLIENT_GENERIC;
            mp.type  = NVOS32_TYPE_IMAGE;
            mp.size  = ECLIPSE_SAXPY_CACHEABLE_SIZE;
            mp.attr  = DRF_DEF(OS32, _ATTR, _LOCATION, _PCI);
            mp.attr2 = DRF_DEF(OS32, _ATTR2, _GPU_CACHEABLE, _YES);
            as = clientGenResourceHandle(pAllocClient, &hPhys);
            if (as == NV_OK)
                as = pAlloc->AllocWithHandle(pAlloc, g_grAllocCache.hClient,
                                             g_grAllocCache.hDevice, hPhys,
                                             NV01_MEMORY_SYSTEM, &mp, sizeof(mp));
        }
        if (as == NV_OK)
        {
            NV_MEMORY_ALLOCATION_PARAMS mp;
            portMemSet(&mp, 0, sizeof(mp));
            mp.owner = HEAP_OWNER_RM_CLIENT_GENERIC;
            mp.type  = NVOS32_TYPE_IMAGE;
            mp.size  = ECLIPSE_SAXPY_CACHEABLE_SIZE;
            mp.attr  = DRF_DEF(OS32, _ATTR, _LOCATION, _PCI);
            mp.flags = NVOS32_ALLOC_FLAGS_VIRTUAL;
            mp.hVASpace = g_grAllocCache.hVas;
            as = clientGenResourceHandle(pAllocClient, &hVirt);
            if (as == NV_OK)
                as = pAlloc->AllocWithHandle(pAlloc, g_grAllocCache.hClient,
                                             g_grAllocCache.hDevice, hVirt,
                                             NV50_MEMORY_VIRTUAL, &mp, sizeof(mp));
        }
        if (as == NV_OK)
            as = pAlloc->Map(pAlloc, g_grAllocCache.hClient, g_grAllocCache.hDevice,
                             hVirt, hPhys, 0, ECLIPSE_SAXPY_CACHEABLE_SIZE,
                             NV04_MAP_MEMORY_FLAGS_NONE, &g_saxpyCacheableVA);
        nv_printf(0, "[eclipse-rm-trace] step23: cacheable buf alloc -> 0x%x hPhys=0x%x VA=0x%llx\n",
                  as, hPhys, g_saxpyCacheableVA);
        if (as == NV_OK)
        {
            g_saxpyCacheableHandle = hPhys;
            g_saxpyCacheableDone = NV_TRUE;
        }
    }

    status = rmGpuLocksAcquire(GPUS_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        rmapiLockRelease();
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pKernelFifo = GPU_GET_KERNEL_FIFO(pGpu);
    pMemoryManager = GPU_GET_MEMORY_MANAGER(pGpu);

    /* 1. Locate channel/buffer/USERD. */
    {
        NvU32 subdevInst;
        status = serverGetClientUnderLock(&g_resServ, g_grAllocCache.hClient, &pRsClient);
        if (status == NV_OK)
            status = CliGetKernelChannel(pRsClient, g_grChanCache.hChannel, &pKernelChannel);
        if (status == NV_OK)
            status = memGetByHandle(pRsClient, g_grChanCache.hPhysBuf, &pBufMemory);
        if (status == NV_OK)
        {
            pBufMemDesc = pBufMemory->pMemDesc;
            subdevInst = gpumgrGetSubDeviceInstanceFromGpu(pGpu);
            pUserdMemDesc = pKernelChannel->pUserdSubDeviceMemDesc[subdevInst];
            if (pBufMemDesc == NULL || pUserdMemDesc == NULL)
                status = NV_ERR_INVALID_STATE;
        }
        if (status == NV_OK && g_saxpyCacheableHandle != 0)
        {
            status = memGetByHandle(pRsClient, g_saxpyCacheableHandle, &pCacheMemory);
            if (status == NV_OK)
            {
                pCacheMemDesc = pCacheMemory->pMemDesc;
                if (pCacheMemDesc == NULL) status = NV_ERR_INVALID_STATE;
            }
        }
        pOut->lookupStatus = status;
        if (status != NV_OK) goto report;
    }

    /* 2. CPU maps (channel buffer, USERD, and the cacheable data buffer). */
    {
        pBufCpu = memmgrMemDescBeginTransfer(pMemoryManager, pBufMemDesc, TRANSFER_FLAGS_NONE);
        pUserdCpu = memmgrMemDescBeginTransfer(pMemoryManager, pUserdMemDesc, userdFlags);
        if (pCacheMemDesc != NULL)
            pCacheCpu = memmgrMemDescBeginTransfer(pMemoryManager, pCacheMemDesc, TRANSFER_FLAGS_NONE);
        pOut->mapStatus = (pBufCpu != NULL && pUserdCpu != NULL &&
                           (pCacheMemDesc == NULL || pCacheCpu != NULL)) ? NV_OK : NV_ERR_GENERIC;
        if (pOut->mapStatus != NV_OK) goto report;
    }

    pOut->kernelVA = g_grChanCache.bufGpuVA + ECLIPSE_SAXPY_KERNEL_OFF;
    pOut->qmdVA    = g_grChanCache.bufGpuVA + ECLIPSE_SAXPY_QMD_OFF;
    pOut->outVA    = g_grChanCache.bufGpuVA + ECLIPSE_SAXPY_Y_OFF;

    /* 3. Prefill x[i]=i, y[i]=100+i; patch a/x/y; build the QMD. */
    {
        /* MUST be >= g_sm75SaxpyKernel's length: portMemCopy is a silent
         * NO-OP (returns NULL, copies nothing) when srcSize > destSize, so
         * a kern[] smaller than the kernel array leaves it uninitialised
         * stack garbage -> the SM fetches garbage and the GSP RCs the
         * channel before a single instruction retires. This was THE step23
         * launch bug: the kernel grew from 64 to 80 dwords, kern[] didn't. */
        NvU32 kern[80];
        NvU32 *qmd = (NvU32 *)(pBufCpu + ECLIPSE_SAXPY_QMD_OFF);
        NvU64 semVA = g_grChanCache.bufGpuVA + ECLIPSE_SAXPY_SEM_OFF;
        NvU64 xVA = g_grChanCache.bufGpuVA + ECLIPSE_SAXPY_X_OFF;
        NvU64 yVA = pOut->outVA;
        NvU64 dxVA = g_grChanCache.bufGpuVA + ECLIPSE_SAXPY_DBGX_OFF;
        NvU64 dyVA = g_grChanCache.bufGpuVA + ECLIPSE_SAXPY_DBGY_OFF;
        volatile NvU32 *px = (volatile NvU32 *)(pBufCpu + ECLIPSE_SAXPY_X_OFF);
        volatile NvU32 *py = (volatile NvU32 *)(pBufCpu + ECLIPSE_SAXPY_Y_OFF);
        volatile NvU32 *pdx = (volatile NvU32 *)(pBufCpu + ECLIPSE_SAXPY_DBGX_OFF);
        volatile NvU32 *pdy = (volatile NvU32 *)(pBufCpu + ECLIPSE_SAXPY_DBGY_OFF);
        NvU32 i;
        (void)dxVA; (void)dyVA; (void)pdx; (void)pdy; (void)g_saxpyCacheableVA;
        /* run #1 replay seeds, made maximally diagnostic. y[i] signatures:
         * 0x3064+4i = full 3*x+y correct; 0 = loads executed but read
         * zero (run #1's original result); 0x1000+i = store used stale
         * pre-IMAD R8 (= the raw x load); 100+i = store never landed. */
        for (i = 0; i < 32; i++)
        {
            px[i] = 0x1000u + i;
            py[i] = 100u + i;
        }

        /* (cacheable buffer disabled -- see the alloc block.) */
        if (pCacheCpu != NULL)
        {
            volatile NvU32 *pc = (volatile NvU32 *)pCacheCpu;
            for (i = 0; i < 32; i++) pc[i] = 0x12340000u | i;
        }

        portMemCopy(kern, sizeof(kern), g_sm75SaxpyKernel, sizeof(g_sm75SaxpyKernel));
        /* run #1's exact patch map: a, then the x pair, then the y pair. */
        kern[5]  = (NvU32)ECLIPSE_SAXPY_A;      /* MOV R4, a      */
        kern[9]  = NvU64_LO32(xVA);             /* MOV R2, x lo   */
        kern[13] = NvU64_HI32(xVA);             /* MOV R3, x hi   */
        kern[17] = NvU64_LO32(yVA);             /* MOV R6, y lo   */
        kern[21] = NvU64_HI32(yVA);             /* MOV R7, y hi   */
        portMemCopy(pBufCpu + ECLIPSE_SAXPY_KERNEL_OFF, sizeof(kern), kern, sizeof(kern));
        /* Read back the patched immediates from the GPU-visible upload
         * (early print, survives capture truncation). */
        {
            volatile NvU32 *pk = (volatile NvU32 *)(pBufCpu + ECLIPSE_SAXPY_KERNEL_OFF);
            nv_printf(0, "[eclipse-rm-trace] step23 uploaded patches: dw5=0x%x dw9=0x%x dw13=0x%x dw17=0x%x dw21=0x%x (want a/x_lo/x_hi/y_lo/y_hi)\n",
                      pk[5], pk[9], pk[13], pk[17], pk[21]);
        }
        *(volatile NvU32 *)(pBufCpu + ECLIPSE_SAXPY_SEM_OFF) = 0;
        *(volatile NvU32 *)(pBufCpu + ECLIPSE_SAXPY_FENCE_OFF) = 0;

        portMemSet(qmd, 0, 256);
        QMD_SET(qmd, QMDF_QMD_MAJOR_VERSION, 2);
        QMD_SET(qmd, QMDF_QMD_VERSION, 2);
        QMD_SET(qmd, QMDF_API_VISIBLE_CALL_LIMIT, 1);
        QMD_SET(qmd, QMDF_SAMPLER_INDEX, 0);
        /* Exactly the proven QMD of run #1 / steps 19-22: caching enabled,
         * no invalidate bits. The MMU faults were never the QMD -- they
         * were a wild-address load (rd/ra swapped in the second LDG). */
        QMD_SET(qmd, QMDF_SM_GLOBAL_CACHING_ENABLE, 1);
        QMD_SET(qmd, QMDF_CTA_RASTER_WIDTH, 1);
        QMD_SET(qmd, QMDF_CTA_RASTER_HEIGHT, 1);
        QMD_SET(qmd, QMDF_CTA_RASTER_DEPTH, 1);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION0, 32);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION1, 1);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION2, 1);
        QMD_SET(qmd, QMDF_PROGRAM_ADDRESS_LOWER, (NvU32)(pOut->kernelVA & 0xFFFFFFFFu));
        QMD_SET(qmd, QMDF_PROGRAM_ADDRESS_UPPER, (NvU32)(pOut->kernelVA >> 32));
        QMD_SET(qmd, QMDF_REGISTER_COUNT_V, ECLIPSE_LAUNCH_REG_COUNT);
        QMD_SET(qmd, QMDF_MIN_SM_CONFIG_SHMEM, 9);
        QMD_SET(qmd, QMDF_TARGET_SM_CONFIG_SHMEM, 9);
        QMD_SET(qmd, QMDF_MAX_SM_CONFIG_SHMEM, 17);
        QMD_SET(qmd, QMDF_CWD_MEMBAR_TYPE, 1);
        QMD_SET(qmd, QMDF_RELEASE_MEMBAR_TYPE, 1);
        QMD_SET(qmd, QMDF_SEMAPHORE_RELEASE_ENABLE0, 1);
        QMD_SET(qmd, QMDF_RELEASE0_ADDRESS_LOWER, (NvU32)(semVA & 0xFFFFFFFFu));
        QMD_SET(qmd, QMDF_RELEASE0_ADDRESS_UPPER, (NvU32)(semVA >> 32));
        QMD_SET(qmd, QMDF_RELEASE0_STRUCTURE_SIZE, 1);
        QMD_SET(qmd, QMDF_RELEASE0_PAYLOAD, ECLIPSE_SAXPY_SEM_PAYLOAD);
        osFlushCpuWriteCombineBuffer();
    }

    /* 4. Token. */
    {
        pOut->tokenStatus = kfifoGenerateWorkSubmitToken(pGpu, pKernelFifo,
                                                         pKernelChannel,
                                                         &pOut->workToken, NV_TRUE);
        pOut->runlistId = kchannelGetRunlistId(pKernelChannel);
        if (pOut->tokenStatus != NV_OK) goto report;
    }

    /* 5. Launch. */
    {
        volatile NvU32 *pb = (volatile NvU32 *)pBufCpu;
        volatile NvU32 *gp = (volatile NvU32 *)(pBufCpu + ECLIPSE_CHAN_GPFIFO_OFF);
        volatile Nvc46fControl *pUserd = (volatile Nvc46fControl *)pUserdCpu;
        NvU32 n = 0, put, gpEntry0, gpEntry1;
        NvU64 pbVA = g_grChanCache.bufGpuVA;
        NvU64 fenceVA = g_grChanCache.bufGpuVA + ECLIPSE_SAXPY_FENCE_OFF;

        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_OBJECT, 1);
        pb[n++] = TURING_COMPUTE_A;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_SHADER_SHARED_MEMORY_WINDOW_A, 2);
        pb[n++] = 0;
        pb[n++] = 0xFE000000;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_SHADER_LOCAL_MEMORY_WINDOW_A, 2);
        pb[n++] = 0;
        pb[n++] = 0xFF000000;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_INVALIDATE_SKED_CACHES, 1);
        pb[n++] = 0;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SEND_PCAS_A, 1);
        pb[n++] = (NvU32)(pOut->qmdVA >> 8);
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SEND_SIGNALING_PCAS_B, 1);
        pb[n++] = DRF_DEF(C5C0, _SEND_SIGNALING_PCAS_B, _INVALIDATE, _TRUE) |
                  DRF_DEF(C5C0, _SEND_SIGNALING_PCAS_B, _SCHEDULE, _TRUE);
        pb[n++] = ECLIPSE_PUSH_HDR(0, NVC46F_SEM_ADDR_LO, 5);
        pb[n++] = NvU64_LO32(fenceVA);
        pb[n++] = DRF_NUM(C46F, _SEM_ADDR_HI, _OFFSET, NvU64_HI32(fenceVA));
        pb[n++] = ECLIPSE_SAXPY_FENCE_PAYLOAD;
        pb[n++] = 0;
        pb[n++] = DRF_DEF(C46F, _SEM_EXECUTE, _OPERATION, _RELEASE) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _RELEASE_WFI, _DIS) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _PAYLOAD_SIZE, _32BIT);
        pOut->pushDwords = n;

        put = pUserd->GPPut;
        gpEntry0 = DRF_DEF(906F, _GP_ENTRY0, _NO_CONTEXT_SWITCH, _FALSE) |
                   DRF_NUM(906F, _GP_ENTRY0, _GET, NvU64_LO32(pbVA) >> 2);
        gpEntry1 = DRF_NUM(906F, _GP_ENTRY1, _GET_HI, NvU64_HI32(pbVA)) |
                   DRF_NUM(906F, _GP_ENTRY1, _LENGTH, n) |
                   DRF_DEF(906F, _GP_ENTRY1, _LEVEL, _MAIN);
        gp[(put % ECLIPSE_CHAN_GPFIFO_ENTRIES) * 2 + 0] = gpEntry0;
        gp[(put % ECLIPSE_CHAN_GPFIFO_ENTRIES) * 2 + 1] = gpEntry1;
        osFlushCpuWriteCombineBuffer();

        pUserd->GPPut = put + 1;
        osFlushCpuWriteCombineBuffer();

        status = kbusFlushPcieForBar0Doorbell_HAL(pGpu, GPU_GET_KERNEL_BUS(pGpu));
        if (status == NV_OK)
            status = kfifoUpdateUsermodeDoorbell_HAL(pGpu, pKernelFifo,
                                                     pOut->workToken, pOut->runlistId);
        pOut->submitStatus = status;
        nv_printf(0, "[eclipse-rm-trace] step23: launch (%u dw, GPPut=%u) -> 0x%x qmd=0x%llx prog=0x%llx y=0x%llx a=%u\n",
                  n, put + 1, pOut->submitStatus, pOut->qmdVA, pOut->kernelVA, pOut->outVA,
                  (NvU32)ECLIPSE_SAXPY_A);
        if (status != NV_OK) goto report;
    }

    /* 6. Poll fence + RELEASE0, then verify y[i] == a*i + (100+i) = 4i+100. */
    {
        volatile NvU32 *pSem = (volatile NvU32 *)(pBufCpu + ECLIPSE_SAXPY_SEM_OFF);
        volatile NvU32 *pFence = (volatile NvU32 *)(pBufCpu + ECLIPSE_SAXPY_FENCE_OFF);
        volatile NvU32 *pY = (volatile NvU32 *)(pBufCpu + ECLIPSE_SAXPY_Y_OFF);
        NvU32 i;
        for (i = 0; i < ECLIPSE_LAUNCH_POLL_MS2; i++)
        {
            if (pOut->fenceStatus != NV_OK && *pFence == ECLIPSE_SAXPY_FENCE_PAYLOAD)
            {
                pOut->fenceStatus = NV_OK;
                pOut->fenceIters = i;
            }
            if (pOut->semStatus != NV_OK && *pSem == ECLIPSE_SAXPY_SEM_PAYLOAD)
            {
                pOut->semStatus = NV_OK;
                pOut->semIters = i;
            }
            if (pOut->fenceStatus == NV_OK && pOut->semStatus == NV_OK)
                break;
            os_delay_us(1000);
        }
        if (pOut->fenceStatus != NV_OK) { pOut->fenceStatus = NV_ERR_TIMEOUT; pOut->fenceIters = i; }
        if (pOut->semStatus != NV_OK)   { pOut->semStatus = NV_ERR_TIMEOUT;   pOut->semIters = i; }

        pOut->matchCount = 0;
        for (i = 0; i < 32; i++)
        {
            /* full-correct signature: y[i] = 3*(0x1000+i) + (100+i). */
            if (pY[i] == 0x3000u + 100u + 4u * i)
                pOut->matchCount++;
            else if (pOut->firstBadIdx == 0xFFFFFFFF)
            {
                pOut->firstBadIdx = i;
                pOut->firstBadVal = pY[i];
            }
        }
        pOut->verifyStatus = (pOut->matchCount == 32) ? NV_OK : NV_ERR_INVALID_DATA;
        nv_printf(0, "[eclipse-rm-trace] step23: fence 0x%x (@%u ms) sem 0x%x (@%u ms) correct %u/32\n",
                  pOut->fenceStatus, pOut->fenceIters, pOut->semStatus, pOut->semIters,
                  pOut->matchCount);
        nv_printf(0, "[eclipse-rm-trace] step23 REPLAY y[0..3]: 0x%x 0x%x 0x%x 0x%x (0x3064+4i=all-correct, 0=loads-read-zero, 0x1000+i=stale-R8-store, 100+i=0x64+i=store-dead)\n",
                  pY[0], pY[1], pY[2], pY[3]);
        /* Ask the RM why: MMU fault info for this channel. Fill the struct
         * (survives capture truncation -- the formatter always prints it)
         * AND nv_printf the string form. */
        {
            NV906F_CTRL_GET_MMU_FAULT_INFO_PARAMS fp;
            RM_API *pRmApi = rmapiGetInterface(RMAPI_GPU_LOCK_INTERNAL);
            NV_STATUS fs;
            portMemSet(&fp, 0, sizeof(fp));
            fs = pRmApi->Control(pRmApi, g_grAllocCache.hClient, g_grChanCache.hChannel,
                                 NV906F_CTRL_CMD_GET_MMU_FAULT_INFO, &fp, sizeof(fp));
            fp.faultString[NV906F_CTRL_MMU_FAULT_STRING_LEN - 1] = '\0';
            pOut->faultCtrlStatus = fs;
            pOut->faultAddrHi = fp.addrHi;
            pOut->faultAddrLo = fp.addrLo;
            pOut->faultType   = fp.faultType;
            nv_printf(0, "[eclipse-rm-trace] step23 MMU-FAULT ctrl=0x%x addr=0x%x%08x type=0x%x str='%s'\n",
                      fs, fp.addrHi, fp.addrLo, fp.faultType, fp.faultString);
        }
    }

report:
    if (pBufCpu != NULL)
        memmgrMemDescEndTransfer(pMemoryManager, pBufMemDesc, TRANSFER_FLAGS_NONE);
    if (pUserdCpu != NULL)
        memmgrMemDescEndTransfer(pMemoryManager, pUserdMemDesc, userdFlags);
    if (pCacheCpu != NULL)
        memmgrMemDescEndTransfer(pMemoryManager, pCacheMemDesc, TRANSFER_FLAGS_NONE);

    /* Cache the result after the FIRST attempt regardless of outcome.
     * procfs regenerates the /proc content once per read() chunk, so a
     * single `cat` used to re-enter this function 2-4 times: each rerun
     * re-seeded the markers (destroying the previous grid's stored
     * evidence) and re-launched on a possibly-wedged channel -- which is
     * also why the captured trace ("fence @2ms") and the final summary
     * ("fence TIMEOUT") could contradict each other in one output. One
     * launch per boot; reruns return this snapshot. */
    portMemCopy(&g_grSaxpyCache, sizeof(g_grSaxpyCache), pOut, sizeof(*pOut));
    g_grSaxpyDone = NV_TRUE;

    rmGpuLocksRelease(GPUS_LOCK_FLAGS_NONE, NULL);
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return NV_OK; /* per-stage statuses carry any failure */
}

/* ===================================================================
 * GIOPS BENCHMARK -- integer-ALU throughput.
 *
 * A big grid (BENCH_CTAS x BENCH_TPC threads) where each thread runs
 * BENCH_IMADS IMAD.U32 ops spread across BENCH_ACCUM INDEPENDENT
 * accumulators (Rk = Rk*R2+R3, k rotating over 8 registers). Consecutive
 * instructions touch different accumulators, so the SM issues them
 * back-to-back (delay 1) instead of stalling on each op's ~6-cycle
 * integer-ALU latency: the 8-deep rotation gives an 8-cycle RAW gap per
 * accumulator, > the 6-cycle latency (validated, and the same 8-gap that
 * silicon proved safe in the SAXPY). 256 threads/CTA = 8 warps/CTA so
 * the four SM schedulers stay fed (latency hidden by occupancy). No
 * memory traffic -> pure ALU throughput. Timed by the GPU PTIMER: two
 * host semaphore releases with RELEASE_TIMESTAMP write {payload, ts_ns}
 * -- t0 before SEND_PCAS, t1 after with RELEASE_WFI (drains the grid).
 * ops = threads * IMADS * 2 (each IMAD = a multiply + an add).
 * =================================================================== */
#define ECLIPSE_BENCH_KERNEL_OFF 0x1800
#define ECLIPSE_BENCH_QMD_OFF    0x3200
#define ECLIPSE_BENCH_T0_OFF     0x4000   /* 16 B ts report (start) */
#define ECLIPSE_BENCH_T1_OFF     0x4040   /* 16 B ts report (end)   */
#define ECLIPSE_BENCH_SEM_OFF    0x4080   /* QMD RELEASE0 (4 B)      */
#define ECLIPSE_BENCH_SEM_PAYLOAD 0x5A55B0D5
#define ECLIPSE_BENCH_T0_PAYLOAD  0x0000B000
#define ECLIPSE_BENCH_T1_PAYLOAD  0x0000B111
#define ECLIPSE_BENCH_IMADS      256      /* per thread, over 8 accumulators */
#define ECLIPSE_BENCH_ACCUM      8        /* independent accumulators R4..R11 */
#define ECLIPSE_BENCH_CTAS       2048
#define ECLIPSE_BENCH_TPC        256      /* threads per CTA = 8 warps */
#define ECLIPSE_BENCH_POLL_MS    5000

typedef struct EclipseGrBench
{
    NvU32 lookupStatus;
    NvU32 mapStatus;
    NvU32 tokenStatus;
    NvU32 submitStatus;
    NvU32 semStatus;
    NvU32 numThreads;
    NvU32 imadsPerThread;
    NvU32 pushDwords;
    NvU32 semIters;
    NvU32 reservedPad;
    NvU64 t0Ns;
    NvU64 t1Ns;
    NvU64 elapsedNs;
    NvU64 totalOps;
    NvU64 kernelVA;
    NvU64 qmdVA;
} EclipseGrBench;

static EclipseGrBench g_grBenchCache;
static NvBool g_grBenchDone = NV_FALSE;

NV_STATUS eclipse_rm_bench(NvU32 gpuInstance, EclipseGrBench *pOut)
{
    OBJGPU *pGpu;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;
    RsClient *pRsClient = NULL;
    KernelChannel *pKernelChannel = NULL;
    KernelFifo *pKernelFifo;
    MemoryManager *pMemoryManager;
    Memory *pBufMemory = NULL;
    MEMORY_DESCRIPTOR *pBufMemDesc = NULL;
    MEMORY_DESCRIPTOR *pUserdMemDesc = NULL;
    NvU8 *pBufCpu = NULL;
    NvU8 *pUserdCpu = NULL;
    NvU32 userdFlags = TRANSFER_FLAGS_USE_BAR1 |
                       TRANSFER_FLAGS_SHADOW_ALLOC |
                       TRANSFER_FLAGS_SHADOW_INIT_MEM;

    if (pOut == NULL)
        return NV_ERR_INVALID_ARGUMENT;
    if (g_grBenchDone)
    {
        portMemCopy(pOut, sizeof(*pOut), &g_grBenchCache, sizeof(g_grBenchCache));
        return NV_OK;
    }
    portMemSet(pOut, 0, sizeof(*pOut));
    pOut->lookupStatus = 0xFFFFFFFF;
    pOut->mapStatus    = 0xFFFFFFFF;
    pOut->tokenStatus  = 0xFFFFFFFF;
    pOut->submitStatus = 0xFFFFFFFF;
    pOut->semStatus    = 0xFFFFFFFF;
    pOut->numThreads     = ECLIPSE_BENCH_CTAS * ECLIPSE_BENCH_TPC;
    pOut->imadsPerThread = ECLIPSE_BENCH_IMADS;

    if (!g_grChanDone)
        return NV_ERR_INVALID_STATE; /* run step17 first */

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);
    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }
    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    status = rmGpuLocksAcquire(GPUS_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        rmapiLockRelease();
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pKernelFifo = GPU_GET_KERNEL_FIFO(pGpu);
    pMemoryManager = GPU_GET_MEMORY_MANAGER(pGpu);

    /* 1. Locate channel + buffer + USERD. */
    {
        NvU32 subdevInst;
        status = serverGetClientUnderLock(&g_resServ, g_grAllocCache.hClient, &pRsClient);
        if (status == NV_OK)
            status = CliGetKernelChannel(pRsClient, g_grChanCache.hChannel, &pKernelChannel);
        if (status == NV_OK)
            status = memGetByHandle(pRsClient, g_grChanCache.hPhysBuf, &pBufMemory);
        if (status == NV_OK)
        {
            pBufMemDesc = pBufMemory->pMemDesc;
            subdevInst = gpumgrGetSubDeviceInstanceFromGpu(pGpu);
            pUserdMemDesc = pKernelChannel->pUserdSubDeviceMemDesc[subdevInst];
            if (pBufMemDesc == NULL || pUserdMemDesc == NULL)
                status = NV_ERR_INVALID_STATE;
        }
        pOut->lookupStatus = status;
        if (status != NV_OK) goto report;
    }

    /* 2. CPU maps. */
    {
        pBufCpu = memmgrMemDescBeginTransfer(pMemoryManager, pBufMemDesc, TRANSFER_FLAGS_NONE);
        pUserdCpu = memmgrMemDescBeginTransfer(pMemoryManager, pUserdMemDesc, userdFlags);
        pOut->mapStatus = (pBufCpu != NULL && pUserdCpu != NULL) ? NV_OK : NV_ERR_GENERIC;
        if (pOut->mapStatus != NV_OK) goto report;
    }

    pOut->kernelVA = g_grChanCache.bufGpuVA + ECLIPSE_BENCH_KERNEL_OFF;
    pOut->qmdVA    = g_grChanCache.bufGpuVA + ECLIPSE_BENCH_QMD_OFF;

    /* 3. Emit the IMAD-throughput kernel + build the QMD. */
    {
        volatile NvU32 *k = (volatile NvU32 *)(pBufCpu + ECLIPSE_BENCH_KERNEL_OFF);
        NvU32 *qmd = (NvU32 *)(pBufCpu + ECLIPSE_BENCH_QMD_OFF);
        NvU64 semVA = g_grChanCache.bufGpuVA + ECLIPSE_BENCH_SEM_OFF;
        NvU32 ki = 0, j;

        /* setup: S2R R0,tid ; MOV R2,3 ; MOV R3,7 ; MOV R4..R11 = 1..8 */
        k[ki++] = 0x00007919; k[ki++] = 0x00000000; k[ki++] = 0x00002100; k[ki++] = 0x000e0200;
        k[ki++] = 0x00027802; k[ki++] = 0x00000003; k[ki++] = 0x00000f00; k[ki++] = 0x000fc400;
        k[ki++] = 0x00037802; k[ki++] = 0x00000007; k[ki++] = 0x00000f00; k[ki++] = 0x000fc400;
        for (j = 0; j < ECLIPSE_BENCH_ACCUM; j++)
        {
            NvU32 r = 4 + j;
            k[ki++] = 0x00007802 | (r << 16);   /* MOV Rr, imm */
            k[ki++] = j + 1;
            k[ki++] = 0x00000f00;
            k[ki++] = 0x000fc400;
        }
        /* IMAD.U32 Rk,Rk,R2,R3 x N, k rotating over 8 accumulators so
         * consecutive ops are independent -> delay 1 (8-deep RAW gap). */
        for (j = 0; j < ECLIPSE_BENCH_IMADS; j++)
        {
            NvU32 r = 4 + (j % ECLIPSE_BENCH_ACCUM);
            k[ki++] = 0x00007224 | (r << 16) | (r << 24);  /* IMAD Rr,Rr,.. */
            k[ki++] = 0x00000002;   /* Rb=R2 */
            k[ki++] = 0x078e0203;   /* signed rrr, Rc=R3 */
            k[ki++] = 0x000fc200;   /* delay 1, no barriers, no wait */
        }
        k[ki++] = 0x0000794d; k[ki++] = 0x00000000; k[ki++] = 0x03800000; k[ki++] = 0x000fea00; /* EXIT */
        for (j = 0; j < 4; j++) { k[ki++] = 0x00007918; k[ki++] = 0; k[ki++] = 0; k[ki++] = 0x000fc000; }

        *(volatile NvU32 *)(pBufCpu + ECLIPSE_BENCH_SEM_OFF) = 0;
        *(volatile NvU32 *)(pBufCpu + ECLIPSE_BENCH_T0_OFF)  = 0;
        *(volatile NvU32 *)(pBufCpu + ECLIPSE_BENCH_T1_OFF)  = 0;
        *(volatile NvU64 *)(pBufCpu + ECLIPSE_BENCH_T0_OFF + 8) = 0;
        *(volatile NvU64 *)(pBufCpu + ECLIPSE_BENCH_T1_OFF + 8) = 0;

        portMemSet(qmd, 0, 256);
        QMD_SET(qmd, QMDF_QMD_MAJOR_VERSION, 2);
        QMD_SET(qmd, QMDF_QMD_VERSION, 2);
        QMD_SET(qmd, QMDF_API_VISIBLE_CALL_LIMIT, 1);
        QMD_SET(qmd, QMDF_SAMPLER_INDEX, 0);
        QMD_SET(qmd, QMDF_SM_GLOBAL_CACHING_ENABLE, 1);
        QMD_SET(qmd, QMDF_CTA_RASTER_WIDTH, ECLIPSE_BENCH_CTAS);
        QMD_SET(qmd, QMDF_CTA_RASTER_HEIGHT, 1);
        QMD_SET(qmd, QMDF_CTA_RASTER_DEPTH, 1);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION0, ECLIPSE_BENCH_TPC);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION1, 1);
        QMD_SET(qmd, QMDF_CTA_THREAD_DIMENSION2, 1);
        QMD_SET(qmd, QMDF_PROGRAM_ADDRESS_LOWER, (NvU32)(pOut->kernelVA & 0xFFFFFFFFu));
        QMD_SET(qmd, QMDF_PROGRAM_ADDRESS_UPPER, (NvU32)(pOut->kernelVA >> 32));
        QMD_SET(qmd, QMDF_REGISTER_COUNT_V, ECLIPSE_LAUNCH_REG_COUNT);
        QMD_SET(qmd, QMDF_MIN_SM_CONFIG_SHMEM, 9);
        QMD_SET(qmd, QMDF_TARGET_SM_CONFIG_SHMEM, 9);
        QMD_SET(qmd, QMDF_MAX_SM_CONFIG_SHMEM, 17);
        QMD_SET(qmd, QMDF_CWD_MEMBAR_TYPE, 1);
        QMD_SET(qmd, QMDF_RELEASE_MEMBAR_TYPE, 1);
        QMD_SET(qmd, QMDF_SEMAPHORE_RELEASE_ENABLE0, 1);
        QMD_SET(qmd, QMDF_RELEASE0_ADDRESS_LOWER, (NvU32)(semVA & 0xFFFFFFFFu));
        QMD_SET(qmd, QMDF_RELEASE0_ADDRESS_UPPER, (NvU32)(semVA >> 32));
        QMD_SET(qmd, QMDF_RELEASE0_STRUCTURE_SIZE, 1);
        QMD_SET(qmd, QMDF_RELEASE0_PAYLOAD, ECLIPSE_BENCH_SEM_PAYLOAD);
        osFlushCpuWriteCombineBuffer();
        nv_printf(0, "[eclipse-rm-trace] bench: kernel %u instr (%u IMAD), grid %u CTAs x %u = %u threads\n",
                  ki / 4, (NvU32)ECLIPSE_BENCH_IMADS, (NvU32)ECLIPSE_BENCH_CTAS,
                  (NvU32)ECLIPSE_BENCH_TPC, pOut->numThreads);
    }

    /* 4. Launch: t0 timestamp -> SEND_PCAS -> t1 timestamp (WFI). */
    {
        volatile NvU32 *pb = (volatile NvU32 *)pBufCpu;
        volatile NvU32 *gp = (volatile NvU32 *)(pBufCpu + ECLIPSE_CHAN_GPFIFO_OFF);
        volatile Nvc46fControl *pUserd = (volatile Nvc46fControl *)pUserdCpu;
        NvU32 n = 0, put, gpEntry0, gpEntry1, workToken = 0, runlistId;
        NvU64 pbVA = g_grChanCache.bufGpuVA;
        NvU64 t0VA = g_grChanCache.bufGpuVA + ECLIPSE_BENCH_T0_OFF;
        NvU64 t1VA = g_grChanCache.bufGpuVA + ECLIPSE_BENCH_T1_OFF;

        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_OBJECT, 1);
        pb[n++] = TURING_COMPUTE_A;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_SHADER_SHARED_MEMORY_WINDOW_A, 2);
        pb[n++] = 0;
        pb[n++] = 0xFE000000;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SET_SHADER_LOCAL_MEMORY_WINDOW_A, 2);
        pb[n++] = 0;
        pb[n++] = 0xFF000000;
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_INVALIDATE_SKED_CACHES, 1);
        pb[n++] = 0;
        /* t0: start timestamp (no WFI) */
        pb[n++] = ECLIPSE_PUSH_HDR(0, NVC46F_SEM_ADDR_LO, 5);
        pb[n++] = NvU64_LO32(t0VA);
        pb[n++] = DRF_NUM(C46F, _SEM_ADDR_HI, _OFFSET, NvU64_HI32(t0VA));
        pb[n++] = ECLIPSE_BENCH_T0_PAYLOAD;
        pb[n++] = 0;
        pb[n++] = DRF_DEF(C46F, _SEM_EXECUTE, _OPERATION, _RELEASE) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _RELEASE_WFI, _DIS) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _PAYLOAD_SIZE, _32BIT) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _RELEASE_TIMESTAMP, _EN);
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SEND_PCAS_A, 1);
        pb[n++] = (NvU32)(pOut->qmdVA >> 8);
        pb[n++] = ECLIPSE_PUSH_HDR(1, NVC5C0_SEND_SIGNALING_PCAS_B, 1);
        pb[n++] = DRF_DEF(C5C0, _SEND_SIGNALING_PCAS_B, _INVALIDATE, _TRUE) |
                  DRF_DEF(C5C0, _SEND_SIGNALING_PCAS_B, _SCHEDULE, _TRUE);
        /* t1: end timestamp WITH WFI -- waits for the grid to drain */
        pb[n++] = ECLIPSE_PUSH_HDR(0, NVC46F_SEM_ADDR_LO, 5);
        pb[n++] = NvU64_LO32(t1VA);
        pb[n++] = DRF_NUM(C46F, _SEM_ADDR_HI, _OFFSET, NvU64_HI32(t1VA));
        pb[n++] = ECLIPSE_BENCH_T1_PAYLOAD;
        pb[n++] = 0;
        pb[n++] = DRF_DEF(C46F, _SEM_EXECUTE, _OPERATION, _RELEASE) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _RELEASE_WFI, _EN) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _PAYLOAD_SIZE, _32BIT) |
                  DRF_DEF(C46F, _SEM_EXECUTE, _RELEASE_TIMESTAMP, _EN);
        pOut->pushDwords = n;

        (void)kfifoGenerateWorkSubmitToken(pGpu, pKernelFifo, pKernelChannel, &workToken, NV_TRUE);
        pOut->tokenStatus = NV_OK;
        runlistId = kchannelGetRunlistId(pKernelChannel);

        put = pUserd->GPPut;
        gpEntry0 = DRF_DEF(906F, _GP_ENTRY0, _NO_CONTEXT_SWITCH, _FALSE) |
                   DRF_NUM(906F, _GP_ENTRY0, _GET, NvU64_LO32(pbVA) >> 2);
        gpEntry1 = DRF_NUM(906F, _GP_ENTRY1, _GET_HI, NvU64_HI32(pbVA)) |
                   DRF_NUM(906F, _GP_ENTRY1, _LENGTH, n) |
                   DRF_DEF(906F, _GP_ENTRY1, _LEVEL, _MAIN);
        gp[(put % ECLIPSE_CHAN_GPFIFO_ENTRIES) * 2 + 0] = gpEntry0;
        gp[(put % ECLIPSE_CHAN_GPFIFO_ENTRIES) * 2 + 1] = gpEntry1;
        osFlushCpuWriteCombineBuffer();
        pUserd->GPPut = put + 1;
        osFlushCpuWriteCombineBuffer();

        status = kbusFlushPcieForBar0Doorbell_HAL(pGpu, GPU_GET_KERNEL_BUS(pGpu));
        if (status == NV_OK)
            status = kfifoUpdateUsermodeDoorbell_HAL(pGpu, pKernelFifo, workToken, runlistId);
        pOut->submitStatus = status;
        nv_printf(0, "[eclipse-rm-trace] bench: launch (%u dw) -> 0x%x qmd=0x%llx prog=0x%llx\n",
                  n, pOut->submitStatus, pOut->qmdVA, pOut->kernelVA);
        if (status != NV_OK) goto report;
    }

    /* 5. Poll the t1 timestamp payload, then read both PTIMER stamps. */
    {
        volatile NvU32 *pT1p = (volatile NvU32 *)(pBufCpu + ECLIPSE_BENCH_T1_OFF);
        NvU32 i;
        for (i = 0; i < ECLIPSE_BENCH_POLL_MS; i++)
        {
            if (*pT1p == ECLIPSE_BENCH_T1_PAYLOAD) { pOut->semStatus = NV_OK; break; }
            os_delay_us(1000);
        }
        pOut->semIters = i;
        if (pOut->semStatus != NV_OK) pOut->semStatus = NV_ERR_TIMEOUT;

        pOut->t0Ns = *(volatile NvU64 *)(pBufCpu + ECLIPSE_BENCH_T0_OFF + 8);
        pOut->t1Ns = *(volatile NvU64 *)(pBufCpu + ECLIPSE_BENCH_T1_OFF + 8);
        pOut->elapsedNs = (pOut->t1Ns > pOut->t0Ns) ? (pOut->t1Ns - pOut->t0Ns) : 0;
        pOut->totalOps = (NvU64)pOut->numThreads * (NvU64)ECLIPSE_BENCH_IMADS * 2ull;
        nv_printf(0, "[eclipse-rm-trace] bench: sem 0x%x (@%u ms) dt=%llu ns ops=%llu\n",
                  pOut->semStatus, pOut->semIters, pOut->elapsedNs, pOut->totalOps);
    }

report:
    if (pBufCpu != NULL)
        memmgrMemDescEndTransfer(pMemoryManager, pBufMemDesc, TRANSFER_FLAGS_NONE);
    if (pUserdCpu != NULL)
        memmgrMemDescEndTransfer(pMemoryManager, pUserdMemDesc, userdFlags);

    portMemCopy(&g_grBenchCache, sizeof(g_grBenchCache), pOut, sizeof(*pOut));
    g_grBenchDone = NV_TRUE;

    rmGpuLocksRelease(GPUS_LOCK_FLAGS_NONE, NULL);
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return NV_OK;
}

/*
 * Step-9: the rest of the real RmInitAdapter device bring-up
 * (arch/nvalloc/unix/src/osinit.c, RmInitNvDevice), run after kgspInitRm
 * exactly like the Linux driver does:
 *
 *   gpumgrStatePreInitGpu -> gpumgrStateInitGpu ->
 *   intrSetIntrEn(DISABLED) -> gpumgrStateLoadGpu(GPU_STATE_DEFAULT)
 *
 * This runs every kernel-side engine's StatePreInit/StateInit/StateLoad
 * against the live GSP (FIFO, GMMU, bus/BAR mappings, memory manager, CE,
 * ...) -- the gateway to actually using the GPU. Interrupts are explicitly
 * left DISABLED (same call the real driver makes before StateLoad), which
 * also matches Eclipse's fully-polled operation. Each phase's status is
 * reported separately so a failure names its phase; the RM's own
 * LEVEL_ERROR prints (and our RmMsg-enabled files) land in the /proc
 * capture for the exact failing engine.
 */
#include "gpu/intr/intr.h"

typedef struct EclipseStateInitResult
{
    NvU32 preInitStatus;
    NvU32 initStatus;
    NvU32 loadStatus;
} EclipseStateInitResult;

NV_STATUS eclipse_rm_state_init(NvU32 gpuInstance, EclipseStateInitResult *pOut)
{
    OBJGPU *pGpu;
    Intr *pIntr;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;
    const NvU32 not_run = 0xFFFFFFFFu;

    if (pOut == NULL)
    {
        return NV_ERR_INVALID_ARGUMENT;
    }
    pOut->preInitStatus = not_run;
    pOut->initStatus    = not_run;
    pOut->loadStatus    = not_run;

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);

    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }

    /*
     * Hold ONLY the API lock here -- NOT the GPU locks. This was the silent
     * 0x63: the gpumgrState*Gpu wrappers each open with
     * rmGpuLocksAcquire(GPUS_LOCK_FLAGS_NONE, ...) (gpu_mgr.c) and acquire
     * the GPU locks themselves around gpuStatePreInit/Init/Load (that is who
     * satisfies gpuStatePreInit's rmGpuLockIsOwner assert). With our wrapper
     * pre-holding the group lock, that acquire found the lock already
     * running and returned NV_ERR_STATE_IN_USE (locks.c) without printing
     * anything, before a single engine ran. RmInitNvDevice (osinit.c) calls
     * these wrappers with only the API lock held -- match it exactly.
     */
    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    /*
     * Version stamp: the previous debugging round burned a reboot because the
     * on-disk kernel was stale (output byte-identical to the prior build, new
     * narration absent) and there was no way to tell from the photo. Bump
     * this string whenever step-9 diagnostics change so the output
     * self-identifies which build produced it.
     */
    ECLIPSE_TRACE("state_init: narration v5 (osMapGPU implemented -- doorbell-region NULL+0x90 fix; expect StateLoad to complete)");
    ECLIPSE_TRACE("state_init: before gpumgrStatePreInitGpu");
    pOut->preInitStatus = gpumgrStatePreInitGpu(pGpu);
    nv_printf(0, "[eclipse-rm-trace] state_init: gpumgrStatePreInitGpu -> 0x%x\n",
              pOut->preInitStatus);
    if (pOut->preInitStatus != NV_OK)
    {
        goto unlock;
    }

    ECLIPSE_TRACE("state_init: before gpumgrStateInitGpu");
    pOut->initStatus = gpumgrStateInitGpu(pGpu);
    nv_printf(0, "[eclipse-rm-trace] state_init: gpumgrStateInitGpu -> 0x%x\n",
              pOut->initStatus);
    if (pOut->initStatus != NV_OK)
    {
        goto unlock;
    }

    /* Same as RmInitNvDevice: keep RM's interrupt enable state at zero so
     * nothing gets enabled during StateLoad (Eclipse is fully polled). */
    pIntr = GPU_GET_INTR(pGpu);
    if (pIntr != NULL)
    {
        intrSetIntrEn(pIntr, INTERRUPT_TYPE_DISABLED);
    }

    ECLIPSE_TRACE("state_init: before gpumgrStateLoadGpu");
    pOut->loadStatus = gpumgrStateLoadGpu(pGpu, GPU_STATE_DEFAULT);
    nv_printf(0, "[eclipse-rm-trace] state_init: gpumgrStateLoadGpu -> 0x%x\n",
              pOut->loadStatus);

unlock:
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return NV_OK;
}

/*
 * Step-10: first real DATA MOVEMENT through the copy engine on the
 * state-loaded GPU. Uses the RM's own internal CE utility channel
 * (MemoryManager.pCeUtils -- constructed during gpuStateLoad's
 * post-scheduling-enable callbacks, mem_mgr.c memmgrInitCeUtils), i.e. the
 * exact same machinery the RM's VRAM scrubber uses, on the exact doorbell
 * path (channelFillGpFifo -> pDoorbellRegisterOffset) that step 9 debugging
 * fixed via the real osMapGPU:
 *
 *   1. Allocate two vidmem buffers A and B (memdescCreate + memdescTagAlloc,
 *      the same pattern kbusVerifyBar2 uses -- already proven on this
 *      hardware during gpuStateInit).
 *   2. CE-memset B with a poison pattern, CE-memset A with the test pattern
 *      (both synchronous: ceutilsMemset waits on the channel semaphore via
 *      channelWaitForFinishPayload -- CE completion is itself part of the
 *      proof).
 *   3. CE-memcopy A -> B.
 *   4. Map B for CPU access through BAR2 (kbusMapRmAperture_HAL, the mapping
 *      path gpuStateInit's MMU self-test already validated) and verify every
 *      dword equals the test pattern -- proving the bytes physically moved
 *      through the GPU's copy engine into VRAM.
 *
 * Every phase reports its own NV_STATUS so a partial failure is directly
 * attributable. All buffers are freed on every path.
 */
typedef struct EclipseStep10Result
{
    NvU32 ceUtilsStatus;   /* pCeUtils present? */
    NvU32 allocAStatus;
    NvU32 allocBStatus;
    NvU32 poisonStatus;    /* CE memset B = poison */
    NvU32 memsetStatus;    /* CE memset A = pattern */
    NvU32 copyStatus;      /* CE copy A -> B */
    NvU32 verifyStatus;    /* CPU readback of B */
    NvU64 bufferSize;
    NvU64 paA;
    NvU64 paB;
    NvU32 pattern;
    NvU32 poison;
    NvU32 dwordsChecked;
    NvU32 mismatchCount;
    NvU32 firstMismatchIdx;
    NvU32 firstMismatchVal;
} EclipseStep10Result;

NV_STATUS eclipse_rm_step10(NvU32 gpuInstance, EclipseStep10Result *pOut)
{
    OBJGPU *pGpu;
    MemoryManager *pMemoryManager;
    MEMORY_DESCRIPTOR *pMemDescA = NULL;
    MEMORY_DESCRIPTOR *pMemDescB = NULL;
    NvU8 *pVaA = NULL;
    NvU8 *pVaB = NULL;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;
    GPU_MASK gpusLockedMask = 0;
    const NvU64 SIZE = 256 * 1024;      /* 256 KiB: big enough to be a real
                                         * transfer, small enough that the
                                         * uncached BAR2 readback stays fast */
    const NvU32 PATTERN = 0xCAFED00Du;
    const NvU32 POISON  = 0x0BAD0BADu;
    /*
     * CE memset REAL semantics (learned on hardware, v1 of this test): the
     * pushbuffer builder programs SET_REMAP_COMPONENTS with
     * _COMPONENT_SIZE_ONE / _NUM_DST_COMPONENTS_ONE (channel_utils.c:1009),
     * so only the LOW BYTE of `pattern` is written, replicated across the
     * surface -- memset A=0xcafed00d really fills A with 0x0d bytes, and the
     * v1 run read back a perfectly uniform 0x0d0d0d0d (proof the memset,
     * copy AND readback all worked; only the test's expectation was wrong).
     */
#define ECLIPSE_BYTE_REP(x) (((NvU32)((x) & 0xFFu)) * 0x01010101u)
    /* Per-dword unique expected value for the CPU-filled copy test: catches
     * offset/aliasing errors a repeated byte cannot. 0x01000193 = FNV prime. */
#define ECLIPSE_FILL(i) (PATTERN ^ ((NvU32)(i) * 0x01000193u))
    const NvU32 not_run = 0xFFFFFFFFu;

    if (pOut == NULL)
    {
        return NV_ERR_INVALID_ARGUMENT;
    }
    portMemSet(pOut, 0, sizeof(*pOut));
    pOut->ceUtilsStatus = not_run;
    pOut->allocAStatus  = not_run;
    pOut->allocBStatus  = not_run;
    pOut->poisonStatus  = not_run;
    pOut->memsetStatus  = not_run;
    pOut->copyStatus    = not_run;
    pOut->verifyStatus  = not_run;
    pOut->bufferSize    = SIZE;
    pOut->pattern       = PATTERN;
    pOut->poison        = POISON;

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);

    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized || !pGpu->bStateLoaded)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }

    /* Same lock discipline as step 8: API lock + this GPU's group lock (CE
     * submission RPCs and vidmem allocation both assert GPU-lock ownership). */
    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    status = rmGpuGroupLockAcquire(pGpu->gpuInstance, GPU_LOCK_GRP_SUBDEVICE,
                                   GPUS_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT,
                                   &gpusLockedMask);
    if (status != NV_OK)
    {
        rmapiLockRelease();
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    ECLIPSE_TRACE("step10: CE data-movement test v2 (byte-remap-aware memset verify + CPU-unique-fill copy verify)");

    pMemoryManager = GPU_GET_MEMORY_MANAGER(pGpu);
    if (pMemoryManager == NULL || pMemoryManager->pCeUtils == NULL)
    {
        pOut->ceUtilsStatus = NV_ERR_OBJECT_NOT_FOUND;
        ECLIPSE_TRACE("step10: pCeUtils missing (gpuStateLoad incomplete?)");
        goto unlock;
    }
    pOut->ceUtilsStatus = NV_OK;

    /* --- vidmem buffers A and B, kbusVerifyBar2's own allocation pattern --- */
    pOut->allocAStatus = memdescCreate(&pMemDescA, pGpu, SIZE, 0, NV_TRUE,
                                       ADDR_FBMEM, NV_MEMORY_UNCACHED,
                                       MEMDESC_FLAGS_NONE);
    if (pOut->allocAStatus == NV_OK)
    {
        memdescTagAlloc(pOut->allocAStatus,
                        NV_FB_ALLOC_RM_INTERNAL_OWNER_UNNAMED_TAG_65, pMemDescA);
    }
    nv_printf(0, "[eclipse-rm-trace] step10: alloc A -> 0x%x\n", pOut->allocAStatus);
    if (pOut->allocAStatus != NV_OK)
    {
        goto cleanup;
    }
    pOut->paA = memdescGetPhysAddr(pMemDescA, AT_GPU, 0);

    pOut->allocBStatus = memdescCreate(&pMemDescB, pGpu, SIZE, 0, NV_TRUE,
                                       ADDR_FBMEM, NV_MEMORY_UNCACHED,
                                       MEMDESC_FLAGS_NONE);
    if (pOut->allocBStatus == NV_OK)
    {
        memdescTagAlloc(pOut->allocBStatus,
                        NV_FB_ALLOC_RM_INTERNAL_OWNER_UNNAMED_TAG_65, pMemDescB);
    }
    nv_printf(0, "[eclipse-rm-trace] step10: alloc B -> 0x%x\n", pOut->allocBStatus);
    if (pOut->allocBStatus != NV_OK)
    {
        goto cleanup;
    }
    pOut->paB = memdescGetPhysAddr(pMemDescB, AT_GPU, 0);
    nv_printf(0, "[eclipse-rm-trace] step10: A PA=0x%llx B PA=0x%llx size=0x%llx\n",
              pOut->paA, pOut->paB, SIZE);

    /* --- CPU (BAR2) mappings of both buffers, up front: the memset spot
     * checks and the CPU fill/verify all need them, and BAR2 mapping was
     * already proven by gpuStateInit's own MMU self-test --- */
    pVaA = kbusMapRmAperture_HAL(pGpu, pMemDescA);
    pVaB = kbusMapRmAperture_HAL(pGpu, pMemDescB);
    if (pVaA == NULL || pVaB == NULL)
    {
        pOut->verifyStatus = NV_ERR_INSUFFICIENT_RESOURCES;
        ECLIPSE_TRACE("step10: BAR2 map of A/B failed");
        goto cleanup;
    }

    /* --- CE memset B = poison, spot-verified (byte-remap semantics) --- */
    {
        CEUTILS_MEMSET_PARAMS params;
        portMemSet(&params, 0, sizeof(params));
        params.pMemDesc = pMemDescB;
        params.offset   = 0;
        params.length   = SIZE;
        params.pattern  = POISON;
        params.flags    = 0; /* synchronous */
        pOut->poisonStatus = ceutilsMemset(pMemoryManager->pCeUtils, &params);
        if (pOut->poisonStatus == NV_OK)
        {
            NvU32 first = MEM_RD32(pVaB);
            NvU32 last  = MEM_RD32(pVaB + SIZE - 4);
            if (first != ECLIPSE_BYTE_REP(POISON) || last != ECLIPSE_BYTE_REP(POISON))
            {
                nv_printf(0, "[eclipse-rm-trace] step10: poison spot-check first=0x%x last=0x%x expected=0x%x\n",
                          first, last, ECLIPSE_BYTE_REP(POISON));
                pOut->poisonStatus = NV_ERR_INVALID_DATA;
            }
        }
    }
    nv_printf(0, "[eclipse-rm-trace] step10: CE memset B=poison + spot-check -> 0x%x\n",
              pOut->poisonStatus);
    if (pOut->poisonStatus != NV_OK)
    {
        goto cleanup;
    }

    /* --- CE memset A = pattern low byte, spot-verified the same way --- */
    {
        CEUTILS_MEMSET_PARAMS params;
        portMemSet(&params, 0, sizeof(params));
        params.pMemDesc = pMemDescA;
        params.offset   = 0;
        params.length   = SIZE;
        params.pattern  = PATTERN;
        params.flags    = 0;
        pOut->memsetStatus = ceutilsMemset(pMemoryManager->pCeUtils, &params);
        if (pOut->memsetStatus == NV_OK)
        {
            NvU32 first = MEM_RD32(pVaA);
            NvU32 last  = MEM_RD32(pVaA + SIZE - 4);
            if (first != ECLIPSE_BYTE_REP(PATTERN) || last != ECLIPSE_BYTE_REP(PATTERN))
            {
                nv_printf(0, "[eclipse-rm-trace] step10: memset spot-check first=0x%x last=0x%x expected=0x%x\n",
                          first, last, ECLIPSE_BYTE_REP(PATTERN));
                pOut->memsetStatus = NV_ERR_INVALID_DATA;
            }
        }
    }
    nv_printf(0, "[eclipse-rm-trace] step10: CE memset A=pattern + spot-check -> 0x%x\n",
              pOut->memsetStatus);
    if (pOut->memsetStatus != NV_OK)
    {
        goto cleanup;
    }

    /* --- CPU-fill A with a per-dword unique sequence, then CE copy A -> B.
     * A repeated byte can't catch offset or aliasing bugs; a unique value in
     * every dword proves the copy engine moved THESE 256 KiB, in order. --- */
    {
        CEUTILS_MEMCOPY_PARAMS params;
        NvU32 i;
        const NvU32 n = (NvU32)(SIZE / 4);
        for (i = 0; i < n; i++)
        {
            MEM_WR32(pVaA + i * 4, ECLIPSE_FILL(i));
        }
        osFlushCpuWriteCombineBuffer();

        portMemSet(&params, 0, sizeof(params));
        params.pSrcMemDesc = pMemDescA;
        params.pDstMemDesc = pMemDescB;
        params.srcOffset   = 0;
        params.dstOffset   = 0;
        params.length      = SIZE;
        params.flags       = 0;
        pOut->copyStatus = ceutilsMemcopy(pMemoryManager->pCeUtils, &params);
    }
    nv_printf(0, "[eclipse-rm-trace] step10: CPU fill A + CE copy A->B -> 0x%x\n",
              pOut->copyStatus);
    if (pOut->copyStatus != NV_OK)
    {
        goto cleanup;
    }

    /* --- CPU readback of B through BAR2: every dword must equal the unique
     * fill value the CPU wrote into A --- */
    {
        NvU32 i;
        const NvU32 n = (NvU32)(SIZE / 4);
        pOut->dwordsChecked = n;
        for (i = 0; i < n; i++)
        {
            NvU32 v = MEM_RD32(pVaB + i * 4);
            if (v != ECLIPSE_FILL(i))
            {
                if (pOut->mismatchCount == 0)
                {
                    pOut->firstMismatchIdx = i;
                    pOut->firstMismatchVal = v;
                }
                pOut->mismatchCount++;
            }
        }
        pOut->verifyStatus = (pOut->mismatchCount == 0) ? NV_OK
                                                        : NV_ERR_INVALID_DATA;
    }
    nv_printf(0, "[eclipse-rm-trace] step10: verify -> 0x%x (%u dwords, %u mismatches)\n",
              pOut->verifyStatus, pOut->dwordsChecked, pOut->mismatchCount);

cleanup:
    if (pVaA != NULL)
    {
        kbusUnmapRmAperture_HAL(pGpu, pMemDescA, &pVaA, NV_TRUE);
    }
    if (pVaB != NULL)
    {
        kbusUnmapRmAperture_HAL(pGpu, pMemDescB, &pVaB, NV_TRUE);
    }
    if (pMemDescB != NULL)
    {
        memdescFree(pMemDescB);
        memdescDestroy(pMemDescB);
    }
    if (pMemDescA != NULL)
    {
        memdescFree(pMemDescA);
        memdescDestroy(pMemDescA);
    }

unlock:
    rmGpuGroupLockRelease(gpusLockedMask, GPUS_LOCK_FLAGS_NONE);
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return NV_OK;
}

/*
 * eclipse_rm_ce_blit -- CE-copy from a pre-existing host-sysmem physical
 * range (the compositor's dumb buffer) into a pre-existing VRAM range (the
 * UEFI GOP scanout framebuffer) via the persistent CeUtils channel.
 *
 * BAR1 <-> VRAM-offset relationship on TU10x:
 *   BAR1 is a linear aperture onto VRAM starting at offset 0 with no
 *   block remapping in the default RM configuration -- NV_PBUS_BAR1_BLOCK
 *   holds the aperture base register, but kbusPreInitBus_GH100 / Turing
 *   leave it at 0 so the window is linear from VRAM byte 0.  The UEFI GOP
 *   framebuffer is mapped by firmware at bar1_phys (or bar1_phys + small
 *   alignment offset), so its VRAM physical offset is simply:
 *     dstFbVramOffset = boot_fb_phys_cpu - bar1_phys
 *   Pass that value as dstFbVramOffset.  memdescDescribe(ADDR_FBMEM, off)
 *   targets the real scanout surface directly without any BAR mapping.
 *
 * DST: ADDR_FBMEM described at dstFbVramOffset -- the scanout surface.
 * SRC: ADDR_SYSMEM described at srcSysmemPa   -- the compositor dumb
 *      buffer.  CeUtils resolves SYSMEM via the GMMU SYSMEM aperture
 *      (PCIe peer-DMA), no explicit BAR1/BAR2 mapping required.
 *
 * The function is synchronous (flags=0) and returns only after the CE
 * completion semaphore fires.  No additional fence is needed for scanout
 * visibility: PCIe write-combine ordering guarantees the display engine
 * sees the updated bytes on its next line-fetch after the semaphore.
 *
 * Callers must ensure CPU writes to srcSysmemPa are visible before calling
 * (osFlushCpuWriteCombineBuffer if the dumb-buffer mapping is WC) -- this
 * function calls it internally as a belt-and-suspenders measure.
 */
NV_STATUS eclipse_rm_ce_blit(
    NvU32 gpuInstance,
    NvU64 dstFbVramOffset,
    NvU64 srcSysmemPa,
    NvU64 size)
{
    OBJGPU            *pGpu;
    MemoryManager     *pMemoryManager;
    MEMORY_DESCRIPTOR *pSrcMemDesc     = NULL;
    MEMORY_DESCRIPTOR *pDstMemDesc     = NULL;
    CEUTILS_MEMCOPY_PARAMS params;
    NV_STATUS          status;
    THREAD_STATE_NODE  threadState;
    GPU_MASK           gpusLockedMask  = 0;

    if (size == 0)
        return NV_ERR_INVALID_ARGUMENT;

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);

    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized || !pGpu->bStateLoaded)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }

    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    status = rmGpuGroupLockAcquire(pGpu->gpuInstance, GPU_LOCK_GRP_SUBDEVICE,
                                   GPUS_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT,
                                   &gpusLockedMask);
    if (status != NV_OK)
    {
        rmapiLockRelease();
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pMemoryManager = GPU_GET_MEMORY_MANAGER(pGpu);
    if (pMemoryManager == NULL || pMemoryManager->pCeUtils == NULL)
    {
        nv_printf(0, "[eclipse-rm-trace] ce_blit: pCeUtils missing (gpuStateLoad incomplete?)\n");
        status = NV_ERR_OBJECT_NOT_FOUND;
        goto unlock;
    }

    /* DST: the GOP scanout surface -- FBMEM at the given VRAM offset.
     * memdescDescribe fills the contiguous physical address directly so no
     * RM heap allocation touches the live scanout region. */
    status = memdescCreate(&pDstMemDesc, pGpu, size, 0, NV_TRUE,
                           ADDR_FBMEM, NV_MEMORY_UNCACHED, MEMDESC_FLAGS_NONE);
    if (status != NV_OK)
    {
        nv_printf(0, "[eclipse-rm-trace] ce_blit: DST memdescCreate -> 0x%x\n", status);
        goto cleanup;
    }
    memdescDescribe(pDstMemDesc, ADDR_FBMEM, dstFbVramOffset, size);

    /* SRC: the compositor's dumb buffer -- contiguous SYSMEM at srcSysmemPa.
     * The CE accesses SYSMEM via the GMMU SYSMEM aperture (PCIe peer-DMA);
     * no explicit BAR mapping is required on the GPU side. */
    status = memdescCreate(&pSrcMemDesc, pGpu, size, 0, NV_TRUE,
                           ADDR_SYSMEM, NV_MEMORY_CACHED, MEMDESC_FLAGS_NONE);
    if (status != NV_OK)
    {
        nv_printf(0, "[eclipse-rm-trace] ce_blit: SRC memdescCreate -> 0x%x\n", status);
        goto cleanup;
    }
    memdescDescribe(pSrcMemDesc, ADDR_SYSMEM, srcSysmemPa, size);

    /* Flush CPU WC writes so the CE sees current pixel data. */
    osFlushCpuWriteCombineBuffer();

    portMemSet(&params, 0, sizeof(params));
    params.pSrcMemDesc = pSrcMemDesc;
    params.pDstMemDesc = pDstMemDesc;
    params.srcOffset   = 0;
    params.dstOffset   = 0;
    params.length      = size;
    params.flags       = 0; /* synchronous -- waits on CE completion semaphore */
    status = ceutilsMemcopy(pMemoryManager->pCeUtils, &params);
    nv_printf(0, "[eclipse-rm-trace] ce_blit: ceutilsMemcopy(src_pa=0x%llx dst_vram=0x%llx size=0x%llx) -> 0x%x\n",
              srcSysmemPa, dstFbVramOffset, size, status);

cleanup:
    if (pDstMemDesc != NULL)
    {
        memdescFree(pDstMemDesc);
        memdescDestroy(pDstMemDesc);
    }
    if (pSrcMemDesc != NULL)
    {
        memdescFree(pSrcMemDesc);
        memdescDestroy(pSrcMemDesc);
    }

unlock:
    rmGpuGroupLockRelease(gpusLockedMask, GPUS_LOCK_FLAGS_NONE);
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return status;
}

/*
 * eclipse_rm_ce_fill_fb -- CE-memset the GOP scanout framebuffer to a solid
 * colour via the persistent CeUtils channel.
 *
 * Use as a visual self-test to confirm dstFbVramOffset is correct before
 * wiring the full sysmem->VRAM copy: a correct offset produces a solid-
 * colour screen; a wrong offset leaves the display unchanged (or corrupts
 * off-screen VRAM, harmlessly).
 *
 * Note on pattern semantics: the CE SET_REMAP_COMPONENTS pushbuffer method
 * uses _COMPONENT_SIZE_ONE / _NUM_DST_COMPONENTS_ONE -- only the LOW BYTE
 * of pattern is written, replicated across the surface.  Pass 0x00 for
 * black, 0xFF for white, 0x00 for off.  This matches step10's behaviour.
 */
NV_STATUS eclipse_rm_ce_fill_fb(
    NvU32 gpuInstance,
    NvU64 fbVramOffset,
    NvU64 size,
    NvU32 pattern)
{
    OBJGPU            *pGpu;
    MemoryManager     *pMemoryManager;
    MEMORY_DESCRIPTOR *pMemDesc       = NULL;
    CEUTILS_MEMSET_PARAMS params;
    NV_STATUS          status;
    THREAD_STATE_NODE  threadState;
    GPU_MASK           gpusLockedMask = 0;

    if (size == 0)
        return NV_ERR_INVALID_ARGUMENT;

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);

    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized || !pGpu->bStateLoaded)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }

    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    status = rmGpuGroupLockAcquire(pGpu->gpuInstance, GPU_LOCK_GRP_SUBDEVICE,
                                   GPUS_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT,
                                   &gpusLockedMask);
    if (status != NV_OK)
    {
        rmapiLockRelease();
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pMemoryManager = GPU_GET_MEMORY_MANAGER(pGpu);
    if (pMemoryManager == NULL || pMemoryManager->pCeUtils == NULL)
    {
        nv_printf(0, "[eclipse-rm-trace] ce_fill_fb: pCeUtils missing (gpuStateLoad incomplete?)\n");
        status = NV_ERR_OBJECT_NOT_FOUND;
        goto unlock;
    }

    /* FBMEM at the given VRAM offset, described directly (no RM heap alloc). */
    status = memdescCreate(&pMemDesc, pGpu, size, 0, NV_TRUE,
                           ADDR_FBMEM, NV_MEMORY_UNCACHED, MEMDESC_FLAGS_NONE);
    if (status != NV_OK)
    {
        nv_printf(0, "[eclipse-rm-trace] ce_fill_fb: memdescCreate -> 0x%x\n", status);
        goto cleanup;
    }
    memdescDescribe(pMemDesc, ADDR_FBMEM, fbVramOffset, size);

    portMemSet(&params, 0, sizeof(params));
    params.pMemDesc = pMemDesc;
    params.offset   = 0;
    params.length   = size;
    params.pattern  = pattern;
    params.flags    = 0; /* synchronous */
    status = ceutilsMemset(pMemoryManager->pCeUtils, &params);
    nv_printf(0, "[eclipse-rm-trace] ce_fill_fb: ceutilsMemset(off=0x%llx size=0x%llx pat=0x%x) -> 0x%x\n",
              fbVramOffset, size, pattern, status);

cleanup:
    if (pMemDesc != NULL)
    {
        memdescFree(pMemDesc);
        memdescDestroy(pMemDesc);
    }

unlock:
    rmGpuGroupLockRelease(gpusLockedMask, GPUS_LOCK_FLAGS_NONE);
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return status;
}

/*
 * eclipse_rm_ce_fill_fb_p2p -- like eclipse_rm_ce_fill_fb, but the destination
 * is a raw HOST physical address (`dstHostPa`) described as ADDR_SYSMEM instead
 * of this GPU's own VRAM. Run on the COMPUTE GPU (which boots reliably) with
 * dstHostPa = the CONSOLE GPU's scanout-FB BAR1 physical address: the compute
 * GPU's CE then issues PCIe writes to that address, which the fabric routes
 * peer-to-peer into the console GPU's BAR1 -> its VRAM. This sidesteps the
 * console GPU's flaky SEC2-resume bring-up entirely. If PCIe P2P is blocked by
 * the chipset (ACS), the CE still completes but the writes never land (screen
 * unchanged) -- the empirical test.
 */
NV_STATUS eclipse_rm_ce_fill_fb_p2p(
    NvU32 gpuInstance,
    NvU64 dstHostPa,
    NvU64 size,
    NvU32 pattern)
{
    OBJGPU            *pGpu;
    MemoryManager     *pMemoryManager;
    MEMORY_DESCRIPTOR *pMemDesc       = NULL;
    CEUTILS_MEMSET_PARAMS params;
    NV_STATUS          status;
    THREAD_STATE_NODE  threadState;
    GPU_MASK           gpusLockedMask = 0;

    if (size == 0)
        return NV_ERR_INVALID_ARGUMENT;

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);

    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized || !pGpu->bStateLoaded)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }

    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    status = rmGpuGroupLockAcquire(pGpu->gpuInstance, GPU_LOCK_GRP_SUBDEVICE,
                                   GPUS_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT,
                                   &gpusLockedMask);
    if (status != NV_OK)
    {
        rmapiLockRelease();
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pMemoryManager = GPU_GET_MEMORY_MANAGER(pGpu);
    if (pMemoryManager == NULL || pMemoryManager->pCeUtils == NULL)
    {
        nv_printf(0, "[eclipse-rm-trace] ce_fill_fb_p2p: pCeUtils missing (gpuStateLoad incomplete?)\n");
        status = NV_ERR_OBJECT_NOT_FOUND;
        goto unlock;
    }

    /* Destination: a raw host physical address (a peer GPU's BAR1) described as
     * ADDR_SYSMEM, UNCACHED (it is device MMIO, not cacheable RAM). */
    status = memdescCreate(&pMemDesc, pGpu, size, 0, NV_TRUE,
                           ADDR_SYSMEM, NV_MEMORY_UNCACHED, MEMDESC_FLAGS_NONE);
    if (status != NV_OK)
    {
        nv_printf(0, "[eclipse-rm-trace] ce_fill_fb_p2p: memdescCreate -> 0x%x\n", status);
        goto cleanup;
    }
    memdescDescribe(pMemDesc, ADDR_SYSMEM, dstHostPa, size);

    portMemSet(&params, 0, sizeof(params));
    params.pMemDesc = pMemDesc;
    params.offset   = 0;
    params.length   = size;
    params.pattern  = pattern;
    params.flags    = 0; /* synchronous */
    status = ceutilsMemset(pMemoryManager->pCeUtils, &params);
    nv_printf(0, "[eclipse-rm-trace] ce_fill_fb_p2p: ceutilsMemset(dst_host=0x%llx size=0x%llx pat=0x%x) -> 0x%x\n",
              dstHostPa, size, pattern, status);

cleanup:
    if (pMemDesc != NULL)
    {
        memdescFree(pMemDesc);
        memdescDestroy(pMemDesc);
    }

unlock:
    rmGpuGroupLockRelease(gpusLockedMask, GPUS_LOCK_FLAGS_NONE);
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return status;
}

/*
 * eclipse_rm_ce_blit_p2p -- like eclipse_rm_ce_blit, but the destination is a
 * raw HOST physical address (`dstHostPa`, a peer GPU's BAR1) described as
 * ADDR_SYSMEM. Run on the COMPUTE GPU to copy the compositor dumb buffer
 * (sysmem RAM) into the CONSOLE GPU's scanout FB via PCIe peer-to-peer, without
 * bringing up the console GPU. Both memdescs are ADDR_SYSMEM: src = cacheable
 * RAM, dst = uncacheable peer MMIO.
 */
NV_STATUS eclipse_rm_ce_blit_p2p(
    NvU32 gpuInstance,
    NvU64 dstHostPa,
    NvU64 srcSysmemPa,
    NvU64 size)
{
    OBJGPU            *pGpu;
    MemoryManager     *pMemoryManager;
    MEMORY_DESCRIPTOR *pSrcMemDesc    = NULL;
    MEMORY_DESCRIPTOR *pDstMemDesc    = NULL;
    CEUTILS_MEMCOPY_PARAMS params;
    NV_STATUS          status;
    THREAD_STATE_NODE  threadState;
    GPU_MASK           gpusLockedMask = 0;

    if (size == 0)
        return NV_ERR_INVALID_ARGUMENT;

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);

    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized || !pGpu->bStateLoaded)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }

    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    status = rmGpuGroupLockAcquire(pGpu->gpuInstance, GPU_LOCK_GRP_SUBDEVICE,
                                   GPUS_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT,
                                   &gpusLockedMask);
    if (status != NV_OK)
    {
        rmapiLockRelease();
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pMemoryManager = GPU_GET_MEMORY_MANAGER(pGpu);
    if (pMemoryManager == NULL || pMemoryManager->pCeUtils == NULL)
    {
        nv_printf(0, "[eclipse-rm-trace] ce_blit_p2p: pCeUtils missing (gpuStateLoad incomplete?)\n");
        status = NV_ERR_OBJECT_NOT_FOUND;
        goto unlock;
    }

    /* DST: peer GPU BAR1 host physical address as ADDR_SYSMEM, UNCACHED. */
    status = memdescCreate(&pDstMemDesc, pGpu, size, 0, NV_TRUE,
                           ADDR_SYSMEM, NV_MEMORY_UNCACHED, MEMDESC_FLAGS_NONE);
    if (status != NV_OK)
    {
        nv_printf(0, "[eclipse-rm-trace] ce_blit_p2p: DST memdescCreate -> 0x%x\n", status);
        goto cleanup;
    }
    memdescDescribe(pDstMemDesc, ADDR_SYSMEM, dstHostPa, size);

    /* SRC: compositor dumb buffer, cacheable host RAM. */
    status = memdescCreate(&pSrcMemDesc, pGpu, size, 0, NV_TRUE,
                           ADDR_SYSMEM, NV_MEMORY_CACHED, MEMDESC_FLAGS_NONE);
    if (status != NV_OK)
    {
        nv_printf(0, "[eclipse-rm-trace] ce_blit_p2p: SRC memdescCreate -> 0x%x\n", status);
        goto cleanup;
    }
    memdescDescribe(pSrcMemDesc, ADDR_SYSMEM, srcSysmemPa, size);

    osFlushCpuWriteCombineBuffer();

    portMemSet(&params, 0, sizeof(params));
    params.pSrcMemDesc = pSrcMemDesc;
    params.pDstMemDesc = pDstMemDesc;
    params.srcOffset   = 0;
    params.dstOffset   = 0;
    params.length      = size;
    params.flags       = 0; /* synchronous */
    status = ceutilsMemcopy(pMemoryManager->pCeUtils, &params);
    nv_printf(0, "[eclipse-rm-trace] ce_blit_p2p: ceutilsMemcopy(src=0x%llx dst_host=0x%llx size=0x%llx) -> 0x%x\n",
              srcSysmemPa, dstHostPa, size, status);

cleanup:
    if (pDstMemDesc != NULL)
    {
        memdescFree(pDstMemDesc);
        memdescDestroy(pDstMemDesc);
    }
    if (pSrcMemDesc != NULL)
    {
        memdescFree(pSrcMemDesc);
        memdescDestroy(pSrcMemDesc);
    }

unlock:
    rmGpuGroupLockRelease(gpusLockedMask, GPUS_LOCK_FLAGS_NONE);
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return status;
}

/*
 * Console-GPU identity, NVIDIA's own way (osinit.c RmInitNvDevice, run just
 * BEFORE kgspInitRm on Linux -- osinit.c:1831-1862):
 *
 *   RmDeterminePrimaryDevice  -> pGpu->setProperty(PDB_PROP_GPU_PRIMARY_DEVICE)
 *   RmSetConsolePreservationParams
 *       -> pKernelBus->bPreserveBar1ConsoleEnabled (console fb at BAR1 base)
 *       -> pMemoryManager->Ram.ReservedConsoleDispMemSize (aligned console size)
 *
 * Neither ever ran in Eclipse, so on the console GPU the SET_GUEST_SYSTEM_INFO
 * RPC (rpc.c:9577: `rpcInfo->bIsPrimary = pGpu->getProperty(pGpu,
 * PDB_PROP_GPU_PRIMARY_DEVICE)`) told GSP-RM it was a headless secondary --
 * while the UEFI GOP scanout was live in its FB and its VGA decode active.
 * The secondary GPU (for which bIsPrimary=false is TRUE) boots perfectly;
 * the console GPU wedges during the SEC2 GSP-RM resume even with the graphic
 * console fully frozen (KD_GRAPHICS, zero CPU pixel writes) -- so the
 * remaining delta vs. Linux is exactly this mis-declared identity plus the
 * missing console-region reservation. Called by bringup_step11 (nvidia.rs)
 * before eclipse_rm_init_gsp, i.e. at the same point in the sequence where
 * Linux runs the real thing.
 */
NV_STATUS eclipse_rm_mark_console_gpu(
    NvU32 gpuInstance,
    NvU64 consoleSize,
    NvU8  bConsoleAtBar1Base)
{
    OBJGPU *pGpu;
    KernelBus *pKernelBus;
    MemoryManager *pMemoryManager;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);

    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return NV_ERR_INVALID_ARGUMENT;
    }

    pGpu->setProperty(pGpu, PDB_PROP_GPU_PRIMARY_DEVICE, NV_TRUE);

    /*
     * BAR1-based console (EFI GOP maps the console fb at the start of BAR1,
     * exactly our case): RM keeps a BAR1 mapping alive for it. osinit.c:1004:
     * bPreserveBar1ConsoleEnabled = (fbBaseAddress == nv->fb->cpu_address).
     */
    pKernelBus = GPU_GET_KERNEL_BUS(pGpu);
    if (pKernelBus != NULL && bConsoleAtBar1Base)
    {
        pKernelBus->bPreserveBar1ConsoleEnabled = NV_TRUE;
    }

    /* osinit.c:1017: reserve the console's display memory so nothing (GSP
     * scrubber, heap) allocates over the live scanout surface. */
    pMemoryManager = GPU_GET_MEMORY_MANAGER(pGpu);
    if (pMemoryManager != NULL && consoleSize > 0)
    {
        pMemoryManager->Ram.ReservedConsoleDispMemSize =
            NV_ALIGN_UP(consoleSize, 0x10000);
    }

    nv_printf(0, "[eclipse-rm-trace] mark_console_gpu: PRIMARY_DEVICE set, "
              "preserveBar1Console=%u reservedConsoleDispMem=0x%llx\n",
              (NvU32)(bConsoleAtBar1Base ? 1 : 0),
              (pMemoryManager != NULL) ? pMemoryManager->Ram.ReservedConsoleDispMemSize : 0);

    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
    return NV_OK;
}

/* ===================================================================
 * DRM: real EDID / connector query via the RM's NV04_DISPLAY_COMMON.
 *
 * Allocates NV04_DISPLAY_COMMON (0x0073) under the step-16 device, then
 * asks the GSP-resident display RM: which outputs exist (GET_SUPPORTED
 * displayMask + which support DDC), which are connected (GET_CONNECT_
 * STATE), and reads the EDID of the first connected one (GET_EDID_V2).
 * Everything is ROUTED through the live RM -- no register bit-banging.
 * On the headless compute GPU this reports "0 connected" (its own real
 * answer); on a GPU with a monitor it returns the panel's EDID head
 * (00 FF..FF 00 magic + PNP manufacturer id + product code), which then
 * feeds DRM get_connector. Read-only: never programs the display engine.
 * =================================================================== */
typedef struct EclipseGrEdid
{
    NvU32 allocStatus;      /* NV04_DISPLAY_COMMON alloc                */
    NvU32 supportedStatus;
    NvU32 displayMask;      /* outputs that physically exist            */
    NvU32 displayMaskDDC;   /* subset that supports DDC/EDID            */
    NvU32 connectStatus;
    NvU32 connectedMask;    /* outputs with something plugged in        */
    NvU32 edidStatus;
    NvU32 edidDisplayId;    /* the display whose EDID we read           */
    NvU32 edidSize;
    NvU32 edidValid;        /* 1 = 00 FF FF FF FF FF FF 00 header seen   */
    NvU8  edidHead[32];     /* header + PNP id + product + serial + date */
    /* Physical connector type per output (GET_CONNECTOR_DATA 0x730250). */
#define ECLIPSE_EDID_MAX_OUTS 16
    NvU32 connTypeStatus;   /* first error, else NV_OK; 0xFFFFFFFF = not reached */
    NvU32 connTypeCount;
    NvU32 connTypeDisplayId[ECLIPSE_EDID_MAX_OUTS];
    NvU32 connType[ECLIPSE_EDID_MAX_OUTS]; /* NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_* */
} EclipseGrEdid;

/*
 * Per-GPU cache: both TU106s register a DRM driver and /proc/gpuedid
 * queries each with its own gpuInstance -- a single global here would
 * replay GPU A's connectors/EDID as GPU B's answer.
 */
static NvBool        g_edidDone[NV_MAX_DEVICES];
static EclipseGrEdid g_edidCache[NV_MAX_DEVICES];
/*
 * One-shot latch for the on-demand kdispAllocateCommonHandle: its failure
 * path leaks the freshly-allocated client/device/subdevice trio
 * (kern_disp.c NV_ASSERT_OR_RETURN quirk), so a persistently failing alloc
 * must run once per boot, not once per proc chunk read.
 */
static NvBool        g_edidAllocTried[NV_MAX_DEVICES];

NV_STATUS eclipse_rm_edid(NvU32 gpuInstance, EclipseGrEdid *pOut)
{
    OBJGPU *pGpu;
    RM_API *pRmApi;
    NV_STATUS status;
    THREAD_STATE_NODE threadState;
    GPU_MASK gpusLockedMask = 0;
    KernelDisplay *pKernelDisplay;
    NvHandle hDispClient;
    NvHandle hDispCommon;

    if (pOut == NULL || gpuInstance >= NV_MAX_DEVICES)
        return NV_ERR_INVALID_ARGUMENT;

    /*
     * Idempotent: busybox `cat` reads /proc in 4KB chunks, re-invoking the
     * content generator (and hence this query) several times. Do the real
     * RM work exactly once per boot per GPU and replay the cached result.
     */
    if (g_edidDone[gpuInstance])
    {
        *pOut = g_edidCache[gpuInstance];
        return NV_OK;
    }

    portMemSet(pOut, 0, sizeof(*pOut));
    pOut->allocStatus     = 0xFFFFFFFF;
    pOut->supportedStatus = 0xFFFFFFFF;
    pOut->connectStatus   = 0xFFFFFFFF;
    pOut->edidStatus      = 0xFFFFFFFF;
    pOut->connTypeStatus  = 0xFFFFFFFF;

    if (!g_grAllocDone)
        return NV_ERR_INVALID_STATE; /* run step16 first */

    threadStateInit(&threadState, THREAD_STATE_FLAGS_NONE);
    status = gpumgrThreadEnableExpandedGpuVisibility();
    if (status != NV_OK)
    {
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    pGpu = gpumgrGetGpu(gpuInstance);
    if (pGpu == NULL || !pGpu->gspRmInitialized)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return (pGpu == NULL) ? NV_ERR_INVALID_ARGUMENT : NV_ERR_INVALID_STATE;
    }
    status = rmapiLockAcquire(API_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT);
    if (status != NV_OK)
    {
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }
    /*
     * ALL-GPUs lock (not just this subdevice's): the on-demand
     * kdispAllocateCommonHandle below constructs an RM client, and
     * clientConstruct (client.c:163, r18) asserts unless the thread either
     * owns ALL GPU locks or none -- a partial group lock trips the
     * deadlock-prevention 0x1A. kdispStatePreInitLocked, the RM's own
     * caller, runs under the gpumgr wrappers' all-GPUs lock; replicate
     * that exact context.
     */
    status = rmGpuGroupLockAcquire(pGpu->gpuInstance, GPU_LOCK_GRP_ALL,
                                   GPUS_LOCK_FLAGS_NONE, RM_LOCK_MODULES_INIT,
                                   &gpusLockedMask);
    if (status != NV_OK)
    {
        rmapiLockRelease();
        gpumgrThreadDisableExpandedGpuVisibility();
        threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);
        return status;
    }

    pRmApi = rmapiGetInterface(RMAPI_GPU_LOCK_INTERNAL);

    /*
     * NV04_DISPLAY_COMMON is effectively one-per-GPU: the RM's KernelDisplay
     * allocates its own DispCommon in kdispStatePreInitLocked
     * (kdispAllocateCommonHandle, kern_disp.c), and a second alloc came back
     * NV_ERR_STATE_IN_USE (0x63) on hardware -- the collision is enforced on
     * the GSP/physical side of the RPCed alloc (the CPU-side alloc_free.c
     * single-instance check is only per-parent-Device). Instead, borrow the
     * RM's internal handle pair -- exactly what the RM itself uses to issue
     * NV0073 controls (see kdispGetSupportedDisplayMask /
     * kdispIsDisplayConnected).
     */
    pKernelDisplay = GPU_GET_KERNEL_DISPLAY(pGpu);
    if (pKernelDisplay == NULL)
    {
        /* No display engine on this GPU (headless secondary). */
        pOut->allocStatus = NV_ERR_NOT_SUPPORTED;
        nv_printf(0, "[eclipse-rm-trace] edid: no KernelDisplay on this GPU (headless)\n");
        status = NV_OK;
        goto unlock;
    }
    hDispClient = kdispGetInternalClientHandle(pKernelDisplay);
    hDispCommon = kdispGetDispCommonHandle(pKernelDisplay);
    if (hDispClient == 0 || hDispCommon == 0)
    {
        /*
         * The RM normally creates the internal DispCommon pair in
         * kdispStatePreInitLocked, but on the console GPU the CPU-side
         * state chain hangs after the EXP1c PDISP reset (run r17) -- and
         * the display controls don't actually need it: they are
         * ROUTE_TO_PHYSICAL RPCs served by the GSP, whose own display
         * state has been live since kgspInitRm. Create the handle pair on
         * demand with the RM's own allocator (an object alloc that RPCs to
         * the GSP; our held locks match kdispStatePreInitLocked's calling
         * context).
         */
        if (!g_edidAllocTried[gpuInstance])
        {
            NV_STATUS allocSt;
            g_edidAllocTried[gpuInstance] = NV_TRUE;
            allocSt = kdispAllocateCommonHandle(pGpu, pKernelDisplay);
            hDispClient = kdispGetInternalClientHandle(pKernelDisplay);
            hDispCommon = kdispGetDispCommonHandle(pKernelDisplay);
            /*
             * NOTE: the RM returns NV_FALSE (== 0 == NV_OK) on its failure
             * paths here, so ret is meaningless -- the re-read handles are
             * the only truth.
             */
            nv_printf(0, "[eclipse-rm-trace] edid: kdispAllocateCommonHandle on demand -> ret=0x%x client=0x%x common=0x%x\n",
                      allocSt, hDispClient, hDispCommon);
        }
    }
    if (hDispClient == 0 || hDispCommon == 0)
    {
        pOut->allocStatus = NV_ERR_INVALID_STATE;
        nv_printf(0, "[eclipse-rm-trace] edid: RM DispCommon not initialized (client=0x%x common=0x%x)\n",
                  hDispClient, hDispCommon);
        status = NV_OK;
        goto unlock;
    }
    pOut->allocStatus = NV_OK; /* have a usable DispCommon handle */
    nv_printf(0, "[eclipse-rm-trace] edid: reuse RM DispCommon (client=0x%x common=0x%x)\n",
              hDispClient, hDispCommon);

    {
        NV0073_CTRL_SYSTEM_GET_SUPPORTED_PARAMS sp;
        portMemSet(&sp, 0, sizeof(sp));
        pOut->supportedStatus = pRmApi->Control(pRmApi, hDispClient, hDispCommon,
                                                NV0073_CTRL_CMD_SYSTEM_GET_SUPPORTED,
                                                &sp, sizeof(sp));
        pOut->displayMask    = sp.displayMask;
        pOut->displayMaskDDC = sp.displayMaskDDC;
        nv_printf(0, "[eclipse-rm-trace] edid: GET_SUPPORTED -> 0x%x mask=0x%x ddc=0x%x\n",
                  pOut->supportedStatus, sp.displayMask, sp.displayMaskDDC);
    }
    {
        NV0073_CTRL_SYSTEM_GET_CONNECT_STATE_PARAMS cs;
        portMemSet(&cs, 0, sizeof(cs));
        cs.displayMask = pOut->displayMask;
        cs.flags = NV0073_CTRL_SYSTEM_GET_CONNECT_STATE_FLAGS_METHOD_CACHED;
        pOut->connectStatus = pRmApi->Control(pRmApi, hDispClient, hDispCommon,
                                              NV0073_CTRL_CMD_SYSTEM_GET_CONNECT_STATE,
                                              &cs, sizeof(cs));
        pOut->connectedMask = cs.displayMask;
        nv_printf(0, "[eclipse-rm-trace] edid: GET_CONNECT_STATE -> 0x%x connected=0x%x\n",
                  pOut->connectStatus, cs.displayMask);
        /*
         * _METHOD_CACHED returns the GSP's LAST detection pass. On the
         * console GPU nobody (no nvkms, no CPU-side display client) has
         * ever run one, so an empty cache reads back as "0 connected" even
         * with the monitor physically plugged in -- and that false negative
         * would then be cached for the boot. When the cached answer is
         * empty but DDC-capable outputs exist, run a REAL detection
         * (_METHOD_DEFAULT), honoring the documented
         * NVOS_STATUS_ERROR_RETRY/retryTimeMs handshake, bounded.
         */
        if (pOut->connectStatus == NV_OK && pOut->connectedMask == 0 &&
            pOut->displayMaskDDC != 0)
        {
            NvU32 attempt;
            for (attempt = 0; attempt < 4; attempt++)
            {
                portMemSet(&cs, 0, sizeof(cs));
                cs.displayMask = pOut->displayMask;
                cs.flags = NV0073_CTRL_SYSTEM_GET_CONNECT_STATE_FLAGS_METHOD_DEFAULT;
                pOut->connectStatus = pRmApi->Control(pRmApi, hDispClient, hDispCommon,
                                                      NV0073_CTRL_CMD_SYSTEM_GET_CONNECT_STATE,
                                                      &cs, sizeof(cs));
                nv_printf(0, "[eclipse-rm-trace] edid: GET_CONNECT_STATE(real detect, try %u) -> 0x%x connected=0x%x retryMs=%u\n",
                          attempt, pOut->connectStatus, cs.displayMask, cs.retryTimeMs);
                if (pOut->connectStatus != NV_ERR_TIMEOUT_RETRY)
                {
                    pOut->connectedMask = cs.displayMask;
                    break;
                }
                osDelay((cs.retryTimeMs != 0 && cs.retryTimeMs < 500) ? cs.retryTimeMs : 100);
            }
        }
    }
    if (pOut->connectStatus == NV_OK && pOut->connectedMask != 0)
    {
        NvU32 did = pOut->connectedMask & (0U - pOut->connectedMask); /* lowest set bit */
        NV0073_CTRL_SPECIFIC_GET_EDID_V2_PARAMS *ep =
            portMemAllocNonPaged(sizeof(*ep));
        if (ep != NULL)
        {
            portMemSet(ep, 0, sizeof(*ep));
            ep->displayId  = did;
            ep->bufferSize = NV0073_CTRL_SPECIFIC_GET_EDID_MAX_EDID_BYTES;
            pOut->edidStatus = pRmApi->Control(pRmApi, hDispClient, hDispCommon,
                                               NV0073_CTRL_CMD_SPECIFIC_GET_EDID_V2,
                                               ep, sizeof(*ep));
            pOut->edidDisplayId = did;
            pOut->edidSize      = ep->bufferSize;
            if (pOut->edidStatus == NV_OK && ep->bufferSize >= 32)
            {
                NvU32 b;
                for (b = 0; b < 32; b++)
                    pOut->edidHead[b] = ep->edidBuffer[b];
                /* Full EDID 1.x header: 00 FF FF FF FF FF FF 00 */
                pOut->edidValid = (ep->edidBuffer[0] == 0x00 &&
                                   ep->edidBuffer[7] == 0x00) ? 1 : 0;
                for (b = 1; b <= 6; b++)
                    if (ep->edidBuffer[b] != 0xFF)
                        pOut->edidValid = 0;
            }
            nv_printf(0, "[eclipse-rm-trace] edid: GET_EDID id=0x%x -> 0x%x size=%u valid=%u mfg=%02x%02x prod=%02x%02x\n",
                      did, pOut->edidStatus, pOut->edidSize, pOut->edidValid,
                      pOut->edidHead[8], pOut->edidHead[9],
                      pOut->edidHead[10], pOut->edidHead[11]);
            portMemFree(ep);
        }
        else
        {
            pOut->edidStatus = NV_ERR_NO_MEMORY;
        }
    }
    /*
     * Physical connector type of every output (DP/HDMI/DVI/...), one
     * GET_CONNECTOR_DATA per displayId bit -- the control only accepts a
     * single display per call. Read-only, same proven handle pair.
     */
    if (pOut->supportedStatus == NV_OK && pOut->displayMask != 0)
    {
        NvU32 rem = pOut->displayMask;
        while (rem != 0 && pOut->connTypeCount < ECLIPSE_EDID_MAX_OUTS)
        {
            NvU32 did = rem & (0U - rem); /* lowest set bit */
            NV_STATUS ctStatus;
            NV0073_CTRL_SPECIFIC_GET_CONNECTOR_DATA_PARAMS cd;
            portMemSet(&cd, 0, sizeof(cd));
            cd.displayId = did;
            ctStatus = pRmApi->Control(pRmApi, hDispClient, hDispCommon,
                                       NV0073_CTRL_CMD_SPECIFIC_GET_CONNECTOR_DATA,
                                       &cd, sizeof(cd));
            /* Keep the first error so a partial failure is visible. */
            if (pOut->connTypeStatus == 0xFFFFFFFF || pOut->connTypeStatus == NV_OK)
                pOut->connTypeStatus = ctStatus;
            pOut->connTypeDisplayId[pOut->connTypeCount] = did;
            pOut->connType[pOut->connTypeCount] =
                (ctStatus == NV_OK && cd.count > 0) ? cd.data[0].type : 0xFFFFFFFF;
            nv_printf(0, "[eclipse-rm-trace] edid: CONNECTOR_DATA id=0x%x -> 0x%x count=%u type=0x%x\n",
                      did, ctStatus, cd.count,
                      pOut->connType[pOut->connTypeCount]);
            pOut->connTypeCount++;
            rem &= rem - 1;
        }
    }
    status = NV_OK;

unlock:
    rmGpuGroupLockRelease(gpusLockedMask, GPUS_LOCK_FLAGS_NONE);
    rmapiLockRelease();
    gpumgrThreadDisableExpandedGpuVisibility();
    threadStateFree(&threadState, THREAD_STATE_FLAGS_NONE);

    /*
     * Cache the outcome so subsequent proc reads replay it verbatim -- but
     * ONLY when the query actually ran (we had a usable DispCommon handle).
     * The transient early-outs (no KernelDisplay yet, handles not populated
     * because gpuStatePreInit hasn't run) must NOT be cached: the r16 run
     * proved the handles appear later in the same boot (step14 stage 4),
     * and a cached miss would shadow them for the rest of the boot.
     */
    if (status == NV_OK && pOut->allocStatus == NV_OK &&
        pOut->supportedStatus == NV_OK && pOut->connectStatus == NV_OK &&
        pOut->edidStatus != NV_ERR_TIMEOUT &&
        pOut->edidStatus != NV_ERR_TIMEOUT_RETRY)
    {
        g_edidCache[gpuInstance] = *pOut;
        g_edidDone[gpuInstance]  = NV_TRUE;
    }
    return status;
}
