//! DRM (Direct Rendering Manager) Scheme for drivers
//!
//! This trait allows drivers to implement DRM/KMS functionality.

use super::Scheme;
use alloc::vec::Vec;

/// DRM Device capabilities
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DrmCaps {
    pub has_3d: bool,
    pub has_cursor: bool,
    pub max_width: u32,
    pub max_height: u32,
}

/// GEM (Graphics Execution Manager) handle
#[derive(Debug, Clone, Copy)]
pub struct GemHandle {
    pub id: u32,
    pub size: usize,
    pub phys_addr: u64,
}

/// DRM Connector (output)
#[derive(Debug, Clone, Copy)]
pub struct DrmConnector {
    pub id: u32,
    pub connected: bool,
    pub mm_width: u32,
    pub mm_height: u32,
    /// DRM_MODE_CONNECTOR_* (0 = Unknown). Synthetic/fallback connectors
    /// keep the historical hardcoded 11.
    pub connector_type: u32,
}

/// DRM CRTC (display controller)
#[derive(Debug, Clone, Copy)]
pub struct DrmCrtc {
    pub id: u32,
    pub fb_id: u32,
    pub x: u32,
    pub y: u32,
}

/// DRM Plane (Overlay, Primary, or Cursor)
#[derive(Debug, Clone, Copy)]
pub struct DrmPlane {
    pub id: u32,
    pub crtc_id: u32,
    pub fb_id: u32,
    pub possible_crtcs: u32,
    pub plane_type: u32, // 1=Primary, 2=Cursor, 0=Overlay
}

/// Abstract trait for DRM Driver implementations
pub trait DrmScheme: Scheme {
    fn get_caps(&self) -> DrmCaps;

    /// Whether this driver can own legacy-KMS scanout/presentation for dumb
    /// framebuffers instead of falling back to software KMS blits.
    fn has_hardware_kms(&self) -> bool {
        false
    }

    /// Retrieve the raw 128-byte EDID for a connector, if available.
    fn get_connector_edid(&self, _id: u32) -> Option<[u8; 128]> {
        None
    }

    /// Read-only register/state dump for GPU bring-up debugging, surfaced at
    /// `/proc/gpudbg`. Default: nothing. Hardware drivers override it to read
    /// (never write) device registers post-boot — early BAR0 access can hang
    /// some GPUs, so this is only ever invoked on demand from userspace. With
    /// multiple GPUs every DRM device is dumped, each labelled by its own name.
    fn debug_dump(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Hand the driver a firmware blob read from the mounted rootfs (e.g.
    /// GSP-RM's `gsp.bin`). Drivers can't read files themselves --
    /// `drivers` sits below `linux-object`/`kernel-hal` in the dependency
    /// graph, so the blob must be pushed down from boot code that already
    /// has filesystem access, once the rootfs is mounted. Default: ignore
    /// (most drivers need no firmware).
    fn set_gsp_firmware(&self, _bytes: alloc::vec::Vec<u8>) {}

    /// Record a human-readable outcome of the boot-time firmware-load attempt
    /// (see `set_gsp_firmware`). Pushed down from boot code even when the load
    /// FAILED, so a driver whose `set_gsp_firmware` was never called can still
    /// report *why* (file not found, short read, etc.) in its bring-up output
    /// instead of a bare "no firmware". Default: ignore.
    fn set_gsp_firmware_status(&self, _status: alloc::string::String) {}

    /// GPU copy-engine bring-up **Step 2**, surfaced (opt-in) at
    /// `/proc/gpustep2`: write the channel instance block into sysmem and issue
    /// the GMMU flush — the first real GPU register writes of the bring-up.
    /// Default: nothing. The hardware driver targets a specific GPU and
    /// returns a human-readable report. Unlike `debug_dump` this is NOT
    /// read-only, hence its own node: `/proc/gpudbg` stays safe to poll.
    ///
    /// TEMPORARY (see nvidia.rs `bringup_step2`): currently targets the
    /// CONSOLE GPU (only one GPU available; the other has unrelated
    /// problems) instead of the original non-console safety net — a wedge
    /// here can blank the only display and require a hard reboot.
    fn bringup_step2(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// GPU copy-engine bring-up **Step 3** (`/proc/gpustep3`): enable the
    /// doorbell, commit the channel runlist, and enable the channel in the
    /// scheduler — with an empty GPFIFO so nothing actually executes. Same
    /// targeting as `bringup_step2` (see its TEMPORARY note). Default: nothing.
    fn bringup_step3(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// GPU copy-engine bring-up **Step 4** (`/proc/gpustep4`): ring the doorbell
    /// with a pushbuffer that binds the copy class (`SET_OBJECT`), exercising the
    /// full doorbell → PBDMA → GMMU-translated pushbuffer fetch → method-parse
    /// path. Same targeting as `bringup_step2` (see its TEMPORARY note). Default: nothing.
    fn bringup_step4(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 5** (`/proc/gpustep5`): the real vendored RM core's
    /// own attach path (`nvidia_rm_sys::rm_init::attach_gpu`) -- real HAL
    /// bind/attach work, not a safe register read. Originally wired into
    /// `debug_dump`, moved out after it hung real hardware on a plain
    /// `cat /proc/gpudbg`. Default: nothing.
    fn bringup_step5(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 6** (`/proc/gpustep6`): the real vendored `kgspInitRm`
    /// GSP-RM boot (VBIOS/FWSEC extraction, Booter secure boot, WPR2 setup).
    /// The deepest and riskiest step yet -- requires `bringup_step5` to have
    /// succeeded first. Default: nothing.
    fn bringup_step6(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 7** (`/proc/gpustep7`): read back the
    /// `GspStaticConfigInfo` the live GSP-RM firmware returned during
    /// `bringup_step6` (GPU name, VRAM geometry, VBIOS IDs) -- proves the
    /// CPU<->GSP RPC channel end-to-end beyond boot. Default: nothing.
    fn bringup_step7(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 8** (`/proc/gpustep8`): read-only RM API controls
    /// (GPU name, UUID, FB heap total/free) executed by the live GSP-RM's
    /// own resource server over the GSP_RM_CONTROL RPC. Default: nothing.
    fn bringup_step8(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 9** (`/proc/gpustep9`): the rest of the real
    /// RmInitAdapter sequence -- gpuStatePreInit/StateInit/StateLoad for all
    /// kernel-side engines against the live GSP. Destructive/one-shot per
    /// boot (result cached). Default: nothing.
    fn bringup_step9(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 10** (`/proc/gpustep10`): first real copy-engine data
    /// movement on the state-loaded GPU -- CE memset + copy between two VRAM
    /// buffers, verified by CPU readback through BAR2. Cached per boot.
    /// Default: nothing.
    fn bringup_step10(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 11** (`/proc/gpustep11`): GSP-RM boot on the CONSOLE
    /// GPU with pre-boot register dumps, full-chain VGA-routing disable,
    /// root-port completion-timeout containment, and post-STARTCPU bus
    /// autopsy. Cached per boot. Default: nothing.
    fn bringup_step11(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 12** (`/proc/gpustep12`): EXP 1 -- console-GPU GSP
    /// boot with the display engine held in reset (scanout DMA stopped) to
    /// test the live-iso-fetch-vs-SEC2 theory. Blanks the screen; run blind
    /// and capture to a file. Default: nothing.
    fn bringup_step12(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 13** (`/proc/gpustep13`): EXP 2 -- console-GPU GSP boot
    /// with a pre-STARTCPU interrupt-drain "pseudo-ISR service loop". Snapshots
    /// + write-1-to-clears the CPU-facing top-level interrupt tree right before
    /// the SEC2 STARTCPU posted store, testing whether an unserviced
    /// fabric/display interrupt (Eclipse is 100% polled, no RM ISR) is what
    /// stalls the write. Does NOT touch PDISP. Cached per boot. Default: nothing.
    fn bringup_step13(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 14** (`/proc/gpustep14`): the CONSOLE GPU's full
    /// bring-up chained in one shot -- attach, GSP-RM boot (with the permanent
    /// console SEC2 drain), RM controls, gpuStatePreInit/Init/Load, and the
    /// copy-engine data-movement test -- bringing the primary to the same
    /// state-loaded, CE-verified state as the secondary. Default: nothing.
    fn bringup_step14(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// CE-offloaded present: copy the compositor's dumb buffer (contiguous
    /// sysmem at `src_sysmem_pa`, `size` bytes) into this GPU's scanout
    /// framebuffer via the persistent CeUtils channel, replacing the CPU
    /// `memcpy`-over-PCIe. Returns true if the CE copy was performed (console
    /// GPU, state-loaded); false to fall back to the CPU blit. Default: false.
    fn ce_present(&self, _src_sysmem_pa: u64, _size: u64) -> bool {
        false
    }

    /// Automatic boot-time compute-GPU bring-up. Runs the proven state-load
    /// chain (attach -> GSP-RM boot -> RM API client -> gpuStateLoad, i.e. the
    /// `/proc/gpustep5;6;8;9` sequence) on every GPU that does NOT drive the
    /// boot display, so the copy-engine present path (`ce_present` over PCIe
    /// P2P) is ready before the compositor starts — no manual `cat` needed.
    /// The GPU that drives the console is SKIPPED: its GSP boot wedges at the
    /// SEC2 STARTCPU store (~1/3 of the time, hanging the machine), so it is
    /// never auto-booted. Called once, synchronously, from the kernel main
    /// after the GSP firmware is loaded and before any userspace runs, so
    /// there is no concurrent RM access. Systems with a single (console) GPU
    /// bring up nothing here and fall back to the CPU blit. General over 1, 2,
    /// 3+ GPUs. Returns a SINGLE clean status line for a compute GPU (empty for
    /// the console GPU and non-NVIDIA drivers) -- the verbose per-step
    /// narration is discarded (still captured into `/proc/gpustep*`), and the
    /// caller suppresses the driver's own log output around this call, so the
    /// desktop console stays clean. Default: nothing.
    fn auto_bringup_compute(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Leave this GPU in a clean (cold) state before a system reboot. A GPU we
    /// brought up carries a live GSP-RM: WPR2 is locked in its VRAM and its
    /// SEC2/GSP falcons are running. Eclipse's `reboot` is a WARM reset -- it
    /// restarts the CPU but does NOT power-cycle the GPU -- so without this the
    /// GPU crosses the reboot still dirty, and the firmware POST that runs
    /// BEFORE the OS on the next boot chokes on it (long POST / hang / recovery).
    /// Implementations issue a PCIe Function Level Reset (or similar) to return
    /// the GPU close to cold state. Only GPUs actually brought up act; the
    /// console GPU and non-NVIDIA drivers are no-ops. Called from the reset
    /// path just before the CPU reset. Returns a short log line. Default: none.
    fn quiesce_for_reboot(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// CE-offload visual test (`/proc/gpucefill`): CE-memset the console GPU's
    /// scanout framebuffer to a solid colour via the persistent CeUtils channel,
    /// to confirm the BAR1->VRAM offset is correct before wiring the full
    /// per-frame CE present blit. Requires the console GPU to be state-loaded
    /// (`/proc/gpustep14`). Default: nothing.
    fn bringup_ce_fill_fb(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// P2P CE-offload visual test (`/proc/gpucefillp2p`): from the COMPUTE GPU,
    /// CE-memset the CONSOLE GPU's scanout framebuffer white over PCIe
    /// peer-to-peer, to confirm P2P works before relying on it for the present
    /// path — the via-A route that avoids the flaky console-GPU bring-up.
    fn bringup_ce_fill_fb_p2p(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Read back (and clear) the CMOS survival breadcrumb the previous
    /// console-GPU boot attempt left, surfaced at `/proc/gpusurvive`. On a box
    /// with no serial, this is how a SEC2-window wedge is diagnosed: the CPU
    /// hangs and nothing else survives, but the CMOS NVRAM keeps the last
    /// milestone + RM narration count across the reboot. Default: nothing.
    fn survival_report(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 15** (`/proc/gpustep15`): probe the GR (graphics/compute)
    /// engine's GPC/TPC/SM shader config on a state-loaded GPU via the live
    /// GSP-RM (GR_GET_GPC_MASK / GR_GET_TPC_MASK controls). Read-only,
    /// repeatable; groundwork toward a real compute launch. Default: nothing.
    fn bringup_step15(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 16** (`/proc/gpustep16`): GR allocation ladder on a
    /// state-loaded GPU (client/device/subdevice/VAS/TSG(GR)/ctxshare) via
    /// the vendored resource server -- front half of a compute launch.
    /// Idempotent. Default: nothing.
    fn bringup_step16(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 17** (`/proc/gpustep17`): compute channel on the
    /// step-16 ladder (USERD + GPFIFO memory + channel-in-TSG +
    /// TURING_COMPUTE_A + schedule). Idempotent. Default: nothing.
    fn bringup_step17(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// `/proc/gpuedid`: real display query via NV04_DISPLAY_COMMON — which
    /// outputs exist, which are connected, and the EDID of the first
    /// connected one. Read-only (never programs the display engine).
    /// Requires step16. Default: nothing.
    fn bringup_edid(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 18** (`/proc/gpustep18`): first Eclipse-authored
    /// submission on the step-17 channel — semaphore-release method stream,
    /// GP entry, GPPut, usermode doorbell, CPU-polled verification.
    /// Idempotent once fully successful. Default: nothing.
    fn bringup_step18(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 19** (`/proc/gpustep19`): first real compute launch —
    /// a Turing QMD pointing at a minimal SM75 kernel, submitted via
    /// SEND_PCAS on the step-17/18 channel, verified by the QMD RELEASE0
    /// semaphore. Idempotent once the semaphore lands. Default: nothing.
    fn bringup_step19(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 20** (`/proc/gpustep20`): kernel that stores a chosen
    /// value to a chosen VA (patched immediates + STG.E.SYS + EXIT), triple
    /// verified (fence, RELEASE0, CPU readback of the stored dword).
    /// Idempotent once release+store verify. Default: nothing.
    fn bringup_step20(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 21** (`/proc/gpustep21`): 32-thread kernel
    /// (out[tid] = tid*3+7) with per-thread CPU verification of all 32
    /// slots. Idempotent once release+verify pass. Default: nothing.
    fn bringup_step21(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 22** (`/proc/gpustep22`): chip-scale grid — 68 CTAs
    /// x 32 threads across all 34 SMs, out[gid] = gid*3+7, all 2176 slots
    /// CPU-verified. Idempotent once release+verify pass. Default: nothing.
    fn bringup_step22(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Bring-up **Step 23** (`/proc/gpustep23`): integer SAXPY
    /// (y[i] = a*x[i] + y[i]) with real global loads (LDG) -- the
    /// load-compute-store canon, per-element verified. Default: nothing.
    fn bringup_step23(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// GIOPS benchmark (`/proc/gpubench`): a big grid of dependent-IMAD
    /// chains timed with the GPU PTIMER. Default: nothing.
    fn bringup_bench(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Read-only hardware-state dump (`/proc/gpudump`): the discriminating
    /// registers (display head liveness, VGA workspace, PMC, BSI, sysmem
    /// flush) for this GPU, with NO GSP boot -- zero wedge risk. Lets us
    /// diff primary vs secondary and pre-decide the display experiments
    /// without spending a boot. Default: nothing.
    fn hw_dump(&self) -> alloc::string::String {
        alloc::string::String::new()
    }

    /// Import a buffer allocated by the kernel (DRM core)
    fn import_buffer(&self, _handle: GemHandle) -> bool {
        true
    }

    /// Free a buffer
    fn free_buffer(&self, _handle: GemHandle) {}

    /// Create a framebuffer from a GEM handle
    fn create_fb(&self, handle_id: u32, width: u32, height: u32, pitch: u32) -> Option<u32>;

    /// Page flip: atomically switch to a new framebuffer
    fn page_flip(&self, fb_id: u32) -> bool;

    /// Set hardware cursor position and/or image
    fn set_cursor(&self, crtc_id: u32, x: i32, y: i32, handle: u32, flags: u32) -> bool;

    /// Wait for vertical blank on a CRTC
    fn wait_vblank(&self, crtc_id: u32) -> bool;

    /// Get driver resources (fb_ids, crtc_ids, connector_ids)
    fn get_resources(&self) -> (Vec<u32>, Vec<u32>, Vec<u32>);

    /// Get connector info
    fn get_connector(&self, id: u32) -> Option<DrmConnector>;

    /// Get crtc info
    fn get_crtc(&self, id: u32) -> Option<DrmCrtc>;

    /// Get plane info
    fn get_plane(&self, id: u32) -> Option<DrmPlane>;

    /// Get all planes supported by the driver
    fn get_planes(&self) -> Vec<u32>;

    /// Set plane properties
    fn set_plane(
        &self,
        plane_id: u32,
        crtc_id: u32,
        fb_id: u32,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        src_x: u32,
        src_y: u32,
        src_w: u32,
        src_h: u32,
    ) -> bool;

    /// Driver-specific IOCTLs
    fn ioctl(&self, _request: u32, _arg: usize) -> Result<usize, i32> {
        Err(38) // ENOSYS
    }
}
