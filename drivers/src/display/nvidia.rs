use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use crate::bus::pci_drivers::PciDriver;
use crate::prelude::{AccelCaps, ColorFormat, DisplayInfo, FrameBuffer};
use crate::scheme::drm::{DrmCaps, DrmConnector, DrmCrtc, DrmPlane, GemHandle};
use crate::scheme::{DisplayScheme, DrmScheme, Scheme};
use crate::utils::dma::DmaRegion;
use crate::{builder::IoMapper, Device, DeviceError, DeviceResult};
use alloc::sync::Arc;
use lock::Mutex;
use pci::{PCIDevice, BAR};

// --- Registers and Constants (aligned with Nova / open-gpu-kernel-modules) ---
#[allow(dead_code)]
mod regs {
    pub const NV_PMC_BOOT_0: u32 = 0x0000_0000;
    pub const PMC_BOOT0_CHIP_ID_SHIFT: u32 = 20;
    pub const PMC_BOOT0_CHIP_ID_MASK: u32 = 0xFFF;

    pub const PMC_BOOT0_CHIPID_TURING_MIN: u32 = 0x160;
    pub const PMC_BOOT0_CHIPID_TURING_MAX: u32 = 0x16F;
    pub const PMC_BOOT0_CHIPID_AMPERE_MIN: u32 = 0x170;
    pub const PMC_BOOT0_CHIPID_AMPERE_MAX: u32 = 0x17F;
    pub const PMC_BOOT0_CHIPID_ADA_MIN: u32 = 0x190;
    pub const PMC_BOOT0_CHIPID_ADA_MAX: u32 = 0x19F;
    pub const PMC_BOOT0_CHIPID_HOPPER_MIN: u32 = 0x1B0;
    pub const PMC_BOOT0_CHIPID_HOPPER_MAX: u32 = 0x1BF;
    pub const PMC_BOOT0_CHIPID_BLACKWELL_MIN: u32 = 0x200;

    pub const NV_PFB_CSTATUS: u32 = 0x0010_020C;
    pub const NV_PFB_CSTATUS_MEM_SIZE_MASK: u32 = 0x7FFF;

    pub const NV_THERM_TEMP: u32 = 0x0002_0400;
    pub const NV_THERM_TEMP_VALUE_MASK: u32 = 0x1FF;
    pub const NV_THERM_TEMP_VALUE_SIGN_BIT: u32 = 0x100;

    // Display resolution registers (legacy/fallback)
    pub const NV50_HEAD0_RASTER_SIZE: u32 = 0x610798;
    pub const NV40_PCRTC_HEAD0_SIZE: u32 = 0x60002C;
}

/// TU106 (Turing) GMMU encode helpers — NV_MMU_VER2 page-table format.
///
/// Verified against nouveau `vmmgp100.c` / open-gpu `gp100/dev_mmu.h` (Turing
/// reuses the gp100 VER2 VMM verbatim). These build page tables in *RAM only*;
/// the GPU never sees them until the instance block is written and the GMMU is
/// flushed (a later, riskier step). Critical fact: the leaf PTE address field
/// is `phys >> 4` (the 53:8 field stores `phys>>12`, and `(phys>>12)<<8 ==
/// phys>>4`); writing `phys>>12` directly hangs the GPU.
mod gmmu {
    /// SYSTEM_COHERENT aperture (HOST). VRAM=0, HOST=2, NCOH=3.
    pub const AP_HOST: u64 = 2;
    /// PITCH (uncompressed) kind.
    pub const KIND_PITCH: u64 = 0x00;

    /// Leaf PTE for a 4 KiB sysmem page, read-write, uncompressed.
    /// VALID(0) | APERTURE 2:1 = HOST | VOL(3) | ADDRESS=phys>>4 | KIND 63:56.
    #[inline]
    pub fn encode_pte_sys(phys: u64) -> u64 {
        (phys >> 4) | (1 << 0) | (AP_HOST << 1) | (1 << 3) | (KIND_PITCH << 56)
    }

    /// Single PDE (PD1/PD2/PD3 levels) pointing at the next table in sysmem.
    /// APERTURE 2:1 = HOST (aperture != 0 ⇒ present; there is no VALID bit) |
    /// VOL(3) | ADDRESS_SYS 53:8 = next>>4. The dual-PDE SMALL half is encoded
    /// identically and stored in the high qword at byte `pdei*0x10 + 8`.
    #[inline]
    pub fn encode_pde_sys(next_table_phys: u64) -> u64 {
        (next_table_phys >> 4) | (AP_HOST << 1) | (1 << 3)
    }

    /// Instance-block PD-base qword (@0x200): root PD phys OR'd with
    /// VER2(1<<10) | 64KiB(1<<11) | HOST_target(2<<0) | VOL(1<<2) == `|0xC06`.
    #[inline]
    pub fn inst_pd_base(root_phys: u64) -> u64 {
        root_phys | 0xC06
    }
}

/// Coherent-sysmem structures for the Turing copy-engine bring-up (the verified
/// memory plan). All allocated via `DmaRegion::alloc_coherent` (page-aligned,
/// zeroed, UC). Built and dumped read-only at `/proc/gpudbg` for hand
/// verification BEFORE any GPU state is changed. The four buffers the *engine*
/// dereferences by VA (src/dst/sem/pushbuffer) are packed into a single 2 MiB
/// GMMU region so one SPT leaf and one PD0 entry cover everything.
#[allow(dead_code)] // inst/userd/gpfifo are wired up in later bring-up steps
struct GpuBringup {
    // 5-level page-directory chain (sysmem-coherent, one 4 KiB page each).
    root: DmaRegion, // desc_12[4], PGD 2-bit, the PDB given to the GPU
    pd3: DmaRegion,  // desc_12[3], PGD 9-bit
    pd2: DmaRegion,  // desc_12[2], PGD 9-bit
    pd0: DmaRegion,  // desc_12[1], dual-PDE 8-bit
    spt: DmaRegion,  // desc_12[0], SPT leaf, 512×8 B PTEs
    // Sysmem structures the engine reaches through the GMMU (by GPU VA), so
    // they stay in coherent sysmem and are mapped into the channel page tables.
    gpfifo: DmaRegion,
    pushbuf: DmaRegion,
    sem: DmaRegion,
    src: DmaRegion,
    dst: DmaRegion,
    /// Copy-engine fault-method buffer (sysmem). Only dereferenced by the CE
    /// engine on a faulting method; a red herring for channel load, kept mapped
    /// at va_base+0x5000 but its instance-block pointer is left disarmed.
    ce_fault: DmaRegion,
    /// HUB MMU non-replayable fault buffer (sysmem). On Volta+ the host requires
    /// a fault buffer armed (NV_VIRTUAL_FUNCTION_PRIV_MMU_FAULT_BUFFER, 0xb83000)
    /// before any channel can run — nouveau arms it in the `fault` subdev before
    /// the FIFO. We arm it in PHYSICAL/SYS_COH mode so no BAR2 mapping is needed.
    fault_buf: DmaRegion,
    /// Base GPU virtual address of the packed 2 MiB region.
    va_base: u64,
    /// Base VRAM offset (0-based into VRAM) for the structures the host reads by
    /// raw physical address — instance block, runlist, USERD. Turing's host
    /// scheduler walks these as VRAM-physical (the 0x002b00 runlist path has no
    /// target field), so they cannot live in sysmem. They are CPU-written via
    /// the PRAMIN window. Layout: inst=+0, runlist=+0x1000, userd=+0x2000.
    vram_base: u64,
}

impl GpuBringup {
    #[inline]
    fn inst_vram(&self) -> u64 {
        self.vram_base
    }
    #[inline]
    fn runlist_vram(&self) -> u64 {
        self.vram_base + 0x1000
    }
    #[inline]
    fn userd_vram(&self) -> u64 {
        self.vram_base + 0x2000
    }
    /// BAR2 instance block VRAM offset (shares the channel's page tables).
    #[inline]
    fn bar2_inst_vram(&self) -> u64 {
        self.vram_base + 0x3000
    }
    #[inline]
    fn gpfifo_va(&self) -> u64 {
        self.va_base + 0x4000
    }
    /// GPU/BAR2 VA of the CE fault-method buffer. Used once we arm the real CE
    /// engine context (after HOST/GP_GET is brought up).
    #[allow(dead_code)]
    #[inline]
    fn ce_fault_va(&self) -> u64 {
        self.va_base + 0x5000
    }
}

impl GpuBringup {
    /// Allocate the memory plan and build the GMMU page tables in RAM. No GPU
    /// register is touched here — only sysmem is written, so this is safe to run
    /// on demand. Returns `None` if the coherent DMA allocator is exhausted.
    fn build(va_base: u64, vram_base: u64) -> Option<Self> {
        let root = DmaRegion::alloc_coherent(0x1000)?;
        let pd3 = DmaRegion::alloc_coherent(0x1000)?;
        let pd2 = DmaRegion::alloc_coherent(0x1000)?;
        let pd0 = DmaRegion::alloc_coherent(0x1000)?;
        let spt = DmaRegion::alloc_coherent(0x1000)?;
        let gpfifo = DmaRegion::alloc_coherent(0x1000)?;
        let pushbuf = DmaRegion::alloc_coherent(0x1000)?;
        let sem = DmaRegion::alloc_coherent(0x1000)?;
        let src = DmaRegion::alloc_coherent(0x1000)?;
        let dst = DmaRegion::alloc_coherent(0x1000)?;
        // CE fault-method buffer: 8 pages (32 KiB) covers the nouveau size
        // formula for any realistic PCE count.
        let ce_fault = DmaRegion::alloc_coherent(0x8000)?;
        // HUB MMU fault buffer: 256 KiB (8192 × 32 B entries) — generous.
        let fault_buf = DmaRegion::alloc_coherent(0x4_0000)?;

        // Pack the engine-visible buffers into one 2 MiB region:
        //  src=+0x0 dst=+0x1000 sem=+0x2000 pushbuffer=+0x3000 gpfifo=+0x4000
        //  ce_fault=+0x5000 (8 pages). The GPFIFO ring and CE fault buffer are
        // referenced by GPU/BAR2 VA, so they are GMMU-mapped like the pushbuffer.
        let src_va = va_base;
        let dst_va = va_base + 0x1000;
        let sem_va = va_base + 0x2000;
        let pb_va = va_base + 0x3000;
        let gpfifo_va = va_base + 0x4000;
        let ce_fault_va = va_base + 0x5000;

        // Leaf PTEs (SPT). idx = (va>>12)&0x1ff.
        let wr64 = |r: &DmaRegion, i: usize, v: u64| unsafe {
            core::ptr::write_volatile(r.as_ptr::<u64>().add(i), v)
        };
        wr64(
            &spt,
            ((src_va >> 12) & 0x1ff) as usize,
            gmmu::encode_pte_sys(src.paddr() as u64),
        );
        wr64(
            &spt,
            ((dst_va >> 12) & 0x1ff) as usize,
            gmmu::encode_pte_sys(dst.paddr() as u64),
        );
        wr64(
            &spt,
            ((sem_va >> 12) & 0x1ff) as usize,
            gmmu::encode_pte_sys(sem.paddr() as u64),
        );
        wr64(
            &spt,
            ((pb_va >> 12) & 0x1ff) as usize,
            gmmu::encode_pte_sys(pushbuf.paddr() as u64),
        );
        wr64(
            &spt,
            ((gpfifo_va >> 12) & 0x1ff) as usize,
            gmmu::encode_pte_sys(gpfifo.paddr() as u64),
        );
        // CE fault buffer: 8 contiguous pages.
        for p in 0..8u64 {
            let va = ce_fault_va + p * 0x1000;
            wr64(
                &spt,
                ((va >> 12) & 0x1ff) as usize,
                gmmu::encode_pte_sys(ce_fault.paddr() as u64 + p * 0x1000),
            );
        }

        // PD0 dual-PDE: pdei = (va>>21)&0xff (== 1 for all, same 2 MiB slot).
        // Low qword = BIG (unused, 0); high qword = SMALL = single-PDE form.
        let pdei = ((src_va >> 21) & 0xff) as usize;
        wr64(&pd0, pdei * 2, 0);
        wr64(&pd0, pdei * 2 + 1, gmmu::encode_pde_sys(spt.paddr() as u64));

        // PD2 / PD3 / root: single PDEs; idx == 0 at all three top levels here.
        wr64(
            &pd2,
            ((src_va >> 29) & 0x1ff) as usize,
            gmmu::encode_pde_sys(pd0.paddr() as u64),
        );
        wr64(
            &pd3,
            ((src_va >> 38) & 0x1ff) as usize,
            gmmu::encode_pde_sys(pd2.paddr() as u64),
        );
        wr64(
            &root,
            ((src_va >> 47) & 0x3) as usize,
            gmmu::encode_pde_sys(pd3.paddr() as u64),
        );

        Some(Self {
            root,
            pd3,
            pd2,
            pd0,
            spt,
            gpfifo,
            pushbuf,
            sem,
            src,
            dst,
            ce_fault,
            fault_buf,
            va_base,
            vram_base,
        })
    }

    /// Read-only dump of the allocated physical layout and every encoded
    /// page-table entry, for hand-verification against the spec before the GPU
    /// is ever pointed at these tables.
    fn dump(&self) -> String {
        use core::fmt::Write;
        let rd64 =
            |r: &DmaRegion, i: usize| unsafe { core::ptr::read_volatile(r.as_ptr::<u64>().add(i)) };
        let mut s = String::new();
        let _ = writeln!(
            s,
            "[gpudbg]  --- GMMU tables (Step 1, built in RAM; GPU not yet pointed at them) ---"
        );
        let _ = writeln!(
            s,
            "[gpudbg]  PD  phys: root={:#x} pd3={:#x} pd2={:#x} pd0={:#x} spt={:#x}",
            self.root.paddr(),
            self.pd3.paddr(),
            self.pd2.paddr(),
            self.pd0.paddr(),
            self.spt.paddr()
        );
        let _ = writeln!(
            s,
            "[gpudbg]  sysmem phys: gpfifo={:#x} pb={:#x} sem={:#x} src={:#x} dst={:#x}",
            self.gpfifo.paddr(),
            self.pushbuf.paddr(),
            self.sem.paddr(),
            self.src.paddr(),
            self.dst.paddr()
        );
        let _ = writeln!(
            s,
            "[gpudbg]  VRAM off: inst={:#x} runlist={:#x} userd={:#x} (host-read via PRAMIN)",
            self.inst_vram(),
            self.runlist_vram(),
            self.userd_vram()
        );
        let va = self.va_base;
        let ri = ((va >> 47) & 0x3) as usize;
        let d3 = ((va >> 38) & 0x1ff) as usize;
        let d2 = ((va >> 29) & 0x1ff) as usize;
        let pdei = ((va >> 21) & 0xff) as usize;
        let _ = writeln!(
            s,
            "[gpudbg]  VA base={:#x} idx[root={} pd3={} pd2={} pd0={}]",
            va, ri, d3, d2, pdei
        );
        let _ = writeln!(s, "[gpudbg]  root[{}] = {:#018x}", ri, rd64(&self.root, ri));
        let _ = writeln!(s, "[gpudbg]  pd3 [{}] = {:#018x}", d3, rd64(&self.pd3, d3));
        let _ = writeln!(s, "[gpudbg]  pd2 [{}] = {:#018x}", d2, rd64(&self.pd2, d2));
        let _ = writeln!(
            s,
            "[gpudbg]  pd0 [{}] big={:#018x} small={:#018x}",
            pdei,
            rd64(&self.pd0, pdei * 2),
            rd64(&self.pd0, pdei * 2 + 1)
        );
        for (name, off) in [
            ("src", 0u64),
            ("dst", 0x1000),
            ("sem", 0x2000),
            ("pb", 0x3000),
            ("gpfifo", 0x4000),
        ] {
            let v = va + off;
            let si = ((v >> 12) & 0x1ff) as usize;
            let _ = writeln!(
                s,
                "[gpudbg]  spt [{:3}] {} va={:#x} pte={:#018x}",
                si,
                name,
                v,
                rd64(&self.spt, si)
            );
        }
        let _ = writeln!(
            s,
            "[gpudbg]  inst PD-base qword(@0x200) = {:#018x} (root|0xC06, points at sysmem PDs)",
            gmmu::inst_pd_base(self.root.paddr() as u64)
        );
        s
    }

    /// Step 4: write a minimal method stream into the pushbuffer — just
    /// `SET_OBJECT(TURING_DMA_COPY_A=0xC5B5)` on subchannel 4. Returns the dword
    /// count. Header `(mthd>>2)|(subc<<13)|(count<<16)|(INC=1<<29)`; for
    /// mthd 0x0, subc 4, count 1 that is 0x20018000. No GPU register touched.
    fn write_setobject_pushbuffer(&self) -> u32 {
        let pb = self.pushbuf.vaddr();
        let w32 =
            |i: usize, v: u32| unsafe { core::ptr::write_volatile((pb as *mut u32).add(i), v) };
        w32(0, 0x2001_8000); // INC subc4 mthd 0x000 (SET_OBJECT) count1
        w32(1, 0x0000_c5b5); // TURING_DMA_COPY_A class
        2
    }

    /// Write a GPFIFO launch entry into ring `slot` pointing at pushbuffer GPU
    /// VA `pb_va` of `n` dwords. entry0 = GET (pb[31:2]); entry1 = GET_HI |
    /// LENGTH<<10. Verified against clc36f.h NVC36F_GP_ENTRY*.
    fn write_gpfifo_entry(&self, slot: usize, pb_va: u64, n: u32) {
        let gp = self.gpfifo.vaddr();
        let w32 =
            |i: usize, v: u32| unsafe { core::ptr::write_volatile((gp as *mut u32).add(i), v) };
        let entry0 = (pb_va as u32) & 0xFFFF_FFFC;
        let entry1 = ((pb_va >> 32) as u32 & 0xFF) | (n << 10);
        w32(slot * 2, entry0);
        w32(slot * 2 + 1, entry1);
    }
}

static BOOT_FB_INFO: Mutex<Option<BootFbInfo>> = Mutex::new(None);

/// Runs `nvidia_rm_sys::rm_init::init_core()` (constructs the real OBJSYS
/// singleton + RM resource server) at most once, regardless of how many
/// GPUs attach or how many times a caller asks. Safe to call from every
/// `NvidiaGpu::debug_dump()`; only the first call actually invokes RM.
static RM_CORE_INIT_STATUS: Mutex<Option<u32>> = Mutex::new(None);

/// Set before invoking RM init, never cleared. If it's already set while
/// `RM_CORE_INIT_STATUS` is still `None`, a previous attempt started and
/// DIED mid-initialization (bring-up faults kill the reading task, not
/// the machine) -- RM's global C state (nvport/TLS init counts, g_pSys,
/// half-constructed OBJSYS children, rm locks) is debris at that point,
/// and re-running real NVIDIA init over it fails nondeterministically at
/// unrelated-looking places. Cost us a full diagnostic cycle: a re-run on
/// a dirty boot "regressed" three trace lines earlier than the previous
/// run and looked like a new bug. Refuse instead; only a reboot resets it.
static RM_CORE_INIT_ATTEMPTED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Distinctive sentinel (not a real NV_STATUS) reported when RM init is
/// refused because a prior in-boot attempt died partway through.
const RM_INIT_POISONED: u32 = 0xDEAD_1417;

fn rm_core_init_once() -> u32 {
    use core::sync::atomic::Ordering;
    let mut status = RM_CORE_INIT_STATUS.lock();
    if let Some(s) = *status {
        return s;
    }
    if RM_CORE_INIT_ATTEMPTED.swap(true, Ordering::SeqCst) {
        log::error!(
            "[NVIDIA] rm_core_init_once: a previous RM init attempt this boot died \
             mid-initialization; refusing to re-enter over its half-initialized \
             global state. Reboot to retry (status={:#x}).",
            RM_INIT_POISONED
        );
        return RM_INIT_POISONED;
    }
    let s = nvidia_rm_sys::rm_init::init_core();
    *status = Some(s);
    s
}

/// One-shot guard so the first CE-offloaded present logs once (a console photo
/// then confirms the desktop is being composited by the copy engine).
static CE_PRESENT_LOGGED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy)]
struct BootFbInfo {
    phys: u64,
    width: u32,
    height: u32,
    pitch: u32,
}

pub fn set_boot_fb_info(phys: u64, width: u32, height: u32, pitch: u32) {
    *BOOT_FB_INFO.lock() = Some(BootFbInfo {
        phys,
        width,
        height,
        pitch,
    });
}

/// Raw EDID of the active display captured by the UEFI bootloader
/// (`EFI_EDID_ACTIVE_PROTOCOL`), stashed at driver init. This is the real
/// panel on the GPU that drives the GOP console -- available with no GPU
/// display bring-up at all. `len` is the valid byte count (0 = none).
static BOOT_EDID: Mutex<Option<([u8; 128], u32)>> = Mutex::new(None);

/// Record the boot-time EDID (called from kernel-hal with the bootloader's
/// `GraphicInfo.edid`). A zero length is stored as "no EDID".
pub fn set_boot_edid(edid: &[u8], len: u32) {
    if len == 0 || edid.is_empty() {
        return;
    }
    let mut buf = [0u8; 128];
    let n = (len as usize).min(edid.len()).min(128);
    buf[..n].copy_from_slice(&edid[..n]);
    *BOOT_EDID.lock() = Some((buf, n as u32));
}

/// The captured UEFI EDID (bytes, valid length), if the firmware exposed one.
pub fn boot_edid() -> Option<([u8; 128], u32)> {
    *BOOT_EDID.lock()
}

/// Physical address of the boot (UEFI GOP) framebuffer, if known. The GPU whose
/// BAR1 aperture contains this address is the one driving the console.
fn boot_fb_phys() -> Option<u64> {
    BOOT_FB_INFO.lock().map(|b| b.phys)
}

/// Byte size of the boot (UEFI GOP) framebuffer (`pitch * height`), if known.
/// Used by the P2P CE path/tests, which run on the compute GPU and therefore
/// cannot read the console FB geometry from their own `self.info`.
fn boot_fb_size() -> Option<u64> {
    BOOT_FB_INFO
        .lock()
        .map(|b| b.pitch as u64 * b.height as u64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvidiaArchitecture {
    Unknown,
    Turing,      // RTX 20 series
    Ampere,      // RTX 30 series
    AdaLovelace, // RTX 40 series
    Hopper,      // H100/H200
    Blackwell,   // RTX 50 series
}

#[derive(Clone, Copy)]
struct ImportedGemHandle {
    id: u32,
    phys_addr: u64,
    size: usize,
}

#[derive(Clone, Copy)]
struct NvidiaKmsFramebuffer {
    id: u32,
    handle_id: u32,
    width: u32,
    height: u32,
    pitch: u32,
    phys_addr: u64,
    size: usize,
}

#[derive(Clone, Copy)]
struct NvidiaKmsState {
    crtc_fb: u32,
    plane_fb: u32,
    last_vblank_us: u64,
}

pub struct NvidiaGpu {
    name: String,
    info: DisplayInfo,
    architecture: NvidiaArchitecture,
    gpu_model: &'static str,
    /// Raw PCI device id, kept for `NOUVEAU_GETPARAM_PCI_DEVICE` (see
    /// `nouveau_uapi.rs`) -- `identify_gpu` already consumes this once at
    /// construction but didn't need to retain it before now.
    device_id: u16,
    vram_size_mb: u32,
    pitch_override: Option<u32>,
    _bar0: usize,
    _bar1: usize,
    /// Physical base of BAR1 (the VRAM aperture). Used to decide whether this GPU
    /// backs the boot framebuffer (i.e. drives the console) and must therefore be
    /// spared from the risky copy-engine bring-up writes.
    bar1_phys: u64,
    /// Physical base and mapped length of BAR0 (the MMIO register aperture),
    /// and this GPU's real PCI location -- needed to attach it to the real
    /// vendored RM core via nvidia_rm_sys::rm_init (GPUATTACHARG wants the
    /// same info NVIDIA's own osInitNvMapping packages from nv_state_t).
    bar0_phys: u64,
    bar0_len: u64,
    /// Physical base and size of BAR2 (NVIDIA logical index `IMEM`, the small
    /// ~32 MiB "instance memory" aperture -- PCI BAR3 on Turing). RM needs this
    /// as `GPUATTACHARG.instPhysAddr`/`instLength`: without it, `kbusVerifyBar2`
    /// (kern_bus_gm107.c) has no BAR2 physical aperture to program, its MMU
    /// self-test write never lands in VRAM, and `gpumgrStateInitGpu` fails with
    /// NV_ERR_MEMORY_ERROR (0x72). Matches osinit.c:708
    /// (`nv->bars[NV_GPU_BAR_INDEX_IMEM]`).
    bar2_phys: u64,
    bar2_len: u64,
    pci_domain: u32,
    pci_bus: u8,
    pci_device: u8,
    vram_allocator: Mutex<Option<NvidiaVramAllocator>>,
    /// Copy-engine bring-up state (GMMU tables + channel structs). Built lazily
    /// on the first `/proc/gpudbg` read so the memory plan is only allocated
    /// when someone is actually debugging GPU bring-up.
    bringup: Mutex<Option<GpuBringup>>,
    /// Result of the real RM attach attempt (nvidia_rm_sys::rm_init), cached
    /// after the first `/proc/gpudbg` read triggers it so repeated reads
    /// don't re-run RM's own object-construction logic.
    rm_attach_result: Mutex<Option<String>>,
    /// Real RM device instance from a successful attach, needed to look the
    /// `OBJGPU*` back up (`gpumgrGetGpu`) for the GSP init step below.
    rm_device_instance: Mutex<Option<u32>>,
    /// Real GSP-RM firmware (`gsp.bin`), pushed down by `zCore`'s boot code
    /// via `set_gsp_firmware` once the rootfs is mounted -- this driver runs
    /// during early PCI enumeration, well before any filesystem exists, so
    /// it cannot read the file itself (see DrmScheme::set_gsp_firmware).
    gsp_firmware: Mutex<Option<Vec<u8>>>,
    /// Human-readable outcome of the boot-time firmware load (set even when it
    /// failed), so `bringup_step6` can explain a missing blob. See
    /// `DrmScheme::set_gsp_firmware_status`.
    gsp_fw_status: Mutex<Option<String>>,
    /// Result of the real kgspInitRm attempt, cached the same way as
    /// `rm_attach_result`.
    gsp_init_result: Mutex<Option<String>>,
    /// Cached step-9 result (gpuState PreInit/Init/Load). One-shot per boot:
    /// the RM state machine is not re-runnable, so the first outcome is
    /// what /proc/gpustep9 keeps reporting.
    state_init_result: Mutex<Option<String>>,
    /// Cached step-10 result (CE memset/copy + readback verify). Cached like
    /// the others so repeated `cat`s don't re-run CE work; a reboot re-arms.
    step10_result: Mutex<Option<String>>,
    /// Imported GEM handles from the DRM core; indexed by core handle id.
    imported_handles: Mutex<Vec<ImportedGemHandle>>,
    /// Nouveau-uAPI state (see `nouveau_uapi.rs`), opt-in via
    /// `nvidia.nouveau_uapi`. `None` until `CHANNEL_ALLOC` succeeds; this
    /// milestone supports exactly one channel, backed by the existing
    /// step16+step17 bring-up ladder.
    nouveau_channels: Mutex<Vec<super::nouveau_uapi::NouveauChannelState>>,
    /// GEM objects allocated through the nouveau-uAPI `GEM_NEW`, distinct
    /// from `imported_handles` (which tracks buffers the generic DRM core
    /// allocated via `CREATE_DUMB`).
    nouveau_gem: Mutex<Vec<super::nouveau_uapi::NouveauGemObject>>,
    /// Next handle to hand out from `GEM_NEW`. Starts at 1 (0 is never a
    /// valid GEM handle, matching Linux DRM convention).
    nouveau_gem_next_handle: AtomicU32,
    /// Active `VM_BIND` GPU-VA mappings, so `UNMAP` can find the RM handle
    /// to tear down.
    nouveau_vm_mappings: Mutex<Vec<super::nouveau_uapi::NouveauVmMapping>>,
    /// Driver-private framebuffer objects keyed by driver fb id.
    kms_framebuffers: Mutex<Vec<NvidiaKmsFramebuffer>>,
    /// Driver-side ids for framebuffer objects.
    next_kms_fb_id: AtomicU32,
    /// Current KMS state exposed by GETCRTC/GETPLANE and used by wait_vblank.
    kms_state: Mutex<NvidiaKmsState>,
    /// MSI interrupt vector assigned by the PCI scan (`irq + 32`), or
    /// `usize::MAX` if this GPU has no MSI. Used only by the console-GPU GSP
    /// boot to bring the GPU's MSI delivery online across the SEC2-resume
    /// window (the Linux-faithful interrupt path). Set once, after construction.
    msi_vector: AtomicUsize,
}

/// Simple bitmap-based VRAM allocator for BAR1 aperture (4KB page granularity)
struct NvidiaVramAllocator {
    base_phys: u64,
    total_size: u64,
    bitmap: Vec<u64>,
}

impl NvidiaVramAllocator {
    fn new(base_phys: u64, total_size: u64) -> Self {
        let num_pages = (total_size / 4096) as usize;
        let num_u64s = num_pages.div_ceil(64);
        Self {
            base_phys,
            total_size,
            bitmap: alloc::vec![0; num_u64s],
        }
    }

    /// Unused (kept for a future purely-Rust-side allocation need): the
    /// nouveau-uAPI `GEM_NEW` handler (`nvidia.rs` `ioctl`) allocates
    /// through the real RM heap instead (`nvidia_rm_sys::rm_init::
    /// gem_alloc_vram`) so it shares RM's own VRAM bookkeeping rather than
    /// carving up the same physical range out-of-band with a second,
    /// independent allocator.
    #[allow(dead_code)]
    fn _alloc(&mut self, size: usize, align: usize) -> Option<u64> {
        let num_pages = size.div_ceil(4096);
        let align_pages = (align.max(4096) / 4096).max(1);
        let total_bits = (self.total_size / 4096) as usize;

        let mut count = 0;
        let mut start_bit = 0;

        for bit in 0..total_bits {
            let uidx = bit / 64;
            let ubit = bit % 64;
            let is_free = (self.bitmap[uidx] & (1 << ubit)) == 0;

            if is_free {
                if count == 0 {
                    if bit % align_pages != 0 {
                        continue;
                    }
                    start_bit = bit;
                }
                count += 1;
                if count >= num_pages {
                    for i in 0..num_pages {
                        let b = start_bit + i;
                        self.bitmap[b / 64] |= 1 << (b % 64);
                    }
                    return Some(self.base_phys + (start_bit as u64 * 4096));
                }
            } else {
                count = 0;
            }
        }
        None
    }

    fn free(&mut self, phys_addr: u64, size: usize) {
        let offset = phys_addr.saturating_sub(self.base_phys);
        if offset >= self.total_size {
            return;
        }
        let start_bit = (offset / 4096) as usize;
        let num_pages = size.div_ceil(4096);
        for i in 0..num_pages {
            let b = start_bit + i;
            if b / 64 < self.bitmap.len() {
                self.bitmap[b / 64] &= !(1 << (b % 64));
            }
        }
    }
}

impl NvidiaGpu {
    fn pitch_pixels(&self) -> usize {
        if let Some(p) = self.pitch_override {
            return (p / 4) as usize;
        }

        let width = self.info.width as usize;
        let height = self.info.height as usize;
        if width == 0 || height == 0 {
            return width;
        }

        // Accept moderately padded scanlines (for example 2048-wide alignment on
        // a 1920-wide mode) while rejecting BAR apertures that are far larger
        // than the visible framebuffer and would produce a bogus inferred pitch.
        const MAX_PITCH_PADDING_PIXELS: usize = 4096;
        let bytes_per_pixel = self.info.format.bytes() as usize;

        // If fb_size is suspiciously large (entire BAR), don't infer pitch from it.
        // A typical 1080p framebuffer is ~8MB. BARs are usually 256MB+.
        if self.info.fb_size >= 16 * 1024 * 1024 {
            return width;
        }

        let visible_size = width.saturating_mul(height).saturating_mul(bytes_per_pixel);

        if self.info.fb_size >= visible_size {
            let inferred = self.info.fb_size / height / bytes_per_pixel;
            if inferred >= width && inferred <= width + MAX_PITCH_PADDING_PIXELS {
                return inferred;
            }
        }

        width
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        device_id: u16,
        bar0: usize,
        fb_vaddr: usize,
        fb_size: usize,
        bar1_phys: u64,
        default_width: u32,
        default_height: u32,
        bar0_phys: u64,
        bar0_len: u64,
        bar2_phys: u64,
        bar2_len: u64,
        pci_domain: u32,
        pci_bus: u8,
        pci_device: u8,
    ) -> DeviceResult<Self> {
        // Boot path: identify from PCI ID only. BAR0 MMIO reads during early
        // driver init can stall the CPU indefinitely on some firmware/GPU combos
        // (screen frozen at 80%). PMC/VRAM/resolution probes are deferred.
        let (arch, gpu_model, vram_size_mb) = identify_gpu(device_id);

        let mut w = default_width;
        let mut h = default_height;
        let mut pitch_override = None;
        let final_fb_vaddr = fb_vaddr;

        // Check if this GPU matches the boot framebuffer (UEFI GOP)
        if let Some(boot_info) = *BOOT_FB_INFO.lock() {
            // How do we know the physical address of fb_vaddr?
            // In zCore/drivers, we usually don't have a direct way back to phys,
            // but we can assume fb_vaddr is mapped to a BAR.
            // We'll trust the PCI scan to have passed the correct bar1_phys in some way,
            // but since we only have fb_vaddr here, we might need more info.
            // However, we can use a heuristic: if we have 2 GPUs, and boot_info.phys
            // is within the range of this GPU's BAR1, then this is the primary GPU.

            // For now, let's assume the caller will set the correct resolution
            // if it knows it. But if it doesn't, we can try to match.
            // Since we don't have the phys address of fb_vaddr here easily
            // without a page table lookup, let's rely on the fact that
            // KCONFIG info is usually more accurate than hardcoded 1920x1080.

            // If the default provided is the "magic" 1920x1080 from pci.rs,
            // and we have boot_info, use boot_info.
            if default_width == 1920 && default_height == 1080 {
                w = boot_info.width;
                h = boot_info.height;
                pitch_override = Some(boot_info.pitch);

                // If the boot phys is within this aperture, we might need to adjust fb_vaddr
                // But usually fb_vaddr is the start of the BAR. GOP might be offset.
                // In eclipse-old: fb_phys = boot_info.phys; offset = fb_phys - bar1_phys;
                // Here we'll just assume the pitch is the main fix needed for now.
                log::info!(
                    "[NVIDIA] Inheriting boot resolution: {}x{} (pitch: {})",
                    w,
                    h,
                    boot_info.pitch
                );
            }
        }

        let temperature = read_temperature(bar0);

        log::warn!(
            "[NVIDIA] Detected {} ({:?}), VRAM: {} MB, Temp: {:?}°C, Res: {}x{}",
            gpu_model,
            arch,
            vram_size_mb,
            temperature,
            w,
            h
        );

        let pitch = pitch_override.unwrap_or(w * 4);

        let info = DisplayInfo {
            width: w,
            height: h,
            pitch,
            format: ColorFormat::ARGB8888,
            fb_base_vaddr: final_fb_vaddr,
            fb_size,
        };

        Ok(Self {
            name,
            info,
            architecture: arch,
            gpu_model,
            device_id,
            vram_size_mb,
            pitch_override,
            _bar0: bar0,
            _bar1: final_fb_vaddr,
            bar1_phys,
            bar0_phys,
            bar0_len,
            bar2_phys,
            bar2_len,
            pci_domain,
            pci_bus,
            pci_device,
            vram_allocator: Mutex::new(Some(NvidiaVramAllocator::new(
                fb_vaddr as u64,
                fb_size as u64,
            ))),
            bringup: Mutex::new(None),
            rm_attach_result: Mutex::new(None),
            rm_device_instance: Mutex::new(None),
            gsp_firmware: Mutex::new(None),
            gsp_fw_status: Mutex::new(None),
            gsp_init_result: Mutex::new(None),
            state_init_result: Mutex::new(None),
            step10_result: Mutex::new(None),
            imported_handles: Mutex::new(Vec::new()),
            nouveau_channels: Mutex::new(Vec::new()),
            nouveau_gem: Mutex::new(Vec::new()),
            // High half of u32, disjoint from linux-object's own DRM_STATE
            // handle ids (CREATE_DUMB/PRIME, sequential starting at 1) --
            // both id spaces are decoded from the same fake-mmap-offset
            // bits by DrmDev::get_vmo, so a collision would resolve a
            // mmap() to the wrong physical range. See
            // drivers/src/scheme/gem_mmap.rs's module doc.
            nouveau_gem_next_handle: AtomicU32::new(0x8000_0001),
            nouveau_vm_mappings: Mutex::new(Vec::new()),
            kms_framebuffers: Mutex::new(Vec::new()),
            next_kms_fb_id: AtomicU32::new(1),
            kms_state: Mutex::new(NvidiaKmsState {
                crtc_fb: 0,
                plane_fb: 0,
                last_vblank_us: 0,
            }),
            msi_vector: AtomicUsize::new(usize::MAX),
        })
    }

    /// Record the MSI vector the PCI scan assigned this GPU (`irq + 32`). Called
    /// once from the probe; `None` (no MSI cap) leaves it as `usize::MAX`.
    pub fn set_msi_vector(&self, irq: Option<usize>) {
        if let Some(irq) = irq {
            self.msi_vector.store(irq + 32, Ordering::Relaxed);
        }
    }

    pub fn architecture(&self) -> NvidiaArchitecture {
        self.architecture
    }
    pub fn model(&self) -> &'static str {
        self.gpu_model
    }
    pub fn vram_size_mb(&self) -> u32 {
        self.vram_size_mb
    }
    pub fn temperature(&self) -> Option<i32> {
        read_temperature(self._bar0)
    }

    /// True if this GPU's BAR1 aperture contains the boot framebuffer — i.e. it
    /// is the GPU scanning out to the monitor. Such a GPU is spared from the
    /// copy-engine bring-up writes so a wedge can never blank the console.
    fn drives_boot_display(&self) -> bool {
        match boot_fb_phys() {
            Some(phys) if phys != 0 => {
                let lo = self.bar1_phys;
                let hi = lo.saturating_add(self.info.fb_size as u64);
                phys >= lo && phys < hi
            }
            _ => false,
        }
    }

    /// This GPU's PCI config-space location (Eclipse is single-segment, so
    /// domain is dropped; RM only ever runs function 0 of the GPU).
    fn cfg_loc(&self) -> pci::Location {
        pci::Location {
            bus: self.pci_bus,
            device: self.pci_device,
            function: 0,
        }
    }

    fn cfg_read16(&self, off: u16) -> u16 {
        unsafe {
            crate::bus::pci::PCI_ACCESS.read16(&crate::bus::pci::PortOpsImpl, self.cfg_loc(), off)
        }
    }

    fn cfg_read32(&self, off: u16) -> u32 {
        unsafe {
            crate::bus::pci::PCI_ACCESS.read32(&crate::bus::pci::PortOpsImpl, self.cfg_loc(), off)
        }
    }

    fn cfg_write16(&self, off: u16, val: u16) {
        unsafe {
            crate::bus::pci::PCI_ACCESS.write16(
                &crate::bus::pci::PortOpsImpl,
                self.cfg_loc(),
                off,
                val,
            )
        }
    }

    /// Offset of the PCI Express capability (cap id 0x10) in config space, or
    /// 0 if the function has none. Walks the standard capabilities list.
    fn pcie_cap_offset(&self) -> u8 {
        // Status register (0x06) bit 4 = capabilities list present.
        if self.cfg_read16(0x06) & (1 << 4) == 0 {
            return 0;
        }
        let mut ptr = (self.cfg_read16(0x34) & 0xFC) as u8; // capabilities pointer
        let mut guard = 0;
        while ptr != 0 && guard < 48 {
            let hdr = self.cfg_read16(ptr as u16);
            if (hdr & 0xFF) as u8 == 0x10 {
                return ptr;
            }
            ptr = ((hdr >> 8) & 0xFC) as u8; // next-capability pointer
            guard += 1;
        }
        0
    }

    /// Issue a PCIe Function Level Reset on this GPU. Returns true if issued.
    /// Follows the PCIe spec: confirm FLR capability, wait for pending
    /// transactions to drain, set Initiate FLR, then wait 100 ms for the reset
    /// to complete. Config state is intentionally NOT restored -- the caller
    /// resets the CPU immediately after, so the GPU only has to survive to the
    /// next firmware POST, which re-inits it from cold.
    fn pcie_flr(&self) -> bool {
        let cap = self.pcie_cap_offset();
        if cap == 0 {
            return false;
        }
        // Device Capabilities (cap+0x04) bit 28 = Function Level Reset capable.
        if self.cfg_read32((cap as u16) + 0x04) & (1 << 28) == 0 {
            return false;
        }
        // Wait (bounded) for Transactions Pending (Device Status cap+0x0A bit 5).
        let t0 = unsafe { crate::bus::drivers_timer_now_as_micros() };
        while self.cfg_read16((cap as u16) + 0x0A) & (1 << 5) != 0 {
            if unsafe { crate::bus::drivers_timer_now_as_micros() }.wrapping_sub(t0) > 100_000 {
                break;
            }
            core::hint::spin_loop();
        }
        // Set Initiate FLR (Device Control cap+0x08 bit 15).
        let devctl = self.cfg_read16((cap as u16) + 0x08);
        self.cfg_write16((cap as u16) + 0x08, devctl | (1 << 15));
        // PCIe requires up to 100 ms before the function is usable again.
        let t1 = unsafe { crate::bus::drivers_timer_now_as_micros() };
        while unsafe { crate::bus::drivers_timer_now_as_micros() }.wrapping_sub(t1) < 100_000 {
            core::hint::spin_loop();
        }
        true
    }

    fn imported_handle(&self, handle_id: u32) -> Option<ImportedGemHandle> {
        self.imported_handles
            .lock()
            .iter()
            .find(|h| h.id == handle_id)
            .copied()
    }

    fn kms_fb(&self, fb_id: u32) -> Option<NvidiaKmsFramebuffer> {
        self.kms_framebuffers
            .lock()
            .iter()
            .find(|f| f.id == fb_id)
            .copied()
    }

    fn present_kms_fb(&self, fb_id: u32) -> bool {
        use crate::bus::phys_to_virt;
        let Some(fb) = self.kms_fb(fb_id) else {
            return false;
        };
        if fb.phys_addr == 0 || fb.size < 4 || fb.pitch == 0 {
            return false;
        }
        let src_vaddr = phys_to_virt(fb.phys_addr as usize);
        let src = unsafe { core::slice::from_raw_parts(src_vaddr as *const u32, fb.size / 4) };
        let width = fb.width.min(self.info.width);
        let height = fb.height.min(self.info.height);
        let src_stride = (fb.pitch / 4) as usize;
        self.blit_from(0, 0, src, src_stride, width, height);
        let _ = self.flush();
        let now = unsafe { crate::bus::drivers_timer_now_as_micros() };
        let mut state = self.kms_state.lock();
        state.crtc_fb = fb.id;
        state.plane_fb = fb.id;
        state.last_vblank_us = now;
        true
    }

    /// Pre-boot hardware-state snapshot (Copilot/checklist item: "comparar
    /// dump de config-space/PMC/display regs primaria vs secundaria justo
    /// antes del resume"). Raw BAR0 reads only -- no RM involvement. Emitted
    /// into the /proc block AND live at ERROR level so the console GPU's
    /// values survive a wedge on screen. The interesting delta vs. the
    /// secondary: PDISP_VGA_WORKSPACE_BASE (live VGA workspace => bit0
    /// VALID) and BSI_SECURE_SCRATCH_14 (BRSS handoff state).
    fn dump_preboot_state(&self, tag: &str) -> String {
        use core::fmt::Write;
        let bar0 = self._bar0;
        let rd =
            |off: usize| -> u32 { unsafe { core::ptr::read_volatile((bar0 + off) as *const u32) } };
        let regs: [(&str, usize); 5] = [
            ("PMC_ENABLE", 0x000200),
            ("PDISP_VGA_WORKSPACE_BASE", 0x625F04),
            ("BSI_SECURE_SCRATCH_14", 0x1180F8),
            ("PBUS_BAR0_WINDOW", 0x001700),
            ("PMC_BOOT_0", 0x000000),
        ];
        let mut s = String::new();
        for (name, off) in regs {
            let v = rd(off);
            let line = alloc::format!("[{}] preboot {} ({:#08x}) = {:#010x}", tag, name, off, v);
            log::error!("{}", line);
            let _ = writeln!(s, "{}", line);
        }
        let cmd = {
            use crate::bus::pci::{PortOpsImpl, PCI_ACCESS};
            use pci::Location;
            let loc = Location {
                bus: self.pci_bus,
                device: self.pci_device,
                function: 0,
            };
            unsafe { PCI_ACCESS.read16(&PortOpsImpl, loc, 0x04) }
        };
        let line = alloc::format!("[{}] preboot PCI COMMAND = {:#06x}", tag, cmd);
        log::error!("{}", line);
        let _ = writeln!(s, "{}", line);
        // PCIe link config of this GPU and its root port: MPS (Max Payload
        // Size) and MRRS (Max Read Request Size) from the PCIe capability's
        // Device Control register. Linux's PCI core NORMALIZES MPS across
        // every tree at boot; Eclipse inherits whatever UEFI programmed, and
        // the two GPUs hang off DIFFERENT root ports. A GPU-vs-root-port MPS
        // mismatch on the primary's port (absent on the secondary's) would
        // explain a TLP-level stall no driver knob can fix -- the top
        // remaining hypothesis now that every Linux-visible knob is matched.
        // Pure config reads, zero risk.
        {
            use crate::bus::pci::{PortOpsImpl, PCI_ACCESS};
            use pci::Location;
            let ops = &PortOpsImpl;
            let pcie_dump = |loc: Location, label: &str| -> String {
                // Walk the capability list for the PCIe capability (ID 0x10).
                let status = unsafe { PCI_ACCESS.read16(ops, loc, 0x06) };
                if status & (1 << 4) == 0 {
                    return alloc::format!("[{}] preboot PCIe {}: no cap list", tag, label);
                }
                let mut ptr = unsafe { PCI_ACCESS.read8(ops, loc, 0x34) } as u16;
                let mut hops = 0;
                while ptr != 0 && hops < 48 {
                    let id = unsafe { PCI_ACCESS.read8(ops, loc, ptr) };
                    if id == 0x10 {
                        let devcap = unsafe { PCI_ACCESS.read32(ops, loc, ptr + 0x04) };
                        let devctl = unsafe { PCI_ACCESS.read16(ops, loc, ptr + 0x08) };
                        let devsta = unsafe { PCI_ACCESS.read16(ops, loc, ptr + 0x0A) };
                        let lnksta = unsafe { PCI_ACCESS.read16(ops, loc, ptr + 0x12) };
                        // MPS/MRRS encode as 128 << field.
                        let mps_cap = 128u32 << (devcap & 0x7);
                        let mps = 128u32 << ((devctl >> 5) & 0x7);
                        let mrrs = 128u32 << ((devctl >> 12) & 0x7);
                        return alloc::format!(
                            "[{}] preboot PCIe {}: DevCtl={:#06x} (MPS={} MRRS={}, cap {}), DevSta={:#06x}, LnkSta={:#06x} (gen{} x{})",
                            tag, label, devctl, mps, mrrs, mps_cap, devsta, lnksta,
                            lnksta & 0xF,
                            (lnksta >> 4) & 0x3F
                        );
                    }
                    ptr = unsafe { PCI_ACCESS.read8(ops, loc, ptr + 1) } as u16;
                    hops += 1;
                }
                alloc::format!("[{}] preboot PCIe {}: cap not found", tag, label)
            };
            let gpu_loc = Location {
                bus: self.pci_bus,
                device: self.pci_device,
                function: 0,
            };
            let line = pcie_dump(gpu_loc, "GPU");
            log::error!("{}", line);
            let _ = writeln!(s, "{}", line);
            if let Some((b, d, f)) = self.find_parent_bridge() {
                let rp_loc = Location {
                    bus: b,
                    device: d,
                    function: f,
                };
                let line = pcie_dump(rp_loc, "root-port");
                log::error!("{}", line);
                let _ = writeln!(s, "{}", line);
            }
        }
        // Sysmem flush buffer target (kern_mem_sys_gm107.c programs it; a
        // zero/garbage value on one GPU would resurrect that theory).
        let flush = rd(0x100C10);
        let line = alloc::format!(
            "[{}] preboot PFB_NISO_FLUSH_SYSMEM_ADDR (0x100c10) = {:#010x}",
            tag,
            flush
        );
        log::error!("{}", line);
        let _ = writeln!(s, "{}", line);
        // Display liveness: the scanout theory REQUIRES the primary's heads
        // to be AWAKE with an advancing raster before it can explain the
        // wedge. NV_PDISP_FE_CORE_HEAD_STATE(i)=0x612078+i*2048, mode bits
        // 9:8 (0=SLEEP,1=SNOOZE,2=AWAKE, dev_disp.h v04_00:31-35);
        // NV_PDISP_RG_DPCA(i)=0x616330+i*2048 (v03_00 header -- read-only
        // probe, 0xBADFxxxx = not present at that offset on v04) read twice
        // ~30ms apart: FRM/LINE counters advancing = live raster fetch.
        // Gated on PMC_ENABLE bit30 (PDISP engine enabled) to avoid priv
        // errors on a display-less config.
        if rd(0x000200) & (1 << 30) != 0 {
            let mut dpca_a = [0u32; 4];
            for (i, slot) in dpca_a.iter_mut().enumerate() {
                *slot = rd(0x616330 + i * 2048);
            }
            // ~30ms spin so a live raster visibly advances its counters.
            let t0 = unsafe { crate::bus::drivers_timer_now_as_micros() };
            while unsafe { crate::bus::drivers_timer_now_as_micros() }.wrapping_sub(t0) < 30_000 {
                core::hint::spin_loop();
            }
            for i in 0..4usize {
                let head = rd(0x612078 + i * 2048);
                let mode = (head >> 8) & 0x3;
                let dpca_b = rd(0x616330 + i * 2048);
                let line = alloc::format!(
                    "[{}] preboot head{} STATE={:#010x} (mode={} {}) DPCA {:#010x} -> {:#010x} ({})",
                    tag,
                    i,
                    head,
                    mode,
                    match mode { 0 => "SLEEP", 1 => "SNOOZE", 2 => "AWAKE", _ => "?" },
                    dpca_a[i],
                    dpca_b,
                    if dpca_a[i] != dpca_b { "ADVANCING = live raster" } else { "frozen" }
                );
                log::error!("{}", line);
                let _ = writeln!(s, "{}", line);
            }
        } else {
            let line = alloc::format!(
                "[{}] preboot PDISP disabled in PMC_ENABLE (no head dump)",
                tag
            );
            log::error!("{}", line);
            let _ = writeln!(s, "{}", line);
        }
        s
    }

    /// Read-only discriminating dump for `/proc/gpudump`: labels the GPU by
    /// role (console vs secondary) and its PCI location, then the full
    /// register snapshot. Safe -- pure BAR0/config reads, no boot.
    fn hw_dump_impl(&self) -> String {
        let role = if self.drives_boot_display() {
            "CONSOLE/primary"
        } else {
            "secondary/headless"
        };
        let mut s = alloc::format!(
            "[gpudump] === {} GPU {:02x}:{:02x}.0 (bar0_phys={:#x} bar1_phys={:#x} vram={}MB) ===\n",
            role, self.pci_bus, self.pci_device, self.bar0_phys, self.bar1_phys, self.vram_size_mb
        );
        s.push_str(&self.dump_preboot_state("gpudump"));
        s
    }

    /// Packed config-space handle for THIS GPU (os_pci_init_handle format:
    /// valid-tag | bus<<16 | device<<8 | function).
    fn config_handle(&self) -> usize {
        0x8000_0000usize | ((self.pci_bus as usize) << 16) | ((self.pci_device as usize) << 8)
    }

    /// Packed config-space handle for the immediate upstream bridge, 0 if
    /// none found.
    fn parent_config_handle(&self) -> usize {
        self.find_parent_bridge()
            .map(|(b, d, f)| {
                0x8000_0000usize | ((b as usize) << 16) | ((d as usize) << 8) | f as usize
            })
            .unwrap_or(0)
    }

    /// Immediate upstream bridge (the one whose secondary bus IS this GPU's
    /// bus) -- the root port for a directly-attached GPU.
    fn find_parent_bridge(&self) -> Option<(u8, u8, u8)> {
        use crate::bus::pci::{PortOpsImpl, PCI_ACCESS};
        use pci::Location;
        let ops = &PortOpsImpl;
        for bus in 0..=self.pci_bus {
            for dev in 0..32u8 {
                for func in 0..8u8 {
                    let loc = Location {
                        bus,
                        device: dev,
                        function: func,
                    };
                    let vend = unsafe { PCI_ACCESS.read16(ops, loc, 0x00) };
                    if vend == 0xFFFF {
                        continue;
                    }
                    let hdr = unsafe { PCI_ACCESS.read8(ops, loc, 0x0E) };
                    if hdr & 0x7F != 0x01 {
                        continue;
                    }
                    let sec = unsafe { PCI_ACCESS.read8(ops, loc, 0x19) };
                    if sec == self.pci_bus {
                        return Some((bus, dev, func));
                    }
                }
            }
        }
        None
    }

    /// Containment: program the root port's PCIe Completion Timeout so a
    /// dead endpoint turns CPU reads into bounded all-ones completions
    /// instead of an unbounded core stall. Best-effort -- logs what it
    /// found; if the platform doesn't support CTO ranges (DevCap2[3:0]==0)
    /// nothing is written.
    fn arm_completion_timeout(&self) -> String {
        use crate::bus::pci::{PortOpsImpl, PCI_ACCESS};
        use core::fmt::Write;
        use pci::Location;
        let mut s = String::new();
        let Some((b, d, f)) = self.find_parent_bridge() else {
            let _ = writeln!(s, "[gpustep11] CTO: no parent bridge found");
            return s;
        };
        let ops = &PortOpsImpl;
        let loc = Location {
            bus: b,
            device: d,
            function: f,
        };
        // Walk the capability list for the PCIe capability (ID 0x10).
        let mut ptr = unsafe { PCI_ACCESS.read8(ops, loc, 0x34) };
        let mut cap = 0u8;
        for _ in 0..16 {
            if ptr == 0 || ptr == 0xFF {
                break;
            }
            let id = unsafe { PCI_ACCESS.read8(ops, loc, ptr as u16) };
            if id == 0x10 {
                cap = ptr;
                break;
            }
            ptr = unsafe { PCI_ACCESS.read8(ops, loc, ptr as u16 + 1) };
        }
        if cap == 0 {
            let _ = writeln!(
                s,
                "[gpustep11] CTO: root port {:02x}:{:02x}.{} has no PCIe cap?",
                b, d, f
            );
            return s;
        }
        let devcap2 = unsafe { PCI_ACCESS.read32(ops, loc, cap as u16 + 0x24) };
        let ranges = devcap2 & 0xF;
        let dc2 = unsafe { PCI_ACCESS.read16(ops, loc, cap as u16 + 0x28) };
        if ranges == 0 {
            let _ = writeln!(
                s,
                "[gpustep11] CTO: root port {:02x}:{:02x}.{} supports no timeout ranges (DevCap2={:#010x}, DC2={:#06x}) -- containment unavailable",
                b, d, f, devcap2, dc2
            );
            return s;
        }
        // Pick the shortest supported range: A(bit0)->0b0001, B->0b0101,
        // C->0b1001, D->0b1101 (PCIe base spec encoding).
        let val: u16 = if ranges & 1 != 0 {
            0b0001
        } else if ranges & 2 != 0 {
            0b0101
        } else if ranges & 4 != 0 {
            0b1001
        } else {
            0b1101
        };
        let new_dc2 = (dc2 & !0x001F) | val; // clear CTO-disable (bit4) + set range
        unsafe { PCI_ACCESS.write16(ops, loc, cap as u16 + 0x28, new_dc2) };
        let _ = writeln!(
            s,
            "[gpustep11] CTO armed on root port {:02x}:{:02x}.{}: DevCap2={:#010x} DC2 {:#06x} -> {:#06x} (reads of a dead endpoint now complete all-ones instead of hanging, chipset permitting)",
            b, d, f, devcap2, dc2, new_dc2
        );
        s
    }

    /// Disable (or restore) legacy VGA routing on every PCI bridge between
    /// the root and this GPU -- PCI Bridge Control (offset 0x3E) bit 3
    /// "VGA Enable". Copilot/checklist item: the earlier experiment only
    /// cleared the GPU function's own I/O decode; the full chain includes
    /// the root port/bridges that forward VGA cycles. Returns the list of
    /// (bus, device, function, old bridge-control value) actually changed,
    /// for the caller to restore afterwards.
    fn set_path_vga_routing(
        &self,
        disable: bool,
        restore: &[(u8, u8, u8, u16)],
    ) -> (String, Vec<(u8, u8, u8, u16)>) {
        use crate::bus::pci::{PortOpsImpl, PCI_ACCESS};
        use core::fmt::Write;
        use pci::Location;
        let ops = &PortOpsImpl;
        let mut changed: Vec<(u8, u8, u8, u16)> = Vec::new();
        let mut s = String::new();
        if disable {
            // Walk every bus below the GPU's: any bridge whose
            // [secondary..subordinate] window routes the GPU's bus is on the
            // path (covers nested switches too, not just the root port).
            for bus in 0..=self.pci_bus {
                for dev in 0..32u8 {
                    for func in 0..8u8 {
                        let loc = Location {
                            bus,
                            device: dev,
                            function: func,
                        };
                        let vend = unsafe { PCI_ACCESS.read16(ops, loc, 0x00) };
                        if vend == 0xFFFF {
                            continue;
                        }
                        let hdr = unsafe { PCI_ACCESS.read8(ops, loc, 0x0E) };
                        if hdr & 0x7F != 0x01 {
                            continue; // not a PCI-PCI bridge
                        }
                        let sec = unsafe { PCI_ACCESS.read8(ops, loc, 0x19) };
                        let sub = unsafe { PCI_ACCESS.read8(ops, loc, 0x1A) };
                        if !(sec <= self.pci_bus && self.pci_bus <= sub) {
                            continue;
                        }
                        let bctl = unsafe { PCI_ACCESS.read16(ops, loc, 0x3E) };
                        if bctl & (1 << 3) != 0 {
                            unsafe { PCI_ACCESS.write16(ops, loc, 0x3E, bctl & !(1 << 3)) };
                            changed.push((bus, dev, func, bctl));
                            let _ = writeln!(
                                s,
                                "[gpustep11] bridge {:02x}:{:02x}.{} VGA routing disabled (BRIDGE_CTL {:#06x} -> {:#06x})",
                                bus, dev, func, bctl, bctl & !(1 << 3)
                            );
                        }
                    }
                }
            }
            if changed.is_empty() {
                let _ = writeln!(
                    s,
                    "[gpustep11] no bridge on the path had VGA routing enabled"
                );
            }
        } else {
            for &(bus, dev, func, old) in restore {
                let loc = Location {
                    bus,
                    device: dev,
                    function: func,
                };
                unsafe { PCI_ACCESS.write16(ops, loc, 0x3E, old) };
                let _ = writeln!(
                    s,
                    "[gpustep11] bridge {:02x}:{:02x}.{} VGA routing restored",
                    bus, dev, func
                );
            }
        }
        (s, changed)
    }

    /// Shared GSP-boot body used by `bringup_step6` (secondary GPU) and
    /// `bringup_step11` (console GPU, with the graphic console frozen by the
    /// /proc generator around this call): INTx mask, kgspInitRm, narration
    /// capture, per-GPU result cache. `tag` labels the output lines
    /// ("gpustep6"/"gpustep11") so each proc file reads naturally.
    /// PBUS PRI-error pre-boot diagnose + engine-level retire (console GPU).
    /// The workflow research identified the pre-STARTCPU pending LEAF[4] bit28
    /// (CPU vector 156, mirrored in legacy PMC_INTR0 bit 28) as PBUS -- the
    /// PRI (priv bus) error collector: nouveau maps legacy PMC bit 28 to
    /// NVKM_SUBDEV_BUS on this whole lineage, and its unit-level status is
    /// NV_PBUS_INTR_0 @ 0x1100 (PRI_SQUASH bit1 / PRI_FECSERR bit2 /
    /// PRI_TIMEOUT bit3), with the FAULTING PRI ADDRESS latched in 0x9084 and
    /// the write data in 0x9088 (nouveau gf100_bus_intr, valid through Turing:
    /// tu102 uses gf100_bus in non-GSP nouveau). A leaf W1C can never retire
    /// it -- the level line follows the unit latch, which is why EXP3's leaf
    /// clear re-asserted "within microseconds". nouveau's documented quench:
    /// read 0x9084/0x9088 for diagnosis, write 0x9084=0, then W1C the handled
    /// bits into 0x1100. Runs BEFORE kgspInitRm with full logging (safe: no
    /// SEC2 resume in flight), so one boot both names the original PRI fault
    /// (smoking gun for WHY only the GOP/primary GPU has it latched) and
    /// retires the level source for real instead of racing it.
    fn pbus_pri_diagnose_and_clear(&self, tag: &str) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let bar0 = self._bar0;
        let rd = |off: usize| unsafe { core::ptr::read_volatile((bar0 + off) as *const u32) };
        let wr =
            |off: usize, v: u32| unsafe { core::ptr::write_volatile((bar0 + off) as *mut u32, v) };
        // Looks like the 0xBADFxxxx PRI-error sentinel (register absent /
        // priv fault)? Never write to a register that read back as one.
        let is_badf = |v: u32| (v & 0xFFFF_0000) == 0xBADF_0000;

        let intr0 = rd(0x1100);
        let save0 = rd(0x9084);
        let save1 = rd(0x9088);
        // LR10-lineage candidates (the only offsets published in this tree);
        // reads may themselves fault (sentinel) -- we clear PBUS right after,
        // so a probe-induced PRI_TIMEOUT is retired too.
        let lr_save0 = rd(0x1984);
        let lr_save1 = rd(0x1988);
        let lr_errc = rd(0x198C);
        let leaf4 = rd(0x00B8_1010);
        let pmc0 = rd(0x100);
        let _ = writeln!(
            s,
            "[{}] PBUS pre-boot: INTR_0={:#010x} (SQUASH={} FECSERR={} TIMEOUT={}) SAVE_0(0x9084)={:#010x} SAVE_1(0x9088)={:#010x}",
            tag,
            intr0,
            (intr0 >> 1) & 1,
            (intr0 >> 2) & 1,
            (intr0 >> 3) & 1,
            save0,
            save1
        );
        if save0 != 0 && !is_badf(save0) {
            let _ = writeln!(
                s,
                "[{}] PBUS latched PRI fault: {} of data {:#010x} at PRI address {:#08x}",
                tag,
                if save0 & 0x2 != 0 { "WRITE" } else { "READ" },
                save1,
                save0 & 0x00FF_FFFC
            );
        }
        let _ = writeln!(
            s,
            "[{}] PBUS alt regs: 0x1984={:#010x} 0x1988={:#010x} 0x198C={:#010x}; LEAF[4]={:#010x} PMC_INTR0={:#010x}",
            tag, lr_save0, lr_save1, lr_errc, leaf4, pmc0
        );
        log::error!("{}", s.trim_end());
        // Retire at the unit: clear the fault latch, then W1C the status.
        if !is_badf(save0) {
            wr(0x9084, 0);
        }
        // 0x1984 (LR10-lineage SAVE_0) stays READ-ONLY: that offset is only
        // published for NVSwitch/LR10 and was never verified on Turing -- a
        // blind write could hit an unrelated register. The nouveau-documented
        // Turing clear path (0x9084=0 + W1C 0x1100) above already covers it.
        let intr0_now = rd(0x1100);
        if intr0_now != 0 && !is_badf(intr0_now) {
            wr(0x1100, intr0_now);
        }
        // Leaf W1C after the unit clear, then verify the level line dropped.
        let leaf4_mid = rd(0x00B8_1010);
        if leaf4_mid != 0 && !is_badf(leaf4_mid) {
            wr(0x00B8_1010, leaf4_mid);
        }
        let leaf4_after = rd(0x00B8_1010);
        let pmc0_after = rd(0x100);
        let intr0_after = rd(0x1100);
        let verdict = if leaf4_after & 0x1000_0000 == 0 && pmc0_after & 0x1000_0000 == 0 {
            "RETIRED (level source quenched at the unit)"
        } else {
            "STILL PENDING (source is not PBUS-latch-only; see values)"
        };
        let tail = alloc::format!(
            "[{}] PBUS after clear: INTR_0={:#010x} LEAF[4]={:#010x} PMC_INTR0={:#010x} -> {}",
            tag,
            intr0_after,
            leaf4_after,
            pmc0_after,
            verdict
        );
        log::error!("{}", tail);
        let _ = writeln!(s, "{}", tail);
        s
    }

    /// Find the PCIe capability (ID 0x10) offset in config space, or None.
    fn pcie_cap_ptr(loc: pci::Location) -> Option<u16> {
        use crate::bus::pci::{PortOpsImpl, PCI_ACCESS};
        let ops = &PortOpsImpl;
        let status = unsafe { PCI_ACCESS.read16(ops, loc, 0x06) };
        if status & (1 << 4) == 0 {
            return None;
        }
        let mut ptr = unsafe { PCI_ACCESS.read8(ops, loc, 0x34) } as u16;
        let mut hops = 0;
        while ptr != 0 && hops < 48 {
            let id = unsafe { PCI_ACCESS.read8(ops, loc, ptr) };
            if id == 0x10 {
                return Some(ptr);
            }
            ptr = unsafe { PCI_ACCESS.read8(ops, loc, ptr + 1) } as u16;
            hops += 1;
        }
        None
    }

    /// PCIe MPS normalization -- Eclipse's equivalent of Linux's
    /// pcie_bus_config tree walk, applied to the one link that matters.
    /// ROOT CAUSE (gpudump on real hardware): UEFI left BOTH GPUs with
    /// DevCtl MPS=256 while BOTH root ports sit at MPS=128 -- a protocol
    /// violation. A GPU-sourced upstream TLP with a >128-byte payload is a
    /// Malformed TLP at the root port; with no OS AER handling the port stops
    /// releasing flow-control credits and every subsequent access through it
    /// stalls -- the CPU then wedges on its next posted write, which is
    /// EXACTLY the observed physics (the Linux-byte-parity bare store to
    /// STARTCPU wedged; every driver-level knob had been equalized). The
    /// secondary GPU survives because its display-less SEC2-HS resume never
    /// generates such bursts. Linux never sees any of this because its PCI
    /// core normalizes MPS across every tree at boot -- the one Linux
    /// behavior Eclipse hadn't replicated. Clamp the GPU's MPS down to the
    /// root port's (lowering is always protocol-safe); MRRS is left alone
    /// (mismatched MRRS is legal -- completers split completions).
    fn normalize_mps(&self, tag: &str) -> String {
        use crate::bus::pci::{PortOpsImpl, PCI_ACCESS};
        use pci::Location;
        let ops = &PortOpsImpl;
        let gpu_loc = Location {
            bus: self.pci_bus,
            device: self.pci_device,
            function: 0,
        };
        let Some((b, d, f)) = self.find_parent_bridge() else {
            return alloc::format!(
                "[{}] MPS normalize: no parent bridge found (skipped)\n",
                tag
            );
        };
        let rp_loc = Location {
            bus: b,
            device: d,
            function: f,
        };
        let (Some(gpu_cap), Some(rp_cap)) =
            (Self::pcie_cap_ptr(gpu_loc), Self::pcie_cap_ptr(rp_loc))
        else {
            return alloc::format!("[{}] MPS normalize: PCIe cap not found (skipped)\n", tag);
        };
        let gpu_devctl = unsafe { PCI_ACCESS.read16(ops, gpu_loc, gpu_cap + 0x08) };
        let rp_devctl = unsafe { PCI_ACCESS.read16(ops, rp_loc, rp_cap + 0x08) };
        let gpu_mps = (gpu_devctl >> 5) & 0x7;
        let rp_mps = (rp_devctl >> 5) & 0x7;
        if gpu_mps <= rp_mps {
            return alloc::format!(
                "[{}] MPS already consistent (GPU {} <= root-port {}); nothing to do\n",
                tag,
                128u32 << gpu_mps,
                128u32 << rp_mps
            );
        }
        let new_devctl = (gpu_devctl & !(0x7 << 5)) | (rp_mps << 5);
        unsafe { PCI_ACCESS.write16(ops, gpu_loc, gpu_cap + 0x08, new_devctl) };
        let rb = unsafe { PCI_ACCESS.read16(ops, gpu_loc, gpu_cap + 0x08) };
        let line = alloc::format!(
            "[{}] MPS NORMALIZED (Linux pcie_bus_config equivalent): GPU DevCtl {:#06x} -> {:#06x} (readback {:#06x}); MPS {} -> {} to match root-port {}\n",
            tag,
            gpu_devctl,
            new_devctl,
            rb,
            128u32 << gpu_mps,
            128u32 << ((new_devctl >> 5) & 0x7),
            128u32 << rp_mps
        );
        log::error!("{}", line.trim_end());
        line
    }

    /// Secondary-bus-reset recovery after a detected post-STARTCPU fabric
    /// wedge (see os_boundary's wedge containment). All bridge/GPU accesses
    /// here are CONFIG space (root-complex-completed, can't hang the core)
    /// until the device answers config again; only then is fake-MMIO cleared
    /// and one BAR0 probe attempted. Returns (recovered, log).
    fn sbr_recover(&self, tag: &str, attempt: u32) -> (bool, String) {
        use crate::bus::pci::{PortOpsImpl, PCI_ACCESS};
        use core::fmt::Write;
        use pci::Location;
        let mut s = String::new();
        let ops = &PortOpsImpl;
        let _ = writeln!(
            s,
            "[{}] WEDGE DETECTED after STARTCPU (config space went all-ones); machine kept ALIVE; secondary-bus-reset recovery attempt #{}",
            tag, attempt
        );
        let Some((bb, bd, bf)) = self.find_parent_bridge() else {
            let _ = writeln!(s, "[{}] recovery: no parent bridge found; cannot SBR", tag);
            return (false, s);
        };
        let bridge = Location {
            bus: bb,
            device: bd,
            function: bf,
        };
        let gpu = Location {
            bus: self.pci_bus,
            device: self.pci_device,
            function: 0,
        };
        let spin_ms = |ms: u64| {
            let t0 = unsafe { crate::bus::drivers_timer_now_as_micros() };
            while unsafe { crate::bus::drivers_timer_now_as_micros() }.wrapping_sub(t0) < ms * 1000
            {
                core::hint::spin_loop();
            }
        };
        unsafe {
            let bc = PCI_ACCESS.read16(ops, bridge, 0x3E);
            PCI_ACCESS.write16(ops, bridge, 0x3E, bc | 0x40); // Secondary Bus Reset
            spin_ms(5);
            PCI_ACCESS.write16(ops, bridge, 0x3E, bc);
        }
        spin_ms(250); // link retrain + device-ready time
                      // Restore the GPU's config: BARs (address bits; RO flag bits are
                      // ignored by the device), COMMAND (MEM+BME, INTx masked). The GPU's
                      // ROM-based GFW/IFR re-runs its own boot after a hot reset;
                      // kgspInitRm's kgspWaitForGfwBootOk then waits for it like on a
                      // cold boot (the vfio/VM-passthrough flow relies on exactly this).
        unsafe {
            PCI_ACCESS.write32(ops, gpu, 0x10, self.bar0_phys as u32);
            PCI_ACCESS.write32(ops, gpu, 0x14, self.bar1_phys as u32);
            PCI_ACCESS.write32(ops, gpu, 0x18, (self.bar1_phys >> 32) as u32);
            PCI_ACCESS.write32(ops, gpu, 0x1C, self.bar2_phys as u32);
            PCI_ACCESS.write32(ops, gpu, 0x20, (self.bar2_phys >> 32) as u32);
            PCI_ACCESS.write16(ops, gpu, 0x04, 0x0406);
        }
        s.push_str(&self.normalize_mps(tag));
        let id = unsafe { PCI_ACCESS.read32(ops, gpu, 0x00) };
        let _ = writeln!(s, "[{}] recovery: post-SBR config ID = {:#010x}", tag, id);
        if id & 0xFFFF != 0x10DE {
            let _ = writeln!(
                s,
                "[{}] recovery FAILED (no config answer); console rendering stays suppressed -- capture this /proc output to a file and reboot",
                tag
            );
            return (false, s);
        }
        // Device answers config again: clear fake-MMIO and risk ONE BAR0
        // probe (without it no retry is possible anyway).
        nvidia_rm_sys::os_boundary::wedge_fake_mmio_clear();
        let boot0 = unsafe { core::ptr::read_volatile(self._bar0 as *const u32) };
        let _ = writeln!(
            s,
            "[{}] recovery: BAR0 PMC_BOOT_0 = {:#010x}; re-enabling console rendering, retrying GSP boot",
            tag, boot0
        );
        nvidia_rm_sys::os_interface::console_quiet_end();
        log::error!("{}", s.trim_end());
        (true, s)
    }

    fn gsp_boot_run(&self, tag: &str, quiet: bool) -> String {
        use core::fmt::Write;
        let device_instance = *self.rm_device_instance.lock();

        // Check the cache before touching gsp_firmware's lock at all, so
        // the two locks are never nested across the FFI call below (same
        // reasoning as bringup_step5).
        let cached = self.gsp_init_result.lock().clone();

        // Cache the ENTIRE block (captured GSP-RM boot narration + result
        // line) so the /proc generator is idempotent across cat's chunked
        // reads -- same requirement (and same fix) as bringup_step5.
        if let Some(cached) = cached {
            cached
        } else if let Some(device_instance) = device_instance {
            let fw = self.gsp_firmware.lock();
            if let Some(fw_bytes) = fw.as_ref() {
                // Snapshot the pre-boot hardware state first (diffable
                // primary-vs-secondary; survives a wedge via the live echo).
                let preboot = self.dump_preboot_state(tag);
                // Normalize this GPU's PCIe MPS to its root port BEFORE any
                // GSP traffic -- the root-cause fix (see normalize_mps).
                let mps_log = self.normalize_mps(tag);
                // Mask this GPU's legacy INTx at the PCI level before booting
                // GSP-RM. On real hardware the boot now gets all the way to
                // "GSP FW RM ready." and THEN the machine livelocks: once
                // GSP-RM is alive it asserts interrupts (RPC completions, log
                // buffers, NOCAT posts), and Eclipse has no ISR for the GPU --
                // nobody acks or masks a level-triggered INTx, so it screams
                // and starves the CPU. Linux never sees this because the RM
                // registers its ISR before RmInitAdapter. Eclipse's bring-up
                // is 100% polled (the RPC message queue is read directly), so
                // the correct equivalent is to keep the device's INTx
                // disabled: PCI COMMAND register (offset 4) bit 10 (Interrupt
                // Disable), the standard way a polled driver quiesces a
                // function. MSI/MSI-X were never enabled, so INTx is the only
                // line it can raise.
                {
                    use crate::bus::pci::{PortOpsImpl, PCI_ACCESS};
                    use pci::Location;
                    let loc = Location {
                        bus: self.pci_bus,
                        device: self.pci_device,
                        function: 0,
                    };
                    let ops = &PortOpsImpl;
                    let cmd = unsafe { PCI_ACCESS.read16(ops, loc, 0x04) };
                    unsafe { PCI_ACCESS.write16(ops, loc, 0x04, cmd | (1 << 10)) };
                    log::warn!(
                        "[NVIDIA] {}: PCI INTx disabled before GSP boot (COMMAND {:#06x} -> {:#06x})",
                        tag,
                        cmd,
                        cmd | (1 << 10)
                    );
                }
                // Capture kgspInitRm's own nv_printf / assert / ECLIPSE_TRACE
                // narration -- the GSP boot is the deepest step and its RM
                // LEVEL_ERROR failure lines only reach the user folded in
                // here (the kernel log::warn! stream is invisible on the
                // bring-up box; see bringup_step5).
                nvidia_rm_sys::os_interface::capture_begin();
                // Arm the sequencer register trace for EVERY GSP boot: it goes
                // live at the RUN_CPU_SEQUENCER RPC and records each register
                // access into the capture buffer (readable later in this
                // /proc file) -- and onto the live screen too when live_echo
                // is armed (step 11's console-GPU boot). The successful
                // secondary boot thus yields a full reference sequence to
                // diff against the console GPU's wedge point.
                nvidia_rm_sys::os_boundary::seq_trace_arm();
                // Console-GPU SEC2-resume mitigation stack. History: the
                // primary GPU (live GOP scanout) wedged at the SEC2 STARTCPU
                // posted write with CPU vector 156 (LEAF[4] bit28, mirrored in
                // legacy PMC_INTR0 bit28) pending-but-masked. Research
                // identified that source as PBUS (the PRI-error collector; a
                // PRI fault sits latched in NV_PBUS_INTR_0 until cleared at
                // the unit -- leaf W1C can never retire it, hence EXP3's
                // "re-asserts within microseconds"). It is retired for real by
                // pbus_pri_diagnose_and_clear() below, BEFORE the boot. The
                // pre-STARTCPU leaf drain stays armed as belt-and-braces (it
                // correlated with the one lucky pre-fix success), and the real
                // Linux-faithful fix is the console-quiet window (see `quiet`):
                // prior-art (nouveau r535 / RM / nova-core) does the STARTCPU
                // write unconditionally -- what they all ALSO do, and we
                // didn't, is never touch the console framebuffer (this GPU's
                // BAR1!) during the boot. The secondary/headless GPU needs
                // none of this.
                let drain_for_console = self.drives_boot_display();
                if drain_for_console {
                    if quiet {
                        // Linux byte-parity: the STARTCPU bracket contributes
                        // ZERO extra MMIO (no BSI pre-read, no intr snapshot,
                        // no drain) -- stock kflcnStartCpu is read CPUCTL then
                        // write CPUCTL_ALIAS with nothing between. The
                        // console-silent + PBUS-clean run still wedged, and
                        // the one successful boot ran WITHOUT the display/
                        // priv-ring probe reads the snapshot later added, so
                        // our own in-window MMIO is the prime remaining
                        // suspect. step13 (loud) keeps the drain+diagnostics.
                        nvidia_rm_sys::os_boundary::linux_parity_arm();
                    } else {
                        nvidia_rm_sys::os_boundary::sec2_drain_arm();
                    }
                }
                // Console GPU: diagnose + retire the latched PBUS PRI error at
                // the unit BEFORE the boot (fully logged -- no SEC2 window in
                // flight yet), so the LEAF[4] bit28 level source is quenched
                // for real instead of raced at STARTCPU time. See the method's
                // doc comment for the research trail.
                let pbus_log = if drain_for_console {
                    self.pbus_pri_diagnose_and_clear(tag)
                } else {
                    String::new()
                };
                // Console-quiet window (Linux console_lock equivalent) when the
                // caller asked for it: the console framebuffer lives in THIS
                // GPU's BAR1, and every prior console-GPU boot interleaved live
                // seq-trace pixel writes with the sequencer MMIO -- the one
                // thing Linux explicitly forbids around kgspInitRm
                // (osinit.c:1841: "to ensure no console writes through BAR1
                // can interfere"). Everything is still captured and folded
                // into this /proc read afterwards; only live rendering stops.
                if quiet && drain_for_console {
                    log::error!(
                        "[NVIDIA] {}: entering console-silent GSP boot window (Linux console_lock equivalent) -- next render after kgspInitRm returns",
                        tag
                    );
                    nvidia_rm_sys::os_interface::console_quiet_begin();
                }
                // Arm the post-STARTCPU wedge watch (console GPU only): if
                // the fabric dies, the machine survives with fake-MMIO and
                // we get to attempt SBR recovery + retry -- converting the
                // ~25-30% per-boot race into up to 3 chances per boot.
                if drain_for_console {
                    nvidia_rm_sys::os_boundary::wedge_watch_arm(self.config_handle());
                    // GPU-independent survival breadcrumb: mark that a console
                    // boot began and zero the RM narration counter, so a wedge
                    // is legible next boot via /proc/gpusurvive even if nothing
                    // else survives (no serial, no /proc, dark framebuffer).
                    nvidia_rm_sys::survival::reset_narration();
                    nvidia_rm_sys::survival::checkpoint(
                        nvidia_rm_sys::survival::milestone::INITRM_CALL,
                    );
                }
                // Linux-faithful interrupt path: bring the GPU's MSI delivery
                // online for the SEC2-resume window instead of running fully
                // INTx-masked. The wedge (foto 1) is the STARTCPU posted store
                // stalling with every CPU-visible interrupt source already
                // clean — consistent with the SEC2/GSP needing an interrupt
                // *delivered* (posted to the LAPIC) for forward progress, which
                // a fully INTx-masked GPU can never provide. The ISR closure
                // must NOT touch BAR0 (a CPU->GPU access wedges in the window):
                // it only counts and self-limits, since the mere MSI delivery
                // (outbound GPU->LAPIC) is the forward-progress signal and the
                // IRQ framework EOIs after it returns.
                let msi_vec = if drain_for_console {
                    self.msi_vector.load(Ordering::Relaxed)
                } else {
                    usize::MAX
                };
                if msi_vec != usize::MAX {
                    nvidia_rm_sys::survival::msi_set_online(msi_vec);
                    let v = msi_vec;
                    let handler: crate::scheme::IrqHandler = alloc::sync::Arc::new(move || {
                        const STORM_CAP: usize = 200_000;
                        // Counter lives in nvidia-rm-sys so the STARTCPU bracket
                        // (os_boundary) can print it on the frozen screen.
                        if nvidia_rm_sys::survival::msi_tick() == STORM_CAP {
                            // Runaway source we cannot clear at the engine
                            // without a wedge-prone BAR access: self-mask so a
                            // storm can never peg the CPU into a hang.
                            crate::net::msi_mask(v);
                        }
                    });
                    let ok = crate::net::msi_register_and_unmask(msi_vec, handler);
                    log::error!(
                        "[NVIDIA] {}: MSI delivery ONLINE for the GSP boot (vector {}, registered={})",
                        tag,
                        msi_vec,
                        ok
                    );
                } else {
                    log::error!(
                        "[NVIDIA] {}: NO MSI vector on this GPU (msi_vector unset) -- GSP boot runs INTx-masked as before; pci.rs found no legacy MSI cap (0x05). NVIDIA may expose only MSI-X (0x11).",
                        tag
                    );
                }
                let mut recovery_log = String::new();
                let mut attempt = 1u32;
                let computed = loop {
                    match nvidia_rm_sys::rm_init::init_gsp(device_instance, fw_bytes) {
                        Ok(()) => break String::from("kgspInitRm OK"),
                        Err(status) => {
                            let msg = alloc::format!(
                                "kgspInitRm FAILED, NV_STATUS={:#x} (attempt {})",
                                status,
                                attempt
                            );
                            if drain_for_console
                                && nvidia_rm_sys::os_boundary::wedge_detected()
                                && attempt < 3
                            {
                                let (recovered, rlog) = self.sbr_recover(tag, attempt);
                                recovery_log.push_str(&rlog);
                                if recovered {
                                    attempt += 1;
                                    nvidia_rm_sys::os_boundary::sec2_drain_arm();
                                    nvidia_rm_sys::os_boundary::seq_trace_arm();
                                    nvidia_rm_sys::os_boundary::wedge_watch_arm(
                                        self.config_handle(),
                                    );
                                    continue;
                                }
                            }
                            break msg;
                        }
                    }
                };
                nvidia_rm_sys::os_boundary::wedge_watch_disarm();
                if drain_for_console {
                    // Past kgspInitRm (OK or a clean NV_STATUS) — so any freeze
                    // recorded from here on was NOT the SEC2-window wedge.
                    nvidia_rm_sys::survival::checkpoint(
                        nvidia_rm_sys::survival::milestone::INITRM_RETURN,
                    );
                }
                // Take the GPU's MSI delivery back offline and report how many
                // MSIs were serviced during the boot — the empirical answer to
                // "do MSIs even fire, and does turning them on shift the wedge?".
                if msi_vec != usize::MAX {
                    crate::net::msi_mask_and_unregister(msi_vec);
                    let (_v, n) = nvidia_rm_sys::survival::msi_status();
                    nvidia_rm_sys::survival::msi_offline();
                    log::error!(
                        "[NVIDIA] {}: MSI delivery offline; {} MSI(s) serviced during the GSP boot{}",
                        tag,
                        n,
                        if n >= 200_000 {
                            " (STORM CAP hit -- source was masked mid-boot)"
                        } else {
                            ""
                        }
                    );
                }
                if quiet && drain_for_console {
                    nvidia_rm_sys::os_interface::console_quiet_end();
                    log::error!(
                        "[NVIDIA] {}: console-silent window exited (kgspInitRm returned)",
                        tag
                    );
                }
                if drain_for_console {
                    nvidia_rm_sys::os_boundary::linux_parity_disarm();
                    nvidia_rm_sys::os_boundary::sec2_drain_disarm();
                }
                nvidia_rm_sys::os_boundary::seq_trace_disarm();
                let captured = nvidia_rm_sys::os_interface::capture_take();
                drop(fw);
                let mut block = String::new();
                block.push_str(&preboot);
                block.push_str(&mps_log);
                block.push_str(&pbus_log);
                block.push_str(&recovery_log);
                if nvidia_rm_sys::os_boundary::wedge_fake_mmio_on() {
                    let _ = writeln!(
                        block,
                        "[{}] NOTE: fabric wedge unrecovered -- console rendering suppressed to keep the machine alive; this /proc output is intact (capture to a file + sync), then reboot.",
                        tag
                    );
                }
                if let Some(log) = captured {
                    if !log.is_empty() {
                        let _ = writeln!(block, "[{}]  --- GSP-RM narration (captured) ---", tag);
                        for line in log.lines() {
                            let _ = writeln!(block, "[{}]  | {}", tag, line);
                        }
                        let _ = writeln!(block, "[{}]  --- end GSP-RM narration ---", tag);
                    }
                }
                let _ = writeln!(block, "[{}]  --- Real GSP-RM boot: {} ---", tag, computed);
                let mut gsp = self.gsp_init_result.lock();
                if gsp.is_none() {
                    *gsp = Some(block.clone());
                }
                block
            } else {
                let status = self
                    .gsp_fw_status
                    .lock()
                    .clone()
                    .unwrap_or_else(|| String::from("no status recorded (loader never ran?)"));
                alloc::format!(
                    "[{}]  --- Real GSP-RM boot: skipped (no gsp.bin in driver) ---\n\
                     [{}]  boot-time firmware load: {}\n",
                    tag,
                    tag,
                    status
                )
            }
        } else {
            alloc::format!(
                "[{}]  --- Real GSP-RM boot: skipped (run /proc/gpustep5 (RM attach) first) ---\n",
                tag
            )
        }
    }

    /// Issue the tu102 GMMU invalidate for our channel's PDB and poll for
    /// completion. Returns `(pre, post, ok)` — the trigger register before and
    /// after, and whether bit31 cleared. Aborts (no write) if a flush is already
    /// in flight. This is the only GPU register write of Step 2.
    /// CPU-write a u32 into VRAM at raw VRAM offset `vram_off` via the PRAMIN
    /// window: point the window base (BAR0+0x1700 = off>>16), then access
    /// BAR0+0x700000+(off&0xFFFF). The window is 64 KiB; we re-point per access
    /// for simplicity. This is how the CPU reaches instmem (BAR1 is GMMU-remapped
    /// and cannot give a known VRAM-physical address).
    fn pramin_w32(&self, vram_off: u64, val: u32) {
        let bar0 = self._bar0;
        unsafe {
            core::ptr::write_volatile((bar0 + 0x1700) as *mut u32, (vram_off >> 16) as u32);
            core::ptr::write_volatile(
                (bar0 + 0x0070_0000 + (vram_off & 0xFFFF) as usize) as *mut u32,
                val,
            );
        }
    }

    fn pramin_r32(&self, vram_off: u64) -> u32 {
        let bar0 = self._bar0;
        unsafe {
            core::ptr::write_volatile((bar0 + 0x1700) as *mut u32, (vram_off >> 16) as u32);
            core::ptr::read_volatile(
                (bar0 + 0x0070_0000 + (vram_off & 0xFFFF) as usize) as *const u32,
            )
        }
    }

    fn pramin_zero(&self, vram_off: u64, len: usize) {
        for i in (0..len).step_by(4) {
            self.pramin_w32(vram_off + i as u64, 0);
        }
    }

    /// Write the channel instance block into VRAM (via PRAMIN). The host reads it
    /// as VRAM-physical. The PD-base at 0x200 points at the *sysmem* page tables
    /// (target=2). USERD pointer is VRAM-physical; the GPFIFO base is a GPU VA
    /// (GMMU-translated). Offsets per nouveau gv100_vmm_join / ramfc_write.
    /// Write the Turing VER2 PDB join (gv100_vmm_join) into a VRAM instance
    /// block via PRAMIN: PD-base @0x200, VA limit @0x208, and the 0x2a0
    /// subcontext descriptor table (entry 0 = real PDB, 1..63 = 0x1/0x1/0).
    /// Shared by the channel and BAR2 instance blocks. Assumes already zeroed.
    fn write_pdb_join_vram(&self, inst: u64, root_phys: u64) {
        let w32 = |off: u64, v: u32| self.pramin_w32(inst + off, v);
        let base = gmmu::inst_pd_base(root_phys); // root | 0xC06 (sysmem target)
        w32(0x200, base as u32);
        w32(0x204, (base >> 32) as u32);
        w32(0x208, ((1u64 << 49) - 1) as u32);
        w32(0x20c, (((1u64 << 49) - 1) >> 32) as u32);
        w32(0x21c, 0);
        w32(0x2a0, base as u32);
        w32(0x2a4, (base >> 32) as u32);
        w32(0x2a8, 0);
        for i in 1..64u64 {
            let o = 0x2a0 + i * 0x10;
            w32(o, 0x1);
            w32(o + 4, 0x1);
            w32(o + 8, 0);
        }
        w32(0x298, 0x1);
        w32(0x29c, 0x0);
    }

    fn write_instance_block_vram(&self, b: &GpuBringup) {
        let inst = b.inst_vram();
        self.pramin_zero(inst, 0x1000);
        let w32 = |off: u64, v: u32| self.pramin_w32(inst + off, v);
        // PD-base + VA limit + Turing PDB descriptor table.
        self.write_pdb_join_vram(inst, b.root.paddr() as u64);
        // RAMFC: USERD (VRAM phys), GPFIFO (GPU VA), ids.
        let userd = b.userd_vram();
        let gpfifo_va = b.gpfifo_va();
        let limit2 = (b.gpfifo.byte_len() as u64 / 8).trailing_zeros();
        w32(0x008, userd as u32);
        w32(0x00c, (userd >> 32) as u32);
        w32(0x010, 0x0000_face);
        w32(0x030, 0x7fff_f902);
        w32(0x048, gpfifo_va as u32);
        w32(0x04c, ((gpfifo_va >> 32) as u32) | (limit2 << 16));
        w32(0x084, 0x2040_0000);
        w32(0x094, 0x3000_0000 | 0xfff);
        // Fetched the real source (nvkm subdev/fifo/gv100.c, gv100_chan_ramfc):
        //   const struct nvkm_chan_func_ramfc gv100_chan_ramfc = {
        //       .write = gv100_chan_ramfc_write, .devm = 0xfff, .priv = true,
        //   };
        // `priv` is a FIXED property of the ramfc func table for this chip
        // generation, not a per-channel choice — EVERY gv100/tu102 channel
        // (client or kernel) uses priv=true. A previous commit here reasoned
        // priv should be false for a "normal client channel" and set
        // 0x0e4=0/0x0f4=0x1000; that directly contradicts the real source,
        // which always writes 0x0e4=(priv?0x20:0)=0x20 and
        // 0x0f4=0x1000|(priv?0x100:0)=0x1100 for this ramfc variant. Fixing
        // to match verbatim.
        w32(0x0e4, 0x0000_0020);
        w32(0x0e8, 0x0000_0000); // chan_id 0
        w32(0x0f4, 0x0000_1100);
        w32(0x0f8, 0x1000_3080);
        // CE/GR engine-context pointers (0x210-0x224, arm bits 0x10000/0x20000
        // at 0x0ac) are left ZERO: HOST never reads them during channel load —
        // only the engine does, on a faulting method, which never happens before
        // GP_GET advances. Arming a CE context with a BAR2 pointer is a red
        // herring for the load-time fault, so we bring up HOST first.
    }

    /// Arm the HUB MMU non-replayable fault buffer (buffer 0) so the host will
    /// schedule channels. NV_VIRTUAL_FUNCTION_PRIV_MMU_FAULT_BUFFER at 0xb83000:
    /// LO = addr|aperture|mode, HI = addr_hi, SIZE = count|ENABLE. We use
    /// PHYSICAL mode + SYS_COH aperture so the buffer is plain sysmem (no BAR2).
    /// Returns (hw_count, lo, hi, size) for reporting.
    fn setup_fault_buffer(&self, b: &GpuBringup) -> (u32, u32, u32, u32) {
        let bar0 = self._bar0;
        let rd =
            |off: u32| unsafe { core::ptr::read_volatile((bar0 + off as usize) as *const u32) };
        let wr = |off: u32, v: u32| unsafe {
            core::ptr::write_volatile((bar0 + off as usize) as *mut u32, v)
        };
        // Latch + read the HW-reported entry count (set bit30, clear ENABLE).
        wr(0x00b8_3010, (rd(0x00b8_3010) & !0xc000_0000) | 0x4000_0000);
        let hw_count = rd(0x00b8_3010) & 0x000f_ffff;
        // Our buffer holds at most 0x40000/32 = 0x2000 entries.
        let cap = (b.fault_buf.byte_len() / 32) as u32;
        let count = hw_count.min(cap);
        let phys = b.fault_buf.paddr() as u64;
        // LO: PHYSICAL(bit0=1) | PHYS_APERTURE SYS_COH(2<<1) | VOL(1<<3) | ADDR.
        let lo = (phys as u32 & 0xffff_f000) | 0x1 | (2 << 1) | (1 << 3);
        wr(0x00b8_3004, (phys >> 32) as u32);
        wr(0x00b8_3000, lo);
        // SIZE: entry count + ENABLE(bit31).
        wr(0x00b8_3010, count | 0x8000_0000);
        (hw_count, lo, (phys >> 32) as u32, rd(0x00b8_3010))
    }

    /// Set BAR2 live so the host can dereference the CE fault-method-buffer
    /// pointer (read by the BAR2 MMU as engine_id=BAR2/client=HOST_CPU). The
    /// BAR2 instance block (VRAM, via PRAMIN) points at the SAME page tables as
    /// the channel, so BAR2 VA == channel VA. Register per tu102_bar_bar2_init.
    fn setup_bar2(&self, b: &GpuBringup) -> (u32, u32, u32) {
        // Build the BAR2 instance block in VRAM with the FULL Turing VER2 PDB
        // join (PD-base + VA limit + the 0x2a0 descriptor table), same as a
        // channel — on Turing even a BAR vmm uses the VER2 join. Shared root.
        let bi = b.bar2_inst_vram();
        self.pramin_zero(bi, 0x1000);
        self.write_pdb_join_vram(bi, b.root.paddr() as u64);

        let bar0 = self._bar0;
        let rd =
            |off: u32| unsafe { core::ptr::read_volatile((bar0 + off as usize) as *const u32) };
        let wr = |off: u32, v: u32| unsafe {
            core::ptr::write_volatile((bar0 + off as usize) as *mut u32, v)
        };
        let before = rd(0x00b8_0f48);
        // 0xb80f48 = 0x80000000 | (bar2_inst_vram >> 12).
        wr(0x00b8_0f48, 0x8000_0000 | (bi >> 12) as u32);
        let after = rd(0x00b8_0f48);
        // Wait for the BAR2 bind to settle (0xb80f50 bits 0xc).
        let mut wait = 0;
        for _ in 0..1_000_000u64 {
            wait = rd(0x00b8_0f50);
            if wait & 0x0000_000c == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        (before, after, wait)
    }

    /// Write the runlist into VRAM (via PRAMIN): cgrp entry + chan entry. The
    /// USERD/inst pointers in the chan entry are VRAM-physical. Per nouveau
    /// gv100_runl_insert_cgrp/chan (chan_id=0, cgrp_id=0, chan_nr=1, runq=0).
    fn write_runlist_vram(&self, b: &GpuBringup) {
        let rl = b.runlist_vram();
        self.pramin_zero(rl, 0x20);
        let w32 = |off: u64, v: u32| self.pramin_w32(rl + off, v);
        let userd = b.userd_vram();
        let inst = b.inst_vram();
        w32(0x00, 0x8003_0001);
        w32(0x04, 1); // chan_nr
        w32(0x08, 0); // cgrp_id
        w32(0x0c, 0);
        w32(0x10, userd as u32); // | (runq<<1), runq=0
        w32(0x14, (userd >> 32) as u32);
        w32(0x18, inst as u32); // | chan_id, chan_id=0
        w32(0x1c, (inst >> 32) as u32);
    }

    /// Global FIFO + per-PBDMA init — the bring-up nouveau does in the fifo
    /// subdev BEFORE any channel commit, which we had skipped. Un-SUSPENDs the
    /// PBDMAs so the host will load a committed channel onto one. Order &
    /// values per nvkm fifo: tu102_fifo_init_pbdmas + gk208/gk104/gf100_runq_init
    /// + gk104_fifo_init. Idempotent.
    fn setup_fifo(&self) {
        let bar0 = self._bar0;
        let rd =
            |off: u32| unsafe { core::ptr::read_volatile((bar0 + off as usize) as *const u32) };
        let wr = |off: u32, v: u32| unsafe {
            core::ptr::write_volatile((bar0 + off as usize) as *mut u32, v)
        };
        // (0) PMC reset pulse for FIFO (nvkm_mc_reset, gk104_mc_reset[]: FIFO =
        // mask 0x00000100 at NV_PMC_ENABLE 0x000200). This is the FIRST thing
        // nouveau does for any engine before touching its registers — disable
        // then re-enable the bit, deasserting reset. We never did this: the
        // register *file* tolerates R/W while clock/reset-gated (writes latch,
        // reads echo them back), but the scheduler FSM that walks
        // PENDING -> ON_PBDMA never actually runs while FIFO sits in reset,
        // which matches every symptom seen so far (clean fault, clean writes,
        // zero scheduling progress). Idempotent — safe to repeat.
        wr(0x0000_0200, rd(0x0000_0200) & !0x0000_0100);
        let _ = rd(0x0000_0200);
        wr(0x0000_0200, rd(0x0000_0200) | 0x0000_0100);
        let _ = rd(0x0000_0200);
        // (A) doorbell-enable (tu102_fifo_init_pbdmas).
        wr(0x00b6_5000, rd(0x00b6_5000) | 0x8000_0000);
        // (B) per-PBDMA (runq) init, stride id*0x2000. NV_PFIFO_PBDMA_MAP has
        // up to 12 entries (same __SIZE_1=12 as the PBDMA_MAP scan elsewhere
        // in this file) -- 0..6 was NOT generous enough: a real-hardware run
        // discovered our CE's runlist is served by PBDMA9, which this loop
        // never touched. Its INTR_STALL/INTR_0/INTR_EN/TIMEOUT were left at
        // whatever the hardware defaulted to, and its GET/GP_GET registers
        // still held stale non-zero values from some prior context -- exactly
        // consistent with SCHED_STATUS.runlist_fetch_busy staying stuck at 1
        // forever and PBDMA9's CHANNEL register reading 0 (nothing ever
        // loaded). Cover the full range; writes to absent PBDMAs are harmless.
        for q in 0..12u32 {
            let s = q * 0x2000;
            // INTR_STALL: clear 0x10000100.
            wr(0x0004_013c + s, rd(0x0004_013c + s) & !0x1000_0100);
            wr(0x0004_0108 + s, 0xffff_ffff); // INTR_0   clear
            wr(0x0004_010c + s, 0xffff_feff); // INTR_EN_0
            wr(0x0004_0148 + s, 0xffff_ffff); // INTR_1   clear
            wr(0x0004_014c + s, 0xffff_ffff); // INTR_EN_1
            wr(0x0004_012c + s, 0x000f_4240); // TIMEOUT = 1000000
        }
        // (C) global fifo init (gk104_fifo_init).
        wr(0x0000_2100, 0xffff_ffff); // PFIFO INTR_0     clear
        wr(0x0000_2140, 0x7fff_ffff); // PFIFO INTR_EN_0
    }

    fn gmmu_flush(&self, root_phys: u64) -> (u32, u32, bool) {
        let bar0 = self._bar0;
        let rd =
            |off: u32| unsafe { core::ptr::read_volatile((bar0 + off as usize) as *const u32) };
        let wr = |off: u32, v: u32| unsafe {
            core::ptr::write_volatile((bar0 + off as usize) as *mut u32, v)
        };
        let pre = rd(0x00b8_30b0);
        if pre & 0x8000_0000 != 0 {
            return (pre, pre, false); // flush already pending — never stack
        }
        wr(0x00b8_30a0, (root_phys >> 8) as u32);
        wr(0x00b8_30a4, 0);
        wr(0x00b8_30b0, 0x8000_0001); // trigger PAGE_ALL invalidate
        let mut post = pre;
        let mut ok = false;
        for _ in 0..5_000_000u64 {
            post = rd(0x00b8_30b0);
            if post & 0x8000_0000 == 0 {
                ok = true;
                break;
            }
            core::hint::spin_loop();
        }
        (pre, post, ok)
    }

    /// Scan the PTOP device-info table (0x022700+i*4, 64 slots) for the copy
    /// engine's runlist id. Volta+ gives EVERY engine its own dedicated
    /// runlist (discovered, not fixed) — we had been assuming runlist 0 is
    /// the copy engine's without ever checking. Mirrors nvkm's
    /// gk104_top_parse exactly: each logical device spans 1+ consecutive
    /// 32-bit words (continuation while bit31 is set; the final word of an
    /// entry, bit31 clear, carries the ENGINE_TYPE -> NVKM engine dispatch).
    ///
    /// On this chip PTOP reports MULTIPLE CE-type entries (type 0x1/0x2/0x3/
    /// 0x13) with DIFFERENT runlist ids — some sharing GR's runlist (almost
    /// certainly a "GRCE", a copy engine reserved for GR context-switch use,
    /// not general DMA) and others standalone. Picking the first one blindly
    /// landed on the GRCE (runlist 0 == GR's runlist), which is plausibly
    /// why nothing ever go scheduled: GRCE's runlist may not be a normal
    /// user-DMA path at all. Prefer a CE runlist that does NOT match GR's.
    /// Returns (runlist_id, engine_id) for the chosen CE. `engine_id` is the
    /// PTOP ENUM word's "engine" field (bits 29:26, gated by bit5=0x20) — a
    /// THIRD id namespace, distinct from both runlist id and PBDMA index,
    /// used to index NV_PFIFO_ENGINE_STATUS(i) = 0x2640+i*8 (per-engine
    /// scheduler status: CTX_STATUS, FAULTED, ENGINE busy/idle). We had
    /// never read this register at all.
    fn find_ce_runlist(&self) -> Option<(u32, u32)> {
        let bar0 = self._bar0;
        let rd =
            |off: u32| unsafe { core::ptr::read_volatile((bar0 + off as usize) as *const u32) };
        let mut ty: u32 = !0;
        let mut have_entry = false;
        let mut runlist: u32 = 0;
        let mut have_runlist = false;
        let mut engine: u32 = 0;
        let mut have_engine = false;
        let mut gr_runlist: Option<u32> = None;
        let mut first_ce: Option<(u32, u32)> = None;
        let mut standalone_ce: Option<(u32, u32)> = None;
        for i in 0..64u32 {
            if !have_entry {
                ty = !0;
                have_runlist = false;
                have_engine = false;
                have_entry = true;
            }
            let data = rd(0x0002_2700 + i * 4);
            match data & 0x3 {
                0 => continue, // NOT_VALID — skip, keep accumulating this entry
                1 => {}        // DATA — addr/fault/inst, unused here
                2 => {
                    if data & 0x20 != 0 {
                        engine = (data >> 26) & 0xf;
                        have_engine = true;
                    }
                    if data & 0x10 != 0 {
                        runlist = (data >> 21) & 0xf;
                        have_runlist = true;
                    }
                }
                3 => ty = (data >> 2) & 0x1fff_ffff, // ENGINE_TYPE
                _ => unreachable!(),
            }
            if data & 0x8000_0000 != 0 {
                continue; // more words follow for this same entry
            }
            if have_runlist {
                if ty == 0x0 {
                    gr_runlist = Some(runlist);
                } else if matches!(ty, 0x1 | 0x2 | 0x3 | 0x13) {
                    let eng = if have_engine { engine } else { u32::MAX };
                    if first_ce.is_none() {
                        first_ce = Some((runlist, eng));
                    }
                    if standalone_ce.is_none() && Some(runlist) != gr_runlist {
                        standalone_ce = Some((runlist, eng));
                    }
                }
            }
            have_entry = false;
        }
        // Re-check standalone candidates against GR's runlist now that GR
        // (which can appear before OR after CE entries in the table) is
        // fully known — a single forward pass may have picked a CE entry
        // that only *looked* standalone before GR's own entry was parsed.
        if let Some(gr) = gr_runlist {
            if standalone_ce.map(|(rl, _)| rl) == Some(gr) {
                standalone_ce = None;
            }
        }
        standalone_ce.or(first_ce)
    }

    /// Same scan as `find_ce_runlist` but reports every finalized entry
    /// (type, inst, runlist) as text, for hardware visibility — does this
    /// chip even expose a runlist field for CE, and what does GR's look like
    /// for comparison.
    fn ptop_report(&self) -> alloc::string::String {
        use core::fmt::Write;
        let bar0 = self._bar0;
        let rd =
            |off: u32| unsafe { core::ptr::read_volatile((bar0 + off as usize) as *const u32) };
        let mut out = alloc::string::String::new();
        let mut ty: u32 = !0;
        let mut have_entry = false;
        let mut runlist: u32 = 0;
        let mut have_runlist = false;
        for i in 0..64u32 {
            if !have_entry {
                ty = !0;
                have_runlist = false;
                have_entry = true;
            }
            let data = rd(0x0002_2700 + i * 4);
            match data & 0x3 {
                0 => continue,
                1 => {}
                2 => {
                    if data & 0x10 != 0 {
                        runlist = (data >> 21) & 0xf;
                        have_runlist = true;
                    }
                }
                3 => ty = (data >> 2) & 0x1fff_ffff,
                _ => unreachable!(),
            }
            if data & 0x8000_0000 != 0 {
                continue;
            }
            let name = match ty {
                0x0 => "GR",
                0x1 | 0x2 | 0x3 | 0x13 => "CE",
                0x8 => "MSPDEC",
                0x9 => "MSPPP",
                0xa => "MSVLD",
                0xb => "MSENC",
                0xc => "VIC",
                0xd => "SEC2",
                0xe | 0xf => "NVENC",
                0x10 => "NVDEC",
                0x14 => "GSP",
                0x15 => "NVJPG",
                _ if ty == !0 => "?",
                _ => "OTHER",
            };
            if ty != !0 {
                let _ = write!(
                    out,
                    " {}(ty={:#x})/rl={}",
                    name,
                    ty,
                    if have_runlist { runlist as i64 } else { -1 }
                );
            }
            have_entry = false;
        }
        out
    }

    /// Idempotently bring the channel to the committed + enabled state (the
    /// Step 3 end-state): instance block, GMMU flush, runlist commit, doorbell
    /// and channel enable. Returns (commit_ok, runlist_id_used). Safe to
    /// repeat — used by Step 4+ so each is self-contained across reboots.
    fn setup_channel(&self, b: &GpuBringup) -> (bool, u32) {
        let runl_id = self.find_ce_runlist().map(|(rl, _)| rl).unwrap_or(0);
        const CHID: u32 = 0;
        let bar0 = self._bar0;
        let rd =
            |off: u32| unsafe { core::ptr::read_volatile((bar0 + off as usize) as *const u32) };
        let wr = |off: u32, v: u32| unsafe {
            core::ptr::write_volatile((bar0 + off as usize) as *mut u32, v)
        };

        self.write_instance_block_vram(b);
        self.write_runlist_vram(b);
        // Arm the HUB MMU fault buffer — required before any channel can run.
        let _ = self.setup_fault_buffer(b);
        let _ = self.setup_bar2(b);
        let _ = self.gmmu_flush(b.root.paddr() as u64);

        // Global FIFO + PBDMA init (un-SUSPEND the PBDMAs) — must precede the
        // runlist commit, else the host leaves the channel at STATUS=PENDING.
        self.setup_fifo();

        // Bind the channel's instance block in CHRAM so the host can find it
        // (gk104_chan_bind_inst: 0x800000+chid*8 = BIND | inst>>12, VRAM target).
        let inst_vram = b.inst_vram();
        wr(
            0x0080_0000 + CHID * 8,
            0x8000_0000 | (inst_vram >> 12) as u32,
        );

        // Ensure runlist scheduling is allowed (NV_PFIFO_SCHED_DISABLE bit=runl
        // id; gk104_runl_allow clears it). Default is 0, but clear it to be sure.
        wr(0x0000_2630, rd(0x0000_2630) & !(1u32 << runl_id));

        // Enable the channel BEFORE committing the runlist (nouveau order is
        // bind -> start(enable) -> commit; the commit is what loads the channel,
        // so it must see an enabled channel). gk104_chan_start: 0x800004 |= 0x400.
        wr(
            0x0080_0004 + CHID * 8,
            rd(0x0080_0004 + CHID * 8) | 0x0000_0400,
        );

        // tu102_chan_start does MORE than gk104_chan_start: right after the
        // PCCSR enable write it ALSO rings the doorbell immediately, with the
        // SAME token a later GPFIFO push would use (runl_id<<16 | chid). This
        // is the actual kick that wakes the HW scheduler to notice a freshly
        // enabled channel and pull it off PENDING — without it the channel
        // can sit at PENDING forever even after a clean runlist commit, which
        // is exactly the symptom we hit. device->vfn->addr.user + 0x0090 ==
        // BAR0 + 0xb80000(priv) + 0x030000(user) + 0x90 == 0xbb0090.
        let token = (runl_id << 16) | CHID;
        wr(0x00bb_0090, token);

        // Runlist commit LAST (2 entries). The runlist lives in VRAM; the host
        // reads it VRAM-physical, no target field needed (tu102_runl_commit).
        let base = 0x0000_2b00 + runl_id * 0x10;
        let runlist_vram = b.runlist_vram();
        wr(base, runlist_vram as u32);
        wr(base + 4, (runlist_vram >> 32) as u32);
        wr(base + 8, 2);
        let mut ok = false;
        for _ in 0..5_000_000u64 {
            if rd(base + 0xc) & 0x0000_8000 == 0 {
                ok = true;
                break;
            }
            core::hint::spin_loop();
        }
        (ok, runl_id)
    }

    pub fn fill_rect(&self, x: u32, y: u32, w: u32, h: u32, color: u32) {
        let width = self.info.width;
        let height = self.info.height;
        let x = x.min(width);
        let y = y.min(height);
        let w = w.min(width.saturating_sub(x));
        let h = h.min(height.saturating_sub(y));
        if w == 0 || h == 0 {
            return;
        }

        let ptr = self.info.fb_base_vaddr as *mut u32;
        let pitch_u32 = self.pitch_pixels();

        for py in 0..h {
            let row_start = (y + py) as usize * pitch_u32 + (x as usize);
            for px in 0..w {
                unsafe {
                    core::ptr::write_volatile(ptr.add(row_start + px as usize), color);
                }
            }
        }
    }

    pub fn blit_rect(&self, src_x: u32, src_y: u32, dst_x: u32, dst_y: u32, w: u32, h: u32) {
        let width = self.info.width;
        let height = self.info.height;
        let w = w
            .min(width.saturating_sub(src_x))
            .min(width.saturating_sub(dst_x));
        let h = h
            .min(height.saturating_sub(src_y))
            .min(height.saturating_sub(dst_y));
        if w == 0 || h == 0 {
            return;
        }

        let ptr = self.info.fb_base_vaddr as *mut u32;
        let pitch_u32 = self.pitch_pixels();

        let same_row_overlap = dst_y == src_y && dst_x > src_x && dst_x < src_x + w;
        let overlap_down = dst_y > src_y && dst_y < src_y + h;

        if same_row_overlap {
            for py in 0..h {
                let src_row = (src_y + py) as usize * pitch_u32 + (src_x as usize);
                let dst_row = (dst_y + py) as usize * pitch_u32 + (dst_x as usize);
                unsafe {
                    for i in (0..w as usize).rev() {
                        core::ptr::write(
                            ptr.add(dst_row + i),
                            core::ptr::read(ptr.add(src_row + i)),
                        );
                    }
                }
            }
        } else if overlap_down {
            for py in (0..h).rev() {
                let src_row = (src_y + py) as usize * pitch_u32 + (src_x as usize);
                let dst_row = (dst_y + py) as usize * pitch_u32 + (dst_x as usize);
                unsafe {
                    core::ptr::copy(ptr.add(src_row), ptr.add(dst_row), w as usize);
                }
            }
        } else {
            for py in 0..h {
                let src_row = (src_y + py) as usize * pitch_u32 + (src_x as usize);
                let dst_row = (dst_y + py) as usize * pitch_u32 + (dst_x as usize);
                unsafe {
                    core::ptr::copy(ptr.add(src_row), ptr.add(dst_row), w as usize);
                }
            }
        }
    }

    /// Live RM display state for this GPU: outputs, connected mask and EDID
    /// head, straight from the NV0073 query (per-instance cached on the C
    /// side, so repeated calls are cheap). `None` until this GPU's bring-up
    /// chain has run, or when the GPU has no display engine.
    fn rm_display_state(&self) -> Option<(u32, nvidia_rm_sys::rm_init::GrEdid)> {
        use core::sync::atomic::{AtomicBool, Ordering};
        let instance = (*self.rm_device_instance.lock())?;
        // The FIRST query runs the full RM/GSP control chain plus a DDC/EDID
        // probe (the C side then caches it for the rest of the boot). That can
        // take a long time and is not reentrant, so serialize opportunistically:
        // a caller that finds another query in flight reports "no RM topology"
        // and the DRM layer falls back to the synthetic connector, instead of
        // parking a second CPU behind it.
        static EDID_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
        if EDID_IN_FLIGHT.swap(true, Ordering::Acquire) {
            return None;
        }
        let d = nvidia_rm_sys::rm_init::edid(instance).ok();
        EDID_IN_FLIGHT.store(false, Ordering::Release);
        let d = d?;
        (d.supported_status == 0 && d.display_mask != 0).then_some((instance, d))
    }

    /// DRM connector id for output bit `bit` on RM instance `instance`.
    /// Both are < 32, so ids are unique across GPUs and never collide with
    /// the synthetic software-KMS ids (1..3) or the legacy fallback (1001).
    fn rm_connector_id(instance: u32, bit: u32) -> u32 {
        1001 + 100 * instance + bit
    }

    /// RM connector type (NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_*) for a
    /// single-bit displayId, from the cached GET_CONNECTOR_DATA sweep.
    fn rm_conn_type(d: &nvidia_rm_sys::rm_init::GrEdid, did: u32) -> Option<u32> {
        let n = (d.conn_type_count as usize).min(d.conn_type_display_id.len());
        (0..n)
            .find(|&i| d.conn_type_display_id[i] == did)
            .map(|i| d.conn_type[i])
    }
}

/// NV0073_CTRL_SPECIFIC_CONNECTOR_DATA_TYPE_* -> DRM_MODE_CONNECTOR_*.
fn nv_conn_type_to_drm(t: u32) -> u32 {
    match t {
        0x00 => 1,                              // VGA_15_PIN
        0x30 | 0x38 | 0x39 => 2,                // DVI_I / LFH_DVI_I_{1,2}
        0x31 => 3,                              // DVI_D
        0x46 | 0x47 | 0x49 | 0x64 | 0x65 => 10, // DP ext/int/serializer, LFH_DP
        0x48 => 10,                             // DP_MINI_EXT
        0x61 | 0x63 => 11,                      // HDMI_A / HDMI_C_MINI
        0x70 => 15,                             // VIRTUAL_WFD
        0x71 | 0x74 => 10,                      // USB_C (DP alt mode)
        0x72 => 16,                             // DSI
        _ => 0,                                 // Unknown
    }
}

/// Short human name for the /proc/gpuedid dump.
fn nv_conn_type_name(t: u32) -> &'static str {
    match t {
        0x00 => "VGA",
        0x30 | 0x38 | 0x39 => "DVI-I",
        0x31 => "DVI-D",
        0x46 | 0x47 | 0x49 | 0x64 | 0x65 => "DP",
        0x48 => "miniDP",
        0x61 => "HDMI",
        0x63 => "miniHDMI",
        0x70 => "virtual",
        0x71 | 0x74 => "USB-C",
        0x72 => "DSI",
        0xFFFF_FFFF => "?",
        _ => "other",
    }
}

#[allow(dead_code)] // used when deferred BAR0 MMIO probe is enabled
fn arch_from_pmc_boot0(boot0: u32) -> NvidiaArchitecture {
    let chip_id = (boot0 >> regs::PMC_BOOT0_CHIP_ID_SHIFT) & regs::PMC_BOOT0_CHIP_ID_MASK;
    if chip_id >= regs::PMC_BOOT0_CHIPID_BLACKWELL_MIN {
        NvidiaArchitecture::Blackwell
    } else if (regs::PMC_BOOT0_CHIPID_HOPPER_MIN..=regs::PMC_BOOT0_CHIPID_HOPPER_MAX)
        .contains(&chip_id)
    {
        NvidiaArchitecture::Hopper
    } else if (regs::PMC_BOOT0_CHIPID_ADA_MIN..=regs::PMC_BOOT0_CHIPID_ADA_MAX).contains(&chip_id) {
        NvidiaArchitecture::AdaLovelace
    } else if (regs::PMC_BOOT0_CHIPID_AMPERE_MIN..=regs::PMC_BOOT0_CHIPID_AMPERE_MAX)
        .contains(&chip_id)
    {
        NvidiaArchitecture::Ampere
    } else if (regs::PMC_BOOT0_CHIPID_TURING_MIN..=regs::PMC_BOOT0_CHIPID_TURING_MAX)
        .contains(&chip_id)
    {
        NvidiaArchitecture::Turing
    } else {
        NvidiaArchitecture::Unknown
    }
}

/// NV_PFAULT_FAULT_TYPE ([4:0] of INFO1) decode (Turing dev_fault.ref.txt).
fn fault_reason_name(r: u32) -> &'static str {
    match r {
        0 => "PDE",
        1 => "PDE_SIZE",
        2 => "PTE(unmapped)",
        3 => "VA_LIMIT",
        4 => "UNBOUND_INST",
        5 => "PRIV",
        6 => "RO",
        7 => "WO",
        0xa => "BAD_APERTURE",
        _ => "?",
    }
}

/// NV_PFAULT_ACCESS_TYPE ([19:16] of INFO1) decode.
fn fault_access_name(a: u32) -> &'static str {
    match a {
        0 => "READ",
        1 => "WRITE",
        2 => "ATOMIC",
        3 => "PREFETCH",
        8 => "PHYS_READ",
        9 => "PHYS_WRITE",
        0xa => "PHYS_ATOMIC",
        _ => "?",
    }
}

fn read_temperature(bar0: usize) -> Option<i32> {
    let raw =
        unsafe { core::ptr::read_volatile((bar0 + regs::NV_THERM_TEMP as usize) as *const u32) };
    if raw == 0 || raw == 0xFFFF_FFFF {
        return None;
    }
    let raw9 = raw & regs::NV_THERM_TEMP_VALUE_MASK;
    if (raw9 & regs::NV_THERM_TEMP_VALUE_SIGN_BIT) != 0 {
        Some((raw9 as i32) - 512)
    } else {
        Some(raw9 as i32)
    }
}

#[allow(dead_code)]
unsafe fn probe_resolution_from_bar0(bar0: usize) -> Option<(u32, u32)> {
    let reg =
        core::ptr::read_volatile((bar0 + regs::NV50_HEAD0_RASTER_SIZE as usize) as *const u32);
    let (w, h) = (reg & 0xFFFF, reg >> 16);
    if w > 0 && h > 0 && w <= 16384 && h <= 16384 {
        return Some((w, h));
    }

    let reg = core::ptr::read_volatile((bar0 + regs::NV40_PCRTC_HEAD0_SIZE as usize) as *const u32);
    let (w, h) = (reg & 0xFFFF, reg >> 16);
    if w > 0 && h > 0 && w <= 16384 && h <= 16384 {
        return Some((w, h));
    }
    None
}

/// Identify GPU based on PCI device ID.
/// Returns (architecture, name, memory_mb).
fn identify_gpu(device_id: u16) -> (NvidiaArchitecture, &'static str, u32) {
    match device_id {
        // Blackwell
        0x2B85 => (NvidiaArchitecture::Blackwell, "GeForce RTX 5090", 32768),
        0x2B89 => (NvidiaArchitecture::Blackwell, "GeForce RTX 5080", 16384),
        0x2C00 => (NvidiaArchitecture::Blackwell, "GeForce RTX 5070 Ti", 16384),
        0x2C20 => (NvidiaArchitecture::Blackwell, "GeForce RTX 5070", 12288),

        // Ada Lovelace
        0x2684 => (NvidiaArchitecture::AdaLovelace, "GeForce RTX 4090", 24576),
        0x2704 => (NvidiaArchitecture::AdaLovelace, "GeForce RTX 4080", 16384),
        0x2782 => (
            NvidiaArchitecture::AdaLovelace,
            "GeForce RTX 4070 Ti",
            12288,
        ),
        0x2786 => (NvidiaArchitecture::AdaLovelace, "GeForce RTX 4070", 12288),
        0x2803 => (NvidiaArchitecture::AdaLovelace, "GeForce RTX 4060 Ti", 8192),
        0x2882 => (NvidiaArchitecture::AdaLovelace, "GeForce RTX 4060", 8192),

        // Ampere
        0x2204 => (NvidiaArchitecture::Ampere, "GeForce RTX 3090", 24576),
        0x2206 => (NvidiaArchitecture::Ampere, "GeForce RTX 3080", 10240),
        0x2484 => (NvidiaArchitecture::Ampere, "GeForce RTX 3070", 8192),
        0x2489 => (NvidiaArchitecture::Ampere, "GeForce RTX 3060 Ti", 8192),
        0x2503 => (NvidiaArchitecture::Ampere, "GeForce RTX 3060", 12288),
        0x2571 => (NvidiaArchitecture::Ampere, "GeForce RTX 3050", 8192),

        // Turing
        0x1E02 => (NvidiaArchitecture::Turing, "GeForce RTX 2080 Ti", 11264),
        0x1E04 => (NvidiaArchitecture::Turing, "GeForce RTX 2080 Super", 8192),
        0x1E07 => (NvidiaArchitecture::Turing, "GeForce RTX 2080", 8192),
        0x1E82 => (NvidiaArchitecture::Turing, "GeForce RTX 2070 Super", 8192),
        0x1E84 => (NvidiaArchitecture::Turing, "GeForce RTX 2070", 8192),
        0x1F02 | 0x1F06 | 0x1F07 => (NvidiaArchitecture::Turing, "GeForce RTX 2060 Super", 8192),
        0x1F03 | 0x1F08 | 0x1F0A | 0x1F0B => (NvidiaArchitecture::Turing, "GeForce RTX 2060", 6144),
        0x1F36 => (NvidiaArchitecture::Turing, "GeForce GTX 1660 Super", 6144),
        0x1F82 => (NvidiaArchitecture::Turing, "GeForce GTX 1660", 6144),
        0x1F91 => (NvidiaArchitecture::Turing, "GeForce GTX 1650 Super", 4096),
        0x1F99 => (NvidiaArchitecture::Turing, "GeForce GTX 1650", 4096),

        _ => (NvidiaArchitecture::Unknown, "Unknown NVIDIA GPU", 0),
    }
}

impl Scheme for NvidiaGpu {
    fn name(&self) -> &str {
        &self.name
    }
    fn handle_irq(&self, _irq_num: usize) {}
}

impl DisplayScheme for NvidiaGpu {
    fn info(&self) -> DisplayInfo {
        self.info
    }
    fn fb(&self) -> FrameBuffer<'_> {
        unsafe {
            FrameBuffer::from_raw_parts_mut(self.info.fb_base_vaddr as *mut u8, self.info.fb_size)
        }
    }

    /// The framebuffer is the GPU's own VRAM, mapped through the PCI BAR. The
    /// generic 2D primitives (`fill_rect` / `copy_rect` / `blit_from`) therefore
    /// write straight into video memory in bulk — already far cheaper than the
    /// per-pixel MMIO path — so we advertise them as accelerated. (A future step
    /// would offload these to the GPU's own copy engine via command channels.)
    fn accel_caps(&self) -> AccelCaps {
        AccelCaps {
            fill: true,
            copy: true,
            blit: true,
        }
    }
}

impl DrmScheme for NvidiaGpu {
    fn pci_bdf(&self) -> Option<(u32, u8, u8, u8)> {
        // RM only ever drives function 0 of the GPU (see `cfg_loc`).
        Some((self.pci_domain, self.pci_bus, self.pci_device, 0))
    }

    fn is_console_gpu(&self) -> bool {
        self.drives_boot_display()
    }

    /// Receives `gsp.bin` read from the mounted rootfs by `zCore`'s boot
    /// code (see `zCore/src/main.rs`, right after rootfs mount) -- stored
    /// for the real `kgspInitRm` call made lazily on the first
    /// `/proc/gpudbg` read, same trigger as the RM attach itself.
    fn set_gsp_firmware(&self, bytes: Vec<u8>) {
        *self.gsp_firmware.lock() = Some(bytes);
    }

    fn set_gsp_firmware_status(&self, status: String) {
        *self.gsp_fw_status.lock() = Some(status);
    }

    /// Read-only GPU state dump (surfaced at `/proc/gpudbg`). Step 1 of the GPU
    /// copy-engine bring-up: confirm MMIO works bidirectionally, identify the
    /// exact chip, and record the VRAM/BAR layout we need for channel structs.
    /// All reads, no writes — safe to run on demand post-boot. With two GPUs
    /// this runs once per NvidiaGpu; `name` (PCI bus:dev.fn) tells them apart,
    /// and a matching BAR1/fb_vaddr marks the one actually driving the display.
    fn debug_dump(&self) -> String {
        use core::fmt::Write;
        let bar0 = self._bar0;
        let rd =
            |off: u32| unsafe { core::ptr::read_volatile((bar0 + off as usize) as *const u32) };
        // NV_PMC_BOOT_0: architecture/chipset id. NV_PCFG mirror at BAR0+0x88000
        // exposes PCI config dword 0 (vendor | device<<16) — reading 0x10de here
        // proves MMIO is alive. (Offsets per nouveau nvkm.)
        let boot0 = rd(regs::NV_PMC_BOOT_0);
        let chipset = (boot0 >> 20) & 0x1ff;
        let pcfg = rd(0x8_8000);
        let cstatus = rd(regs::NV_PFB_CSTATUS);
        let mut s = String::new();
        let _ = writeln!(s, "[gpudbg] === {} ({}) ===", self.name, self.gpu_model);
        // nvidia-rm-sys bring-up: first real-hardware exercise of the C-compile
        // + FFI-link pipeline that will host vendored NVIDIA open-gpu-kernel-
        // modules source. Not NVIDIA code yet -- see nvidia-rm-sys/build.rs.
        // A prior isolated (non-workspace) build already confirmed the object
        // code and cross-language linkage are correct; this is the first time
        // it runs inside the actual kernel binary/linker script/panic handler.
        let (nvrm_result, nvrm_logged) = nvidia_rm_sys::smoke_test(17, 25);
        let _ = writeln!(
            s,
            "[gpudbg]  nvrm-sys smoke test: C-add(17,25)={} C->Rust-callback-saw={} (both should be 42)",
            nvrm_result, nvrm_logged
        );
        // First REAL vendored NVIDIA C (src/nvidia/src/libraries/fnv_hash/
        // fnv_hash.c, MIT) exercised on real hardware, not the hand-written
        // smoke test above. fnv1Hash64 on an empty slice can't touch the
        // hash loop at all (zero-length buffer), so it must return the raw
        // FNV-1 64-bit offset basis unchanged: 0xcbf29ce484222325. Any other
        // value means either the wrong function ran or something is broken
        // in the real NVIDIA source path, not something we wrote.
        let nvrm_fnv_empty = nvidia_rm_sys::fnv_hash::fnv1_hash64(&[]);
        let nvrm_fnv_hello = nvidia_rm_sys::fnv_hash::fnv1_hash64(b"hello");
        let _ = writeln!(
            s,
            "[gpudbg]  nvrm-sys REAL NVIDIA fnv1Hash64(\"\")={:#018x} (expect 0xcbf29ce484222325) fnv1Hash64(\"hello\")={:#018x}",
            nvrm_fnv_empty, nvrm_fnv_hello
        );
        let _ = writeln!(
            s,
            "[gpudbg]  arch={:?} BAR0={:#x} BAR1/fb_vaddr={:#x} fb_size={:#x} VRAM={}MB",
            self.architecture, bar0, self._bar1, self.info.fb_size, self.vram_size_mb
        );
        let _ = writeln!(
            s,
            "[gpudbg]  PMC_BOOT_0(0x0)={:#010x} -> chipset=0x{:03x}",
            boot0, chipset
        );
        let _ = writeln!(
            s,
            "[gpudbg]  PCFG(0x88000)={:#010x} vendor={:#06x} device={:#06x}",
            pcfg,
            pcfg & 0xffff,
            pcfg >> 16
        );
        let _ = writeln!(
            s,
            "[gpudbg]  PFB_CSTATUS(0x10020c)={:#010x} drives_console={}",
            cstatus,
            self.drives_boot_display()
        );

        // --- Step 0: FIFO / MMU status (read-only "hang oracle") ---
        // Confirms which runlist owns the copy engine and that no MMU fault is
        // latched at boot, BEFORE any risky write. All reads. Offsets per
        // nouveau tu102 (vfn/fifo/mmu). A PRI-error sentinel (0xbadfxxxx) here
        // just means the engine block is in reset — still harmless to read.
        let doorbell_en = rd(0x00b6_5000);
        let _ = writeln!(s, "[gpudbg]  --- FIFO/MMU (Step 0, read-only) ---");
        let _ = writeln!(
            s,
            "[gpudbg]  DOORBELL_EN(0xb65000)={:#010x} (bit31={})",
            doorbell_en,
            doorbell_en >> 31
        );
        for rl in 0..2u32 {
            let base = 0x0000_2b00 + rl * 0x10;
            let _ = writeln!(
                s,
                "[gpudbg]  RUNL{} base_lo(0x{:x})={:#010x} base_hi={:#010x} submit={:#010x} cfg(0x{:x})={:#010x}",
                rl,
                base,
                rd(base),
                rd(base + 4),
                rd(base + 8),
                base + 0xc,
                rd(base + 0xc)
            );
        }
        // RUNL0/1 above are only ever the console/GR runlists on this chip —
        // a real-hardware run discovered the CE's actual runlist is 8 (not
        // 0/1), so its own commit/submit registers had never been shown here.
        // find_ce_runlist is a read-only PTOP scan; safe in this always-on dump.
        if let Some((ce_rl, _)) = self.find_ce_runlist() {
            if ce_rl >= 2 {
                let base = 0x0000_2b00 + ce_rl * 0x10;
                let _ = writeln!(
                    s,
                    "[gpudbg]  RUNL{}(CE) base_lo(0x{:x})={:#010x} base_hi={:#010x} submit={:#010x} cfg(0x{:x})={:#010x}",
                    ce_rl,
                    base,
                    rd(base),
                    rd(base + 4),
                    rd(base + 8),
                    base + 0xc,
                    rd(base + 0xc)
                );
            }
        }
        let _ = writeln!(s, "[gpudbg]  CHAN0_CFG(0x800004)={:#010x}", rd(0x0080_0004));
        let _ = writeln!(
            s,
            "[gpudbg]  MMU flush PDB(0xb830a0)={:#010x} hi(0xb830a4)={:#010x} trigger(0xb830b0)={:#010x}",
            rd(0x00b8_30a0),
            rd(0x00b8_30a4),
            rd(0x00b8_30b0)
        );

        // --- MMU fault snapshot (Turing tu102: 0xb83080..0xb83094, read-only) ---
        // These latch the most recent non-replayable fault. We never write the
        // clear reg (0xb83094) so the fault stays pinned for inspection.
        let f_info1 = rd(0x00b8_3090);
        let _ = writeln!(s, "[gpudbg]  --- MMU fault snapshot (read-only) ---");
        let _ = writeln!(
            s,
            "[gpudbg]  FAULT_INFO1(0xb83090)={:#010x} valid={} hub={} access={}({}) client={:#x} reason={}({})",
            f_info1,
            f_info1 >> 31,
            (f_info1 >> 20) & 1,
            (f_info1 >> 16) & 0xf,
            fault_access_name((f_info1 >> 16) & 0xf),
            (f_info1 >> 8) & 0x7f,
            f_info1 & 0x1f,
            fault_reason_name(f_info1 & 0x1f),
        );
        if f_info1 & 0x8000_0000 != 0 {
            let addr_lo = rd(0x00b8_3080);
            let addr_hi = rd(0x00b8_3084);
            let info0 = rd(0x00b8_3088);
            let inst_hi = rd(0x00b8_308c);
            let _ = writeln!(
                s,
                "[gpudbg]  FAULT_VA={:#x}{:08x} engine_id={:#x} inst={:#x}{:08x}",
                addr_hi,
                addr_lo & 0xffff_f000,
                info0 & 0xff,
                inst_hi,
                info0 & 0xffff_f000,
            );
        }

        // --- Per-channel (PCCSR) + per-PBDMA status (read-only) ---
        let pccsr = rd(0x0080_0004);
        let _ = writeln!(
            s,
            "[gpudbg]  PCCSR0(0x800004)={:#010x} enable={} busy={} status={} pbdma_faulted={} eng_faulted={}",
            pccsr,
            pccsr & 1,
            (pccsr >> 28) & 1,
            (pccsr >> 24) & 0xf,
            (pccsr >> 22) & 1,
            (pccsr >> 23) & 1,
        );
        let _ = writeln!(
            s,
            "[gpudbg]  PCCSR0_INST(0x800000)={:#010x}",
            rd(0x0080_0000)
        );
        for i in 0..2u32 {
            let pb = 0x0004_0000 + i * 0x2000;
            let _ = writeln!(
                s,
                "[gpudbg]  PBDMA{} STATUS(0x{:x})={:#010x} CHANNEL={:#010x} GP_GET={:#010x} GP_PUT={:#010x} GET={:#010x} INTR_0={:#010x}",
                i,
                pb + 0x100,
                rd(pb + 0x100),
                rd(pb + 0x120),
                rd(pb + 0x14),
                rd(pb),
                rd(pb + 0x18),
                rd(pb + 0x108),
            );
        }
        // PBDMA0/1 above are not necessarily the PBDMA(s) that serve the CE's
        // runlist (discovered as PBDMA9 on the last real-hardware run). Dump
        // whichever PBDMA(s) NV_PFIFO_PBDMA_MAP actually routes the CE's
        // runlist to, so a stuck/never-armed PBDMA is visible without needing
        // the opt-in /proc/gpustep4.
        if let Some((ce_rl, _)) = self.find_ce_runlist() {
            for i in 0..12u32 {
                if i < 2 {
                    continue; // already shown above
                }
                let map = rd(0x0000_2390 + i * 4) & 0xffff;
                if map & (1 << ce_rl) == 0 {
                    continue;
                }
                let pb = 0x0004_0000 + i * 0x2000;
                let _ = writeln!(
                    s,
                    "[gpudbg]  PBDMA{}(serves CE runl{}) STATUS(0x{:x})={:#010x} CHANNEL={:#010x} GP_GET={:#010x} GP_PUT={:#010x} GET={:#010x} INTR_0={:#010x}",
                    i,
                    ce_rl,
                    pb + 0x100,
                    rd(pb + 0x100),
                    rd(pb + 0x120),
                    rd(pb + 0x14),
                    rd(pb),
                    rd(pb + 0x18),
                    rd(pb + 0x108),
                );
            }
        }

        // --- Engine -> runlist map (NV_PTOP_DEVICE_INFO 0x022700, read-only) ---
        // Walk the device-info table; dump non-zero raw entries so we can decode
        // which runlist owns the copy engines.
        let _ = writeln!(s, "[gpudbg]  --- PTOP device-info (0x022700, non-zero) ---");
        for i in 0..64u32 {
            let e = rd(0x0002_2700 + i * 4);
            if e != 0 {
                let _ = writeln!(s, "[gpudbg]  DEVINFO[{:2}]={:#010x}", i, e);
            }
        }

        // --- Step 1: build the GMMU tables in RAM and dump them (no GPU writes) ---
        {
            let mut g = self.bringup.lock();
            if g.is_none() {
                // GPU VA base for the packed 2 MiB region (avoids null-VA).
                *g = GpuBringup::build(0x0020_0000, 0x0300_0000);
            }
            match g.as_ref() {
                Some(b) => s.push_str(&b.dump()),
                None => {
                    let _ = writeln!(
                        s,
                        "[gpudbg]  GMMU: alloc_coherent FAILED (DMA pool exhausted)"
                    );
                }
            }
        }

        s
    }

    /// Step 5 (`/proc/gpustep5`), NOT read-only and NOT part of `/proc/gpudbg`:
    /// first real invocation of the vendored RM core's own object
    /// construction (`nvidia_rm_sys::rm_init`, OBJSYS/resource-server/OBJGPU
    /// via NVOC). Moved out of `debug_dump` after it hung the machine on a
    /// plain `cat /proc/gpudbg` on real hardware -- this does real HAL
    /// bind/attach work, not a safe register read, so it gets its own
    /// deliberate opt-in trigger like bringup_step2/3/4. Cached after the
    /// first attempt so repeated reads don't re-run it.
    fn bringup_step5(&self) -> String {
        use core::fmt::Write;
        // TEMPORARY: absolute-first-line checkpoint, using the exact same
        // log::warn! mechanism already proven visible at driver-init time
        // ("[NVIDIA] GPU at ..."), bypassing nv_printf/C entirely -- two
        // real-hardware tests in a row (with confirmed-fresh binaries)
        // produced zero output even after fixing the info->warn level
        // bug, so this determines whether the function is even entered/
        // whether ANY print is visible from this exact call context
        // before reaching the lock or any real RM code.
        log::warn!("[NVIDIA] bringup_step5: entered");
        let bar0 = self._bar0;
        log::warn!("[NVIDIA] bringup_step5: read self._bar0 = {:#x}", { bar0 });
        let mut s = String::new();
        {
            // TEMPORARY chip-ID probe: read PMC_BOOT_0 (offset 0) and
            // PMC_BOOT_42 (offset 0xA00) directly through our mapped BAR0,
            // the exact registers RM's gpumgrGetGpuHalFactor reads to
            // identify the chip. gpumgrAttachGpu now returns 0x56
            // (NV_ERR_NOT_SUPPORTED) -- which is exactly what
            // halmgrGetHalForGpu returns when the chip ID matches no known
            // HAL, so the leading theory is our BAR0 reads don't return the
            // real chip ID. For a TU106 the real values are: PMC_BOOT_42
            // bits 29:24 (ARCHITECTURE) == 0x16 and the IMPLEMENTATION
            // nibble (bits 23:20) == 6. 0x0 or 0xFFFFFFFF here means BAR0
            // MMIO is not actually reaching the GPU (mapping/decode wrong),
            // which is the whole ballgame. Written into the RETURNED string
            // (not just log::warn) so it survives the RM init log spew and
            // is always visible in the `cat` output.
            let boot0 = unsafe { core::ptr::read_volatile(bar0 as *const u32) };
            let boot42 = unsafe { core::ptr::read_volatile((bar0 + 0xA00) as *const u32) };
            // PMC_BOOT_1 @ 0x4: gpuDetermineVirtualMode (gpu.c:4552) asserts
            // that the VGPU field (bits 17:16) read at attach time matches
            // the value read later through the IoAperture; a mismatch is the
            // 0x40 (NV_ERR_INVALID_STATE). _VF==0x2, _PV==0x1, _REAL==0x0;
            // a bare-metal PF TU106 must read _REAL (0x0) in bits 17:16.
            let boot1 = unsafe { core::ptr::read_volatile((bar0 + 0x4) as *const u32) };
            let arch = (boot42 >> 24) & 0x3F;
            let impl_ = (boot42 >> 20) & 0xF;
            let vgpu = (boot1 >> 16) & 0x3;
            let _ = writeln!(
                s,
                "[gpustep5]  BAR0 chip-ID probe: PMC_BOOT_0={:#010x} PMC_BOOT_42={:#010x} \
                 (arch={:#x} impl={:#x}; TU106 expects arch=0x16 impl=0x6)",
                boot0, boot42, arch, impl_
            );
            let _ = writeln!(
                s,
                "[gpustep5]  PMC_BOOT_1={:#010x} VGPU(bits17:16)={:#x} \
                 (0=REAL/bare-metal, 1=PV, 2=VF; bare-metal PF must be 0)",
                boot1, vgpu
            );
            log::warn!(
                "[NVIDIA] bringup_step5: BAR0 probe: PMC_BOOT_0={:#010x} \
                 PMC_BOOT_42={:#010x} PMC_BOOT_1={:#010x} (arch={:#x} impl={:#x} vgpu={:#x})",
                boot0,
                boot42,
                boot1,
                arch,
                impl_,
                vgpu
            );
        }

        // The /proc read is served by seq_read_at, which re-invokes this
        // generator for EVERY chunk `cat` requests. So the returned String
        // must be byte-for-byte identical across calls: cat's first read
        // (offset 0) runs the attach and yields the full string incl.
        // narration; its second read (offset = first-chunk length) calls us
        // again. If that second string is a different length -- which it was
        // when the narration only got appended on the non-cached path -- the
        // offset lands past its end, read returns 0/EOF, and the output is
        // truncated mid-line (exactly what hid the RM narration and the
        // real result last run). The BAR0 probe above is deterministic; the
        // attach + narration is not (runs once, then cached), so cache the
        // ENTIRE post-probe block -- narration and result line together --
        // and emit it verbatim on every call.
        log::warn!("[NVIDIA] bringup_step5: checking cached result");
        let cached = self.rm_attach_result.lock().clone();
        log::warn!(
            "[NVIDIA] bringup_step5: cache check done, cached={}",
            cached.is_some()
        );

        let block = if let Some(cached) = cached {
            cached
        } else {
            // Capture the RM's own nv_printf / assert / ECLIPSE_TRACE
            // narration into an in-memory buffer for the duration of core
            // init + attach. On this bring-up box the kernel `log::warn!`
            // stream never reaches the monitor -- only this returned String
            // (the `cat /proc/gpustep5` stdout) does -- so folding the RM's
            // narration in here is the only way it's actually visible. The
            // RmMsg rule set in eclipse_rm_init_core makes gpu.c/gpu_mgr.c
            // narrate every step, so the last captured line pins where a
            // graceful failure (e.g. 0x40) originates inside gpumgrAttachGpu.
            nvidia_rm_sys::os_interface::capture_begin();
            let core_status = rm_core_init_once();
            let computed = if core_status != 0 {
                alloc::format!("eclipse_rm_init_core FAILED, NV_STATUS={:#x}", core_status)
            } else {
                match nvidia_rm_sys::rm_init::attach_gpu(
                    self.pci_domain,
                    self.pci_bus,
                    self.pci_device,
                    self.bar0_phys,
                    bar0 as *mut core::ffi::c_void,
                    self.bar0_len,
                    self.bar1_phys,
                    self.vram_size_mb as u64 * 1024 * 1024,
                    self.bar2_phys,
                    self.bar2_len,
                ) {
                    Ok(device_instance) => {
                        *self.rm_device_instance.lock() = Some(device_instance);
                        alloc::format!("gpumgrAttachGpu OK, deviceInstance={}", device_instance)
                    }
                    Err(status) => {
                        alloc::format!("gpumgrAttachGpu FAILED, NV_STATUS={:#x}", status)
                    }
                }
            };
            let captured = nvidia_rm_sys::os_interface::capture_take();
            // Build the full post-probe block: captured RM narration first
            // (each line prefixed for the `cat` reader), then the result.
            let mut block = String::new();
            if let Some(log) = captured {
                if !log.is_empty() {
                    let _ = writeln!(block, "[gpustep5]  --- RM narration (captured) ---");
                    for line in log.lines() {
                        let _ = writeln!(block, "[gpustep5]  | {}", line);
                    }
                    let _ = writeln!(block, "[gpustep5]  --- end RM narration ---");
                }
            }
            let _ = writeln!(block, "[gpustep5]  --- Real RM attach: {} ---", computed);
            // Publish; harmless if two callers race here (single-shell
            // manual testing only today) since both compute the same block
            // and either write wins.
            let mut attach = self.rm_attach_result.lock();
            if attach.is_none() {
                *attach = Some(block.clone());
            }
            block
        };

        s.push_str(&block);
        s
    }

    /// Step 7: read back the `GspStaticConfigInfo` the live GSP-RM returned
    /// during step 6's GET_GSP_STATIC_INFO RPC. Pure readback -- no RPCs, no
    /// register writes -- so it is safe to run any number of times. All-zero
    /// name means step 6 has not completed on this GPU.
    fn bringup_step7(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let device_instance = *self.rm_device_instance.lock();
        let Some(device_instance) = device_instance else {
            return String::from("[gpustep7]  skipped (run /proc/gpustep5 (RM attach) first)\n");
        };
        match nvidia_rm_sys::rm_init::get_gsp_info(device_instance) {
            Ok(info) => {
                let name_len = info.gpu_name.iter().position(|&b| b == 0).unwrap_or(64);
                let short_len = info
                    .gpu_short_name
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(64);
                let name = core::str::from_utf8(&info.gpu_name[..name_len]).unwrap_or("<non-utf8>");
                let short =
                    core::str::from_utf8(&info.gpu_short_name[..short_len]).unwrap_or("<non-utf8>");
                if name.is_empty() {
                    let _ = writeln!(
                        s,
                        "[gpustep7]  GSP static info is all zeros -- GSP-RM not booted on this GPU yet (run /proc/gpustep6)"
                    );
                } else {
                    let _ = writeln!(s, "[gpustep7]  --- Firmware-reported GPU info (from live GSP-RM via GET_GSP_STATIC_INFO) ---");
                    let _ = writeln!(s, "[gpustep7]  GPU name:   {}", name);
                    let _ = writeln!(s, "[gpustep7]  Short name: {}", short);
                    let _ = writeln!(
                        s,
                        "[gpustep7]  VRAM:       {} MiB ({} bytes), bus width {} bits, ram type {}",
                        info.fb_length / (1024 * 1024),
                        info.fb_length,
                        info.fb_bus_width,
                        info.fb_ram_type
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep7]  L2 cache:   {} KiB",
                        info.l2_cache_size / 1024
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep7]  VBIOS:      valid={} subvendor={:#06x} subdevice={:#06x}",
                        info.vbios_valid != 0,
                        info.vbios_sub_vendor,
                        info.vbios_sub_device
                    );
                }
            }
            Err(status) => {
                let _ = writeln!(
                    s,
                    "[gpustep7]  eclipse_rm_get_gsp_info FAILED, NV_STATUS={:#x}",
                    status
                );
            }
        }
        s
    }

    /// Step 8: three read-only RM API controls answered by the live GSP-RM's
    /// resource server (GSP_RM_CONTROL RPC): GPU name, GID/UUID, FB heap
    /// total/free. heap_free is dynamic firmware bookkeeping -- proof of a
    /// live, working RM API path end-to-end. Safe to run repeatedly.
    fn bringup_step8(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let device_instance = *self.rm_device_instance.lock();
        let Some(device_instance) = device_instance else {
            return String::from("[gpustep8]  skipped (run /proc/gpustep5 (RM attach) first)\n");
        };
        nvidia_rm_sys::os_interface::capture_begin();
        let result = nvidia_rm_sys::rm_init::rm_api_demo(device_instance);
        let captured = nvidia_rm_sys::os_interface::capture_take();
        if let Some(log) = captured {
            for line in log.lines() {
                let _ = writeln!(s, "[gpustep8]  | {}", line);
            }
        }
        match result {
            Ok(demo) => {
                let _ = writeln!(s, "[gpustep8]  --- RM API controls served by live GSP-RM (GSP_RM_CONTROL RPC) ---");
                if demo.name_status == 0 {
                    let n = demo.name.iter().position(|&b| b == 0).unwrap_or(64);
                    let _ = writeln!(
                        s,
                        "[gpustep8]  GET_NAME_STRING: {}",
                        core::str::from_utf8(&demo.name[..n]).unwrap_or("<non-utf8>")
                    );
                } else {
                    let _ = writeln!(
                        s,
                        "[gpustep8]  GET_NAME_STRING: NV_STATUS={:#x}",
                        demo.name_status
                    );
                }
                if demo.gid_status == 0 {
                    let n = (demo.gid_length as usize).min(demo.gid.len());
                    let _ = writeln!(
                        s,
                        "[gpustep8]  GET_GID_INFO (UUID): {}",
                        core::str::from_utf8(&demo.gid[..n]).unwrap_or("<non-utf8>")
                    );
                } else {
                    let _ = writeln!(
                        s,
                        "[gpustep8]  GET_GID_INFO: NV_STATUS={:#x}",
                        demo.gid_status
                    );
                }
                if demo.fb_status == 0 {
                    let _ = writeln!(
                        s,
                        "[gpustep8]  FB_GET_INFO_V2: heap {} MiB total, {} MiB free, bus width {} bits",
                        demo.heap_size_kb / 1024,
                        demo.heap_free_kb / 1024,
                        demo.bus_width
                    );
                } else {
                    let _ = writeln!(
                        s,
                        "[gpustep8]  FB_GET_INFO_V2: NV_STATUS={:#x}",
                        demo.fb_status
                    );
                }
            }
            Err(status) => {
                let _ = writeln!(
                    s,
                    "[gpustep8]  eclipse_rm_step8 FAILED, NV_STATUS={:#x} (GSP not booted? run /proc/gpustep6)",
                    status
                );
            }
        }
        s
    }

    /// Step 9: gpuStatePreInit + gpuStateInit + gpuStateLoad -- the rest of
    /// the real RmInitAdapter device bring-up, run against the live GSP.
    /// One-shot per boot (the RM state machine is not re-runnable), so the
    /// whole block (captured narration + per-phase result) is cached and
    /// re-served on subsequent reads, like step 6.
    fn bringup_step9(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let device_instance = *self.rm_device_instance.lock();
        let Some(device_instance) = device_instance else {
            return String::from("[gpustep9]  skipped (run /proc/gpustep5 (RM attach) first)\n");
        };
        let cached = self.state_init_result.lock().clone();
        let block = if let Some(cached) = cached {
            cached
        } else {
            nvidia_rm_sys::os_interface::capture_begin();
            // Live-echo state-init narration at ERROR level so it survives the
            // default LOG=error filter and lands on the console AS IT RUNS.
            // gpuStateInit currently faults on real hardware (NULL write at
            // vaddr 0x90); the captured buffer is only folded into this /proc
            // read on a clean return, so without live echo a fault prints
            // nothing and we can't see which engine died. The last live
            // "[nvidia-rm] ..." line before the panic pinpoints it.
            nvidia_rm_sys::os_interface::live_echo_begin();
            let result = nvidia_rm_sys::rm_init::state_init(device_instance);
            nvidia_rm_sys::os_interface::live_echo_end();
            let captured = nvidia_rm_sys::os_interface::capture_take();
            let mut block = String::new();
            if let Some(log) = captured {
                if !log.is_empty() {
                    let _ = writeln!(block, "[gpustep9]  --- state-init narration (captured) ---");
                    for line in log.lines() {
                        let _ = writeln!(block, "[gpustep9]  | {}", line);
                    }
                    let _ = writeln!(block, "[gpustep9]  --- end narration ---");
                }
            }
            let early_err = result.is_err();
            match result {
                Ok(r) => {
                    let phase = |st: u32| -> String {
                        match st {
                            0 => String::from("OK"),
                            0xFFFF_FFFF => String::from("not reached"),
                            e => alloc::format!("FAILED NV_STATUS={:#x}", e),
                        }
                    };
                    let _ = writeln!(
                        block,
                        "[gpustep9]  gpuStatePreInit: {}",
                        phase(r.pre_init_status)
                    );
                    let _ = writeln!(
                        block,
                        "[gpustep9]  gpuStateInit:    {}",
                        phase(r.init_status)
                    );
                    let _ = writeln!(
                        block,
                        "[gpustep9]  gpuStateLoad:    {}",
                        phase(r.load_status)
                    );
                    if r.pre_init_status == 0 && r.init_status == 0 && r.load_status == 0 {
                        let _ = writeln!(block, "[gpustep9]  --- FULL RmInitAdapter-equivalent bring-up COMPLETE: GPU is state-loaded ---");
                    }
                }
                Err(status) => {
                    let _ = writeln!(
                        block,
                        "[gpustep9]  eclipse_rm_state_init FAILED, NV_STATUS={:#x} (GSP not booted? run /proc/gpustep6)",
                        status
                    );
                }
            }
            // Cache only when the C call actually ran (Ok). An early Err --
            // e.g. "GSP not booted" from gpuinit's pass over the console GPU
            // -- has no RM side effects, and caching it shadowed the real
            // step14 attempt later in the same boot (r16 run: stage 4
            // replayed gpuinit's 0x40 even though step8 had just proven the
            // GSP live).
            if !early_err {
                let mut cache = self.state_init_result.lock();
                if cache.is_none() {
                    *cache = Some(block.clone());
                }
            }
            block
        };
        s.push_str(&block);
        s
    }

    /// Step 10 (`/proc/gpustep10`): first real DATA MOVEMENT through the
    /// copy engine on the state-loaded GPU -- CE memset of a pattern into
    /// vidmem buffer A (and a poison into B), CE copy A->B, then CPU
    /// readback of B through BAR2 verifying every dword. Uses the RM's own
    /// internal CeUtils channel (the VRAM scrubber's machinery), driving the
    /// exact doorbell path the step-9 osMapGPU fix repaired. Requires a
    /// successful step 9 first (gpuStateLoad OK).
    fn bringup_step10(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        // The console-GPU guard is gone: the SEC2-resume wedge that once
        // blocked its GSP boot is fixed (gsp_boot_run's console drain), so the
        // primary can be state-loaded and run the CE test like the secondary.
        let device_instance = *self.rm_device_instance.lock();
        let Some(device_instance) = device_instance else {
            return String::from(
                "[gpustep10] skipped (run steps 5/6/9 first: attach, GSP boot, state init)\n",
            );
        };
        let cached = self.step10_result.lock().clone();
        let block = if let Some(cached) = cached {
            cached
        } else {
            nvidia_rm_sys::os_interface::capture_begin();
            // Live-echo like step 9: CE submission exercises channel +
            // doorbell paths for the first time; if anything faults, the
            // last live line names the phase.
            nvidia_rm_sys::os_interface::live_echo_begin();
            let result = nvidia_rm_sys::rm_init::step10(device_instance);
            nvidia_rm_sys::os_interface::live_echo_end();
            let captured = nvidia_rm_sys::os_interface::capture_take();
            let mut block = String::new();
            if let Some(log) = captured {
                if !log.is_empty() {
                    let _ = writeln!(block, "[gpustep10] --- CE-test narration (captured) ---");
                    for line in log.lines() {
                        let _ = writeln!(block, "[gpustep10] | {}", line);
                    }
                    let _ = writeln!(block, "[gpustep10] --- end narration ---");
                }
            }
            let early_err = result.is_err();
            match result {
                Ok(r) => {
                    let phase = |st: u32| -> String {
                        match st {
                            0 => String::from("OK"),
                            0xFFFF_FFFF => String::from("not reached"),
                            e => alloc::format!("FAILED NV_STATUS={:#x}", e),
                        }
                    };
                    let _ = writeln!(
                        block,
                        "[gpustep10] buffers: {} KiB each, A PA={:#x} B PA={:#x} (VRAM)",
                        r.buffer_size / 1024,
                        r.pa_a,
                        r.pa_b
                    );
                    let _ = writeln!(
                        block,
                        "[gpustep10] CeUtils channel:   {}",
                        phase(r.ce_utils_status)
                    );
                    let _ = writeln!(
                        block,
                        "[gpustep10] alloc A:           {}",
                        phase(r.alloc_a_status)
                    );
                    let _ = writeln!(
                        block,
                        "[gpustep10] alloc B:           {}",
                        phase(r.alloc_b_status)
                    );
                    // CE memset writes only the pattern's LOW BYTE replicated
                    // (SET_REMAP_COMPONENTS _COMPONENT_SIZE_ONE,
                    // channel_utils.c) -- spot checks in C already account
                    // for that; show the byte the hardware actually wrote.
                    let _ = writeln!(
                        block,
                        "[gpustep10] CE memset B (byte {:#04x}) + spot-check: {}",
                        r.poison & 0xFF,
                        phase(r.poison_status)
                    );
                    let _ = writeln!(
                        block,
                        "[gpustep10] CE memset A (byte {:#04x}) + spot-check: {}",
                        r.pattern & 0xFF,
                        phase(r.memset_status)
                    );
                    let _ = writeln!(
                        block,
                        "[gpustep10] CPU unique-fill A + CE copy A->B: {}",
                        phase(r.copy_status)
                    );
                    let _ = writeln!(
                        block,
                        "[gpustep10] CPU verify B (per-dword unique): {} ({} dwords checked, {} mismatches)",
                        phase(r.verify_status),
                        r.dwords_checked,
                        r.mismatch_count
                    );
                    if r.mismatch_count > 0 {
                        // Expected value mirrors the C's ECLIPSE_FILL(i).
                        let expected = r.pattern ^ r.first_mismatch_idx.wrapping_mul(0x0100_0193);
                        let _ = writeln!(
                            block,
                            "[gpustep10] first mismatch: dword {} = {:#010x} (expected {:#010x})",
                            r.first_mismatch_idx, r.first_mismatch_val, expected
                        );
                    }
                    if r.verify_status == 0 {
                        let _ = writeln!(
                            block,
                            "[gpustep10] --- COPY ENGINE DATA MOVEMENT VERIFIED: pattern written, copied and read back through real hardware ---"
                        );
                    }
                }
                Err(status) => {
                    let _ = writeln!(
                        block,
                        "[gpustep10] eclipse_rm_step10 FAILED, NV_STATUS={:#x} (state not loaded? run /proc/gpustep9)",
                        status
                    );
                }
            }
            // Same rule as step9: an early Err ran nothing in the RM, so
            // caching it would shadow a later real attempt in this boot.
            if !early_err {
                let mut cache = self.step10_result.lock();
                if cache.is_none() {
                    *cache = Some(block.clone());
                }
            }
            block
        };
        s.push_str(&block);
        s
    }

    /// Step 6 (`/proc/gpustep6`), NOT read-only and NOT part of `/proc/gpudbg`:
    /// first real invocation of `kgspInitRm` (kernel_gsp.c) -- the deepest,
    /// riskiest bring-up step yet (VBIOS/FWSEC extraction, Booter ucode
    /// secure boot on SEC2, WPR2 setup). Kept on its own explicit trigger,
    /// same reasoning as `bringup_step5`. Requires a successful
    /// `bringup_step5` first AND gsp.bin already pushed down by
    /// `set_gsp_firmware` (zCore's boot code, after rootfs mount) --
    /// reports which is missing rather than erroring if either is absent.
    fn bringup_step6(&self) -> String {
        let mut s = String::new();

        // EXPERIMENT (SEC2 CORE_RESUME wedge): starting SEC2 to resume GSP-RM
        // permanently wedges the GPU's bus interface on the CONSOLE GPU -- even
        // a raw BSI read after 500 ms of total MMIO silence never returns. The
        // software sequence is byte-for-byte what nouveau/Linux run successfully
        // on Turing, so the suspect is console-GPU-specific state: it is the
        // VBIOS-POSTed primary with GOP scanout live, and its BAR1 is being
        // written by this very console (GSP-RM's devinit sequencer may also
        // reconfigure apertures under our feet). The second RTX 2060 Super has
        // none of that baggage. So: boot GSP only on the GPU(s) NOT driving the
        // boot display. If the secondary boots clean, the driver stack is
        // proven end-to-end and the console-GPU collision is isolated as the
        // remaining problem (likely fix: stop console rendering during its GSP
        // boot). If the secondary wedges identically, the console theory dies.
        if self.drives_boot_display() {
            return String::from(
                "[gpustep6]  --- Real GSP-RM boot: SKIPPED on console GPU (SEC2 resume wedges its bus \
                 while the console renders into its BAR1; use /proc/gpustep11, which freezes the \
                 graphic console around the boot) ---\n",
            );
        }

        s.push_str(&self.gsp_boot_run("gpustep6", false));
        s
    }

    /// Step 11 (`/proc/gpustep11`): GSP-RM boot on the CONSOLE GPU -- the one
    /// step 6 refuses to touch. The wedge theory, refined by the secondary
    /// GPU booting flawlessly with byte-identical software: during the SEC2
    /// GSP-RM resume window, CPU writes into this GPU's BAR1 (which is
    /// exactly where the graphic console framebuffer lives -- and step 6's
    /// RmMsg narration prints DOZENS of lines, each one drawing pixels) stall
    /// the bus for good. NVIDIA's own driver avoids this class of collision
    /// with os_disable_console_access() around init. Eclipse's equivalent:
    /// the /proc/gpustep11 generator (linux-object procfs) puts the active VT
    /// into KD_GRAPHICS around this call -- pixel presentation stops (the VT
    /// shadow buffer keeps accumulating; serial/dmesg unaffected), and the
    /// return to KD_TEXT repaints everything that happened meanwhile.
    fn bringup_step11(&self) -> String {
        if !self.drives_boot_display() {
            return String::from(
                "[gpustep11] SKIPPED on secondary GPU (already boots via /proc/gpustep6)\n",
            );
        }
        let mut s = String::new();
        // Declare this GPU's real identity to RM BEFORE the GSP boot, exactly
        // where Linux does (RmDeterminePrimaryDevice /
        // RmSetConsolePreservationParams right before kgspInitRm): it is the
        // PRIMARY device with a live UEFI GOP console in its BAR1. Without
        // this, the SET_GUEST_SYSTEM_INFO RPC told GSP-RM `bIsPrimary=false`
        // and no console region was reserved -- the one remaining difference
        // vs. the (working) secondary GPU after the console-freeze experiment
        // exonerated CPU pixel writes. Idempotent: plain property/field
        // writes, safe to repeat on a cached re-read.
        if let Some(device_instance) = *self.rm_device_instance.lock() {
            let (console_size, at_bar1_base) = match *BOOT_FB_INFO.lock() {
                Some(fb) => (
                    fb.pitch as u64 * fb.height as u64,
                    fb.phys == self.bar1_phys,
                ),
                None => (0, false),
            };
            let mark = nvidia_rm_sys::rm_init::mark_console_gpu(
                device_instance,
                console_size,
                at_bar1_base,
            );
            match mark {
                Ok(()) => {
                    s.push_str(&alloc::format!(
                        "[gpustep11] console-GPU identity declared to RM (PRIMARY_DEVICE, console {} KiB, at BAR1 base: {})\n",
                        console_size / 1024,
                        at_bar1_base
                    ));
                }
                Err(status) => {
                    s.push_str(&alloc::format!(
                        "[gpustep11] mark_console_gpu FAILED, NV_STATUS={:#x} (continuing to boot anyway)\n",
                        status
                    ));
                }
            }
        }
        // EXPERIMENT (SEC2-RTOS resume wedge, round 3): the live trace showed
        // the console GPU boots GSP-RM fine all the way to the CPU sequencer's
        // CORE_RESUME -- Booter Load clean, RISC-V started, RUN_CPU_SEQUENCER
        // RPC received -- and then the FIRST BAR0 read after restarting SEC2
        // (whose VBIOS SEC2-RTOS/BSI payload runs display/VGA restore phases
        // on a PRIMARY device) never completes. NVIDIA's own primary-VGA
        // detection keys on PCI I/O decode (kbifIsPciIoAccessEnabled,
        // osinit.c:900) -- the one config-space difference vs. the (working)
        // secondary GPU, and one the GPU firmware can see through its own
        // config mirror. So: clear PCI COMMAND bit 0 (I/O Space Enable) for
        // the duration of the boot, making the console GPU indistinguishable
        // from a secondary to the SEC2-RTOS, and restore it afterwards.
        // Console rendering is untouched (BAR1 is MEM space, bit 1).
        let io_cmd_old = {
            use crate::bus::pci::{PortOpsImpl, PCI_ACCESS};
            use pci::Location;
            let loc = Location {
                bus: self.pci_bus,
                device: self.pci_device,
                function: 0,
            };
            let ops = &PortOpsImpl;
            let cmd = unsafe { PCI_ACCESS.read16(ops, loc, 0x04) };
            unsafe { PCI_ACCESS.write16(ops, loc, 0x04, cmd & !0x0001) };
            s.push_str(&alloc::format!(
                "[gpustep11] PCI I/O decode disabled for the boot (COMMAND {:#06x} -> {:#06x}; SEC2-RTOS should now see a non-primary GPU)\n",
                cmd,
                cmd & !0x0001
            ));
            cmd
        };
        // Full-chain legacy-VGA routing disable (device I/O decode above +
        // every bridge on the path here): the SEC2-RTOS display/VGA handoff
        // suspects legacy routing state; the earlier round only cleared the
        // function-level bit. Restored after the boot returns.
        let (bridge_log, bridges_changed) = self.set_path_vga_routing(true, &[]);
        s.push_str(&bridge_log);
        // Containment (root-port completion timeout) + post-STARTCPU bus
        // autopsy instrumentation -- see their doc comments.
        s.push_str(&self.arm_completion_timeout());
        nvidia_rm_sys::os_boundary::autopsy_arm(self.config_handle(), self.parent_config_handle());
        // Diagnostics stay on: live_echo lifts RM narration (and the
        // sequencer register trace, armed inside gsp_boot_run) to ERROR so
        // it renders live at LOG=error -- a wedge leaves the exact hanging
        // register access as the last line on screen.
        nvidia_rm_sys::os_interface::live_echo_begin();
        let boot = self.gsp_boot_run("gpustep11", false);
        nvidia_rm_sys::os_interface::live_echo_end();
        nvidia_rm_sys::os_boundary::autopsy_disarm();
        let (restore_log, _) = self.set_path_vga_routing(false, &bridges_changed);
        s.push_str(&restore_log);
        {
            use crate::bus::pci::{PortOpsImpl, PCI_ACCESS};
            use pci::Location;
            let loc = Location {
                bus: self.pci_bus,
                device: self.pci_device,
                function: 0,
            };
            let ops = &PortOpsImpl;
            // Restore the original COMMAND value, except INTx stays masked
            // (gsp_boot_run masked it; Eclipse is fully polled).
            unsafe { PCI_ACCESS.write16(ops, loc, 0x04, io_cmd_old | (1 << 10)) };
            s.push_str("[gpustep11] PCI I/O decode restored after boot\n");
        }
        s.push_str(&boot);
        s
    }

    /// Step 12 (`/proc/gpustep12`): EXP 1 -- console-GPU GSP boot with the
    /// DISPLAY ENGINE HELD IN RESET (PMC_ENABLE bit 30 cleared right after
    /// kgspCalculateFbLayout consumes NV_PDISP_VGA_WORKSPACE_BASE, via the
    /// register-shim trigger in os_boundary). Zero isochronous scanout
    /// traffic during the SEC2-RTOS resume: if the wedge is live-display FB
    /// fetch vs. the HS payload, this boot COMPLETES. THE SCREEN GOES DARK
    /// at the trigger and stays dark until reboot -- run it blind:
    ///   `cat /proc/gpustep12 > /r12.txt; sync`
    /// then hard-reset and read /r12.txt. Skip this experiment entirely if
    /// the step-11 preboot dump showed the primary's heads already
    /// SLEEP/frozen (theory pre-falsified).
    fn hw_dump(&self) -> String {
        self.hw_dump_impl()
    }

    fn bringup_step12(&self) -> String {
        if !self.drives_boot_display() {
            return String::from(
                "[gpustep12] SKIPPED on secondary GPU (already boots via /proc/gpustep6)\n",
            );
        }
        let mut s = String::new();
        s.push_str(
            "[gpustep12] EXP1c: PDISP reset ONLY for the SEC2-resume window, then restored at 'RISCV started' -- screen blanks then comes back; capture with `cat /proc/gpustep12 > /r12.txt; sync`\n",
        );
        // EXP1c: EXP1b proved being non-primary doesn't fix the
        // kgspWaitForRmInitDone timeout -- so PDISP-in-reset itself is what
        // stalls GSP-RM's init (it touches the display engine before
        // GSP_INIT_DONE). Fix: os_boundary holds PDISP in reset only across
        // the SEC2 HS-resume (the wedge window) and restores it on the
        // "RISCV started" narration marker, so GSP-RM finds the display alive.
        // Still non-primary for now to isolate the timeout fix.
        s.push_str(&self.arm_completion_timeout());
        nvidia_rm_sys::os_boundary::autopsy_arm(self.config_handle(), self.parent_config_handle());
        nvidia_rm_sys::os_boundary::pdisp_kill_arm();
        nvidia_rm_sys::os_interface::live_echo_begin();
        let boot = self.gsp_boot_run("gpustep12", false);
        nvidia_rm_sys::os_interface::live_echo_end();
        nvidia_rm_sys::os_boundary::pdisp_kill_disarm();
        nvidia_rm_sys::os_boundary::autopsy_disarm();
        s.push_str(&boot);
        s
    }

    /// Step 13 (`/proc/gpustep13`): EXP2 -- console-GPU GSP boot with a
    /// pre-STARTCPU interrupt-drain "pseudo-ISR service loop" (Copilot's
    /// leading hypothesis). Eclipse is 100% polled with INTx masked and no RM
    /// ISR, so during the SEC2 CORE_RESUME window a fabric/display interrupt
    /// the GPU raises is never serviced -- the prime suspect for the STARTCPU
    /// posted-write stall (flow-control credit exhaustion) that Linux, whose
    /// ISR drains it, never hits. Right before the STARTCPU store, os_boundary
    /// snapshots the CPU-facing top-level interrupt tree (ERROR level, survives
    /// the wedge -> names the pending vector) and write-1-to-clears the leaves
    /// until quiescent, then lets the store through. Unlike EXP1 it does NOT
    /// touch PDISP, so it can't break GSP-RM's early boot. Two outcomes, both
    /// useful in one boot: STARTCPU drains (autopsy runs, boot continues) =>
    /// hypothesis confirmed, drain is the fix; still wedges => the snapshot
    /// tells us exactly which interrupt Linux services in that window. The
    /// screen is untouched (no display reset) -- but capture to a file anyway
    /// in case STARTCPU still wedges:
    ///   `cat /proc/gpustep13 > /r13.txt; sync` then read /r13.txt.
    fn bringup_step13(&self) -> String {
        if !self.drives_boot_display() {
            return String::from(
                "[gpustep13] SKIPPED on secondary GPU (already boots via /proc/gpustep6)\n",
            );
        }
        let mut s = String::new();
        s.push_str(
            "[gpustep13] EXP3: pre-STARTCPU interrupt snapshot + UNCONDITIONAL W1C drain (classifies latched vs live-level source); no PDISP/display touch -- snapshot at ERROR survives a wedge; capture with `cat /proc/gpustep13 > /r13.txt; sync`\n",
        );
        // Same containment + autopsy instrumentation as step11/12 so the
        // post-STARTCPU physics are classified either way. The ONLY new
        // variable vs. a plain console boot is the interrupt drain armed below.
        s.push_str(&self.arm_completion_timeout());
        nvidia_rm_sys::os_boundary::autopsy_arm(self.config_handle(), self.parent_config_handle());
        nvidia_rm_sys::os_boundary::sec2_drain_arm();
        nvidia_rm_sys::os_interface::live_echo_begin();
        let boot = self.gsp_boot_run("gpustep13", false);
        nvidia_rm_sys::os_interface::live_echo_end();
        nvidia_rm_sys::os_boundary::sec2_drain_disarm();
        nvidia_rm_sys::os_boundary::autopsy_disarm();
        s.push_str(&boot);
        s
    }

    /// Step 14 (`/proc/gpustep14`): the CONSOLE GPU's full bring-up chained in
    /// one shot -- RM attach, GSP-RM boot (with the permanent console SEC2
    /// drain in gsp_boot_run), RM-client controls, gpuStatePreInit/Init/Load,
    /// and the copy-engine data-movement test -- so the primary reaches the
    /// same state-loaded, CE-verified state the secondary already has, in a
    /// single `cat`. Each sub-step is cached and live-echoed, so a wedge or
    /// failure at any stage leaves its trail on the console and in the capture.
    /// Blanks nothing and needs no display reset. Capture with
    /// `cat /proc/gpustep14 > /r14.txt; sync`. On the secondary GPU this is a
    /// no-op (it already boots via gpustep6 and runs 8/9/10 directly).
    fn bringup_step14(&self) -> String {
        if !self.drives_boot_display() {
            return String::from(
                "[gpustep14] SKIPPED on secondary GPU (use gpustep5/6/8/9/10 directly)\n",
            );
        }
        let mut s = String::new();
        s.push_str(
            "[gpustep14] === CONSOLE GPU full bring-up: attach -> GSP boot (drain) -> RM controls -> state-load -> CE ===\n",
        );
        // 1. RM attach (sets rm_device_instance). bringup_step5 is idempotent
        //    (cached); safe to always call -- it no-ops if already attached.
        if self.rm_device_instance.lock().is_none() {
            s.push_str("[gpustep14] --- stage 1: RM attach (gpustep5) ---\n");
            s.push_str(&self.bringup_step5());
        }
        // 1.5 REMOVED: do NOT declare PRIMARY_DEVICE/console to RM before the
        //    boot. Cross-build statistics over every console-GPU boot ever
        //    made: with mark_console_gpu (bIsPrimary=true + consoleMemSize in
        //    SET_SYSTEM_INFO -- all step11 runs and every step14 run since
        //    da884def): 0 successes in 9+ boots. WITHOUT it (bIsPrimary=false,
        //    no console reservation -- the pre-da884def step13/14 runs): 2
        //    successes in 3, INCLUDING the full attach->boot->state-load->CE
        //    chain with the console visibly still working afterwards. The
        //    mechanism matches the cross-cluster model exactly: declaring a
        //    primary/console GPU makes the SEC2-HS CORE_RESUME payload run
        //    its display/VGA/console-preservation path -- display-domain PRI
        //    traffic while the head is actively scanning, the precise
        //    SYS<->DISP forward-progress hazard that wedges the fabric. Linux
        //    tolerates it via something environmental we haven't identified;
        //    our polled bring-up doesn't need the reservation (the GSP's FB
        //    carving demonstrably left the scanout surface intact on the
        //    successful full-chain run). Revisit console preservation later,
        //    post-boot, if FB carving ever eats the console.
        // 2. GSP-RM boot. gsp_boot_run arms the console SEC2 drain internally
        //    now, so this is the proven path; cached after the first boot.
        s.push_str("[gpustep14] --- stage 2: GSP-RM boot (kgspInitRm, console-SILENT, Linux-parity STARTCPU, VGA decode off, PBUS pre-clear) ---\n");
        // Renounce legacy VGA decode for the boot, like Linux does at PCI
        // probe (nv-pci.c:855-858: vga_tryget + vga_set_legacy_decoding
        // VGA_RSRC_NONE): clear the function's I/O decode + every bridge VGA
        // routing bit on the path. Restored after the boot. Console rendering
        // is untouched (BAR1 is MEM space). With console-silence and the
        // Linux-parity STARTCPU bracket this makes the boot environment
        // converge on Linux's in every knob Linux is known to set.
        let io_cmd_old = {
            use crate::bus::pci::{PortOpsImpl, PCI_ACCESS};
            use pci::Location;
            let loc = Location {
                bus: self.pci_bus,
                device: self.pci_device,
                function: 0,
            };
            let ops = &PortOpsImpl;
            let cmd = unsafe { PCI_ACCESS.read16(ops, loc, 0x04) };
            unsafe { PCI_ACCESS.write16(ops, loc, 0x04, cmd & !0x0001) };
            s.push_str(&alloc::format!(
                "[gpustep14] PCI I/O decode disabled for the boot (COMMAND {:#06x} -> {:#06x})\n",
                cmd,
                cmd & !0x0001
            ));
            cmd
        };
        let (bridge_log, bridges_changed) = self.set_path_vga_routing(true, &[]);
        s.push_str(&bridge_log);
        // LOUD + the empirical 0058b6f4 bracket (tight leaf-W1C before the
        // store, NO display/priv-ring reads). The falsification ladder is
        // complete: console silence, PBUS unit clear, VGA decode off, MPS
        // normalization and full Linux byte-parity (empty bracket) ALL
        // wedged; the only two boots that ever survived STARTCPU ran the
        // tight-W1C bracket without the EXP4 display-cluster reads (r13 on
        // 359cef1e, full r14 chain on 0058b6f4). Mechanism is only partially
        // understood (a hub-leaf W1C posted immediately before the store,
        // plausibly flushing/fencing the PRI path across the SEC2 handoff),
        // but the correlation is 2-for-3 vs 0-for-everything-else, so run
        // the exact recipe with all the new pre-boot hygiene (console-mark,
        // MPS normalize, PBUS clear, VGA off) layered on top. gsp_boot_run
        // arms the drain for console GPUs when quiet=false.
        nvidia_rm_sys::os_interface::live_echo_begin();
        s.push_str(&self.gsp_boot_run("gpustep14", false));
        nvidia_rm_sys::os_interface::live_echo_end();
        let (restore_log, _) = self.set_path_vga_routing(false, &bridges_changed);
        s.push_str(&restore_log);
        {
            use crate::bus::pci::{PortOpsImpl, PCI_ACCESS};
            use pci::Location;
            let loc = Location {
                bus: self.pci_bus,
                device: self.pci_device,
                function: 0,
            };
            let ops = &PortOpsImpl;
            // Restore the original COMMAND value, except INTx stays masked
            // (gsp_boot_run masked it; Eclipse is fully polled).
            unsafe { PCI_ACCESS.write16(ops, loc, 0x04, io_cmd_old | (1 << 10)) };
            s.push_str("[gpustep14] PCI I/O decode restored after boot\n");
        }
        // 3-5. RM controls, state pre-init/init/load, CE data movement -- reuse
        //    the exact same code paths proven on the secondary GPU.
        s.push_str("[gpustep14] --- stage 3: RM API controls (gpustep8) ---\n");
        s.push_str(&self.bringup_step8());
        s.push_str("[gpustep14] --- stage 4: gpuStatePreInit/Init/Load (gpustep9) ---\n");
        s.push_str(&self.bringup_step9());
        s.push_str("[gpustep14] --- stage 5: copy-engine data movement (gpustep10) ---\n");
        s.push_str(&self.bringup_step10());
        s.push_str("[gpustep14] === console GPU bring-up chain complete (see per-stage results above) ===\n");
        s
    }

    /// CE-offloaded present: dumb buffer (sysmem) -> scanout FB (VRAM) via the
    /// persistent CeUtils channel. Called from the DRM `scanout()` per frame in
    /// place of the CPU blit, when the console GPU is state-loaded. Takes the RM
    /// locks internally (safe: `scanout()` holds no DRM lock here). Returns true
    /// only if the CE copy actually ran, so the caller can fall back to CPU.
    /// Automatic boot-time compute-GPU bring-up (see the `DrmScheme` trait
    /// doc). Runs the proven `/proc/gpustep5;6;8;9` chain on this GPU — but
    /// only if it does NOT drive the boot display. The console GPU is skipped
    /// unconditionally: its GSP-RM boot wedges at the SEC2 STARTCPU store
    /// (see `bringup_step6`), so it is never auto-booted; the compute GPU(s)
    /// are the reliable path and drive the console's scanout FB over PCIe P2P.
    fn auto_bringup_compute(&self) -> String {
        // The console GPU is never auto-booted (its GSP boot wedges at the SEC2
        // STARTCPU store) and drives the display fine via the GOP framebuffer.
        // Return nothing so the quiet boot path prints no line for it.
        if self.drives_boot_display() {
            return String::new();
        }
        // Run the proven state-load chain (the same sequence a user triggers as
        // `cat /proc/gpustep5;6;8;9`), executed once at boot before any
        // userspace or scanout touches RM (fixed RM thread-id 0, no concurrent
        // access -> no reentrancy hazard). The verbose per-step narration is
        // DISCARDED here -- it still lands in the /proc/gpustep* capture buffers
        // for debugging, but must not flood the desktop console. The caller
        // suppresses the driver's own log output around this call; all this
        // method emits is a single clean status line.
        let _ = self.bringup_step5();
        let _ = self.bringup_step6();
        let _ = self.bringup_step8();
        let _ = self.bringup_step9();
        if self.rm_device_instance.lock().is_some() {
            alloc::format!(
                "GPU {:02x}:{:02x}.0 {} listo — aceleración de present activada (compute/P2P)",
                self.pci_bus,
                self.pci_device,
                self.gpu_model,
            )
        } else {
            alloc::format!(
                "GPU {:02x}:{:02x}.0 {} sin aceleración de present — se usa copia por CPU",
                self.pci_bus,
                self.pci_device,
                self.gpu_model,
            )
        }
    }

    /// Leave this GPU cold for the next firmware POST (see the `DrmScheme`
    /// trait doc). Only a GPU we actually state-loaded carries a live GSP-RM /
    /// locked WPR2 that a warm reboot would strand; others are no-ops. Issues a
    /// PCIe Function Level Reset, which on Turing resets the engines and
    /// falcons and lets the next VBIOS devinit re-run cleanly.
    fn quiesce_for_reboot(&self) -> String {
        if self.rm_device_instance.lock().is_none() {
            return String::new();
        }
        if self.pcie_flr() {
            alloc::format!(
                "[gpureset] FLR emitido en {:02x}:{:02x}.0 (estado limpio para el POST)\n",
                self.pci_bus,
                self.pci_device,
            )
        } else {
            alloc::format!(
                "[gpureset] {:02x}:{:02x}.0 sin capacidad FLR; GPU sin resetear\n",
                self.pci_bus,
                self.pci_device,
            )
        }
    }

    fn ce_present(&self, src_sysmem_pa: u64, size: u64) -> bool {
        if src_sysmem_pa == 0 || size == 0 {
            return false;
        }
        let device_instance = match *self.rm_device_instance.lock() {
            Some(d) => d,
            None => return false, // this GPU not state-loaded
        };
        let fb_phys = match boot_fb_phys() {
            Some(p) if p != 0 => p,
            _ => return false,
        };
        let (st, how) = if self.drives_boot_display() {
            // Console GPU: its own CE writes its own VRAM (ADDR_FBMEM). Direct,
            // but only when the console GPU is state-loaded (its bring-up is
            // unreliable), so this rarely fires in practice.
            let bar1 = self.bar1_phys;
            if fb_phys < bar1 {
                return false;
            }
            (
                nvidia_rm_sys::rm_init::ce_blit(
                    device_instance,
                    fb_phys - bar1,
                    src_sysmem_pa,
                    size,
                ),
                "console/FBMEM",
            )
        } else {
            // Compute GPU: P2P copy into the console GPU's scanout FB (its BAR1
            // host physical address, ADDR_SYSMEM). The reliable path — the
            // compute GPU always boots. Depends on PCIe P2P not being ACS-blocked.
            (
                nvidia_rm_sys::rm_init::ce_blit_p2p(device_instance, fb_phys, src_sysmem_pa, size),
                "compute/P2P",
            )
        };
        if st == 0 {
            if !CE_PRESENT_LOGGED.swap(true, Ordering::Relaxed) {
                log::warn!(
                    "[NVIDIA] CE-offload present ACTIVE ({}): src {:#x} -> FB {:#x} size {:#x}",
                    how,
                    src_sysmem_pa,
                    fb_phys,
                    size
                );
            }
            true
        } else {
            false
        }
    }

    /// `/proc/gpucefill`: CE-offload visual test. On the state-loaded console
    /// GPU, CE-memset the scanout framebuffer to a solid colour via the
    /// persistent CeUtils channel (`eclipse_rm_ce_fill_fb`). If the screen turns
    /// that colour, the BAR1->VRAM offset (`fb_phys - bar1_phys`) is correct and
    /// the CE can drive the display — the green light to wire the full per-frame
    /// `ce_blit` present path. The low byte of the pattern is what the CE writes
    /// (byte-remap), so a replicated-byte colour (here 0xFF -> white) results.
    fn bringup_ce_fill_fb(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        if !self.drives_boot_display() {
            return String::from(
                "[gpucefill] SKIPPED on secondary GPU (it has no scanout framebuffer)\n",
            );
        }
        let device_instance = match *self.rm_device_instance.lock() {
            Some(d) => d,
            None => {
                return String::from(
                    "[gpucefill] skipped: console GPU not state-loaded -- run `cat /proc/gpustep14` first\n",
                );
            }
        };
        let fb_phys = match boot_fb_phys() {
            Some(p) if p != 0 => p,
            _ => {
                return String::from("[gpucefill] no boot framebuffer physical address recorded\n")
            }
        };
        let bar1 = self.bar1_phys;
        if fb_phys < bar1 {
            let _ = writeln!(
                s,
                "[gpucefill] fb_phys {:#x} < bar1_phys {:#x} -- unexpected; aborting (would underflow the VRAM offset)",
                fb_phys, bar1
            );
            return s;
        }
        let fb_vram_offset = fb_phys - bar1;
        let size = (self.info.pitch as u64) * (self.info.height as u64);
        // Low byte replicated by the CE: 0xFF -> every byte 0xFF -> white.
        let pattern: u32 = 0x0000_00FF;
        let _ = writeln!(
            s,
            "[gpucefill] fb_phys={:#x} bar1_phys={:#x} => fb_vram_offset={:#x}  size={:#x} ({}x{} pitch {})  pattern={:#x} (low byte -> white)",
            fb_phys, bar1, fb_vram_offset, size, self.info.width, self.info.height, self.info.pitch, pattern
        );
        let st = nvidia_rm_sys::rm_init::ce_fill_fb(device_instance, fb_vram_offset, size, pattern);
        let _ = writeln!(
            s,
            "[gpucefill] ce_fill_fb -> {:#x} ({})",
            st,
            if st == 0 {
                "OK -- if the screen is now WHITE, the VRAM offset is correct and the CE drives the display"
            } else {
                "FAILED -- CE submit did not complete"
            }
        );
        s
    }

    /// `/proc/gpucefillp2p`: P2P CE-offload visual test. On the state-loaded
    /// COMPUTE GPU (the reliable one), CE-memset the CONSOLE GPU's scanout
    /// framebuffer to white via PCIe peer-to-peer (`eclipse_rm_ce_fill_fb_p2p`
    /// with dst = the console FB's host physical address). If the screen turns
    /// white, PCIe P2P works and we can drive the display from the compute GPU
    /// without ever bringing up the flaky console GPU — the whole point of
    /// via-A. If the CE returns OK but the screen does NOT change, P2P is
    /// ACS-blocked. Requires the compute GPU state-loaded (its own bring-up).
    fn bringup_ce_fill_fb_p2p(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        if self.drives_boot_display() {
            return String::from(
                "[gpucefillp2p] skipped on the CONSOLE GPU -- this test drives it FROM the compute GPU via P2P\n",
            );
        }
        let device_instance = match *self.rm_device_instance.lock() {
            Some(d) => d,
            None => {
                return String::from(
                    "[gpucefillp2p] compute GPU not state-loaded -- bring it up first (gpustep5/6/8/9 on the secondary)\n",
                );
            }
        };
        let fb_phys = match boot_fb_phys() {
            Some(p) if p != 0 => p,
            _ => {
                return String::from(
                    "[gpucefillp2p] no boot framebuffer physical address recorded\n",
                )
            }
        };
        let size = match boot_fb_size() {
            Some(s) if s != 0 => s,
            _ => return String::from("[gpucefillp2p] no boot framebuffer size recorded\n"),
        };
        let pattern: u32 = 0x0000_00FF; // low byte -> white
        let _ = writeln!(
            s,
            "[gpucefillp2p] compute GPU instance={} -> console FB host_pa={:#x} size={:#x} pattern={:#x} (P2P)",
            device_instance, fb_phys, size, pattern
        );
        let st = nvidia_rm_sys::rm_init::ce_fill_fb_p2p(device_instance, fb_phys, size, pattern);
        let _ = writeln!(
            s,
            "[gpucefillp2p] ce_fill_fb_p2p -> {:#x} ({})",
            st,
            if st == 0 {
                "CE OK -- if the screen is now WHITE, PCIe P2P works and the compute GPU can drive the display"
            } else {
                "FAILED -- CE submit did not complete"
            }
        );
        if st == 0 {
            s.push_str("[gpucefillp2p] NOTE: CE OK but screen UNCHANGED => P2P is ACS-blocked (writes routed away from the console BAR1)\n");
        }
        s
    }

    /// `/proc/gpusurvive`: read + clear the CMOS survival breadcrumb from the
    /// previous console-GPU boot attempt. Only the console GPU reports (it is
    /// the one that wedges, and the breadcrumb is global — a second reader would
    /// just see the already-cleared slate).
    fn survival_report(&self) -> String {
        if self.drives_boot_display() {
            nvidia_rm_sys::survival::read_report_and_clear()
        } else {
            String::new()
        }
    }

    /// Step 15 (`/proc/gpustep15`): probe the GR (graphics/compute) engine's
    /// shader config on a state-loaded GPU via the live GSP-RM's resource
    /// server (GR_GET_GPC_MASK / GR_GET_TPC_MASK controls) -- the first read of
    /// the SM array the compute engine runs on. Read-only, repeatable, no
    /// channel machinery; groundwork toward a real compute launch. Works on
    /// any GPU that has completed state-load (secondary via gpustep9, console
    /// via gpustep14).
    fn bringup_step15(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let device_instance = *self.rm_device_instance.lock();
        let Some(device_instance) = device_instance else {
            return String::from(
                "[gpustep15] skipped (bring the GPU up first: gpustep5/6/8/9 on the secondary, or gpustep14 on the console)\n",
            );
        };
        nvidia_rm_sys::os_interface::capture_begin();
        let result = nvidia_rm_sys::rm_init::step15(device_instance);
        let captured = nvidia_rm_sys::os_interface::capture_take();
        if let Some(log) = captured {
            for line in log.lines() {
                let _ = writeln!(s, "[gpustep15] | {}", line);
            }
        }
        let phase = |st: u32| -> String {
            if st == 0 {
                String::from("OK")
            } else {
                alloc::format!("NV_STATUS={:#x}", st)
            }
        };
        match result {
            Ok(gr) => {
                let _ = writeln!(
                    s,
                    "[gpustep15] --- GR (graphics/compute) engine config from live GSP-RM ---"
                );
                let _ = writeln!(
                    s,
                    "[gpustep15] GR_GET_GPC_MASK: {} mask={:#010x} ({} GPCs)",
                    phase(gr.gpc_mask_status),
                    gr.gpc_mask,
                    gr.num_gpc
                );
                if gr.gpc_mask_status == 0 {
                    for gpc in 0..8usize {
                        if (gr.gpc_mask >> gpc) & 1 == 1 {
                            let _ = writeln!(
                                s,
                                "[gpustep15]   GPC{}: {} TPCs",
                                gpc, gr.per_gpc_tpc[gpc]
                            );
                        }
                    }
                    let _ = writeln!(
                        s,
                        "[gpustep15] GR_GET_TPC_MASK: {}",
                        phase(gr.tpc_mask_status)
                    );
                    // Turing packs TWO SMs per TPC (Volta+; the 1-SM/TPC layout
                    // was consumer Pascal). RTX 2060 Super: 17 TPCs => 34 SMs.
                    let _ = writeln!(
                        s,
                        "[gpustep15] --- {} TPCs total => {} usable SMs (Turing: 2 SMs/TPC) ---",
                        gr.total_tpc,
                        gr.total_tpc * 2
                    );
                }
            }
            Err(status) => {
                let _ = writeln!(
                    s,
                    "[gpustep15] eclipse_rm_step15 FAILED, NV_STATUS={:#x} (GR not state-loaded? run gpustep9 or gpustep14)",
                    status
                );
            }
        }
        // Interrupt kernel table: the GSP's own authoritative vector->engine
        // map (the same control kernel RM uses to build its interrupt table:
        // NV2080_CTRL_CMD_INTERNAL_INTR_GET_KERNEL_TABLE). Settles empirically
        // which engine owns CPU vector 156 (the LEAF[4] bit28 level source
        // behind the console GPU's SEC2 wedge) and which engine drives legacy
        // PMC mask 0x10000000 -- research says PBUS for both; this is the
        // ground truth from this exact GPU.
        fn engine_name(idx: u32) -> &'static str {
            match idx {
                0 => "NULL",
                1 => "TMR",
                2 => "DISP",
                3 => "FB",
                4 => "FIFO",
                7 => "BUS",
                8 => "PMGR",
                11 => "BIF",
                13 => "PRIVRING",
                14 => "PMU",
                15 => "CE0",
                16 => "CE1",
                17 => "CE2",
                18 => "CE3",
                19 => "CE4",
                20 => "CE5",
                43 => "LTC",
                44 => "FBHUB",
                45 => "HDACODEC",
                46 => "GMMU",
                47 => "SEC2",
                49 => "NVLINK",
                50 => "GSP",
                59 => "REPLAYABLE_FAULT",
                60 => "ACCESS_CNTR",
                61 => "NON_REPLAYABLE_FAULT",
                64 => "INFO_FAULT",
                65 => "NVDEC0",
                73 => "CPU_DOORBELL",
                74 => "PRIV_DOORBELL",
                75 => "MMU_ECC_ERROR",
                77 => "PERFMON",
                84 => "GR0",
                156 => "GR_FECS_LOG",
                164 => "TMR_SWRL",
                165 => "DISP_GSP",
                166 => "REPLAYABLE_FAULT_CPU",
                167 => "NON_REPLAYABLE_FAULT_CPU",
                _ => "?",
            }
        }
        match nvidia_rm_sys::rm_init::intr_table(device_instance) {
            Ok(t) => {
                if t.ctrl_status != 0 {
                    let _ = writeln!(
                        s,
                        "[gpustep15] INTR_GET_KERNEL_TABLE control FAILED, NV_STATUS={:#x} (table below is empty)",
                        t.ctrl_status
                    );
                }
                let _ = writeln!(
                    s,
                    "[gpustep15] --- GSP interrupt kernel table ({} entries; rows with a vector or legacy PMC mask; >>> = vector 156 or mask 0x10000000) ---",
                    t.table_len
                );
                for e in t.entries.iter().take(t.table_len as usize) {
                    let hot = e.vector_stall == 156
                        || e.vector_non_stall == 156
                        || e.pmc_intr_mask & 0x1000_0000 != 0;
                    let has_vec = e.vector_stall != u32::MAX || e.vector_non_stall != u32::MAX;
                    if hot || e.pmc_intr_mask != 0 || has_vec {
                        let vs = if e.vector_stall == u32::MAX {
                            String::from("-")
                        } else {
                            alloc::format!("{}", e.vector_stall)
                        };
                        let vn = if e.vector_non_stall == u32::MAX {
                            String::from("-")
                        } else {
                            alloc::format!("{}", e.vector_non_stall)
                        };
                        let _ = writeln!(
                            s,
                            "[gpustep15] {} engine={:3} ({:<22}) pmcMask={:#010x} vecStall={:>5} vecNonStall={:>5}",
                            if hot { ">>>" } else { "   " },
                            e.engine_idx,
                            engine_name(e.engine_idx),
                            e.pmc_intr_mask,
                            vs,
                            vn
                        );
                    }
                }
            }
            Err(status) => {
                let _ = writeln!(s, "[gpustep15] intr_table FAILED, NV_STATUS={:#x}", status);
            }
        }
        s
    }

    /// Step 16 (`/proc/gpustep16`): the GR allocation ladder on a
    /// state-loaded GPU -- client -> device -> subdevice -> VA space -> TSG
    /// bound to the GRAPHICS engine -> context share (SYNC/VEID0), all
    /// through the vendored resource server against the live GSP. The first
    /// allocations Eclipse makes itself (everything before adopted GSP
    /// internal handles), and the front half of a real compute launch;
    /// step17 adds the GPFIFO channel + TURING_COMPUTE_A (golden context).
    /// Idempotent: the C side keeps the ladder alive and caches the result.
    fn bringup_step16(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let device_instance = *self.rm_device_instance.lock();
        let Some(device_instance) = device_instance else {
            return String::from(
                "[gpustep16] skipped (bring the GPU up first: gpustep5/6/8/9 on the secondary, or gpustep14 on the console)\n",
            );
        };
        nvidia_rm_sys::os_interface::capture_begin();
        let result = nvidia_rm_sys::rm_init::step16(device_instance);
        let captured = nvidia_rm_sys::os_interface::capture_take();
        if let Some(log) = captured {
            for line in log.lines() {
                let _ = writeln!(s, "[gpustep16] | {}", line);
            }
        }
        let phase = |st: u32| -> String {
            match st {
                0 => String::from("OK"),
                0xFFFF_FFFF => String::from("not reached"),
                e => alloc::format!("FAILED NV_STATUS={:#x}", e),
            }
        };
        match result {
            Ok(g) => {
                let _ = writeln!(
                    s,
                    "[gpustep16] --- GR allocation ladder (resource server on live GSP) ---"
                );
                let _ = writeln!(
                    s,
                    "[gpustep16] NV01_ROOT client:        {} (hClient={:#010x})",
                    phase(g.client_status),
                    g.h_client
                );
                let _ = writeln!(
                    s,
                    "[gpustep16] NV01_DEVICE_0:           {} (hDevice={:#010x})",
                    phase(g.device_status),
                    g.h_device
                );
                let _ = writeln!(
                    s,
                    "[gpustep16] NV20_SUBDEVICE_0:        {} (hSubdevice={:#010x})",
                    phase(g.subdev_status),
                    g.h_subdevice
                );
                let _ = writeln!(
                    s,
                    "[gpustep16] FERMI_VASPACE_A:         {} (hVas={:#010x})",
                    phase(g.vas_status),
                    g.h_vas
                );
                let _ = writeln!(
                    s,
                    "[gpustep16] KEPLER_CHANNEL_GROUP_A:  {} (hTsg={:#010x}, engineType=GRAPHICS)",
                    phase(g.tsg_status),
                    g.h_tsg
                );
                let _ = writeln!(
                    s,
                    "[gpustep16] FERMI_CONTEXT_SHARE_A:   {} (hCtxShare={:#010x})",
                    phase(g.ctxshare_status),
                    g.h_ctxshare
                );
                if g.ctxshare_status == 0 {
                    let _ = writeln!(s, "[gpustep16] --- GR ALLOCATION LADDER COMPLETE: TSG on the GRAPHICS runlist with a live subcontext; step17 = GPFIFO channel + TURING_COMPUTE_A (golden context) ---");
                }
            }
            Err(status) => {
                let _ = writeln!(
                    s,
                    "[gpustep16] eclipse_rm_step16 FAILED, NV_STATUS={:#x} (GPU not GSP-booted/state-loaded?)",
                    status
                );
            }
        }
        s
    }

    /// Step 17 (`/proc/gpustep17`): compute channel on the step-16 ladder --
    /// USERD (vidmem) + 64 KiB pushbuffer/GPFIFO memory mapped in our VAS +
    /// error notifier + GPFIFO channel (chip class, e.g. TURING_CHANNEL_
    /// GPFIFO_A) inside the TSG with our ctxshare + TURING_COMPUTE_A object
    /// + GPFIFO_SCHEDULE. After this the channel is live on the GRAPHICS
    /// runlist; step18 = QMD + SASS kernel + doorbell = first Eclipse-
    /// authored compute launch. Requires gpustep16 first. Idempotent.
    fn bringup_step17(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let device_instance = *self.rm_device_instance.lock();
        let Some(device_instance) = device_instance else {
            return String::from("[gpustep17] skipped (bring the GPU up first, then gpustep16)\n");
        };
        nvidia_rm_sys::os_interface::capture_begin();
        let result = nvidia_rm_sys::rm_init::step17(device_instance);
        let captured = nvidia_rm_sys::os_interface::capture_take();
        if let Some(log) = captured {
            for line in log.lines() {
                let _ = writeln!(s, "[gpustep17] | {}", line);
            }
        }
        let phase = |st: u32| -> String {
            match st {
                0 => String::from("OK"),
                0xFFFF_FFFF => String::from("not reached"),
                e => alloc::format!("FAILED NV_STATUS={:#x}", e),
            }
        };
        match result {
            Ok(c) => {
                let _ = writeln!(
                    s,
                    "[gpustep17] --- compute channel on the step-16 ladder ---"
                );
                let _ = writeln!(
                    s,
                    "[gpustep17] USERD (vidmem, {} B):     {} (hUserd={:#010x})",
                    c.userd_size,
                    phase(c.userd_status),
                    c.h_userd
                );
                let _ = writeln!(
                    s,
                    "[gpustep17] sysmem buf 64K:           {} (hPhysBuf={:#010x})",
                    phase(c.buf_status),
                    c.h_phys_buf
                );
                let _ = writeln!(
                    s,
                    "[gpustep17] virtual in hVas:          {} (hVirtBuf={:#010x})",
                    phase(c.virt_status),
                    c.h_virt_buf
                );
                let _ = writeln!(
                    s,
                    "[gpustep17] Map -> GPU VA:            {} (VA={:#x})",
                    phase(c.map_status),
                    c.buf_gpu_va
                );
                let _ = writeln!(
                    s,
                    "[gpustep17] error notifier 4K:        {} (hNotifier={:#010x})",
                    phase(c.notif_status),
                    c.h_notifier
                );
                let _ = writeln!(
                    s,
                    "[gpustep17] GPFIFO channel (class {:#06x}): {} (hChannel={:#010x})",
                    c.channel_class,
                    phase(c.chan_status),
                    c.h_channel
                );
                let _ = writeln!(
                    s,
                    "[gpustep17] TURING_COMPUTE_A:         {} (hCompute={:#010x})",
                    phase(c.compute_status),
                    c.h_compute
                );
                let _ = writeln!(
                    s,
                    "[gpustep17] GPFIFO_SCHEDULE:          {}",
                    phase(c.sched_status)
                );
                if c.sched_status == 0 {
                    let _ = writeln!(s, "[gpustep17] --- COMPUTE CHANNEL LIVE ON THE GRAPHICS RUNLIST: step18 = QMD + SASS kernel + doorbell (first Eclipse compute launch) ---");
                }
            }
            Err(status) => {
                let _ = writeln!(
                    s,
                    "[gpustep17] eclipse_rm_step17 FAILED, NV_STATUS={:#x} (run gpustep16 first; GPU state-loaded?)",
                    status
                );
            }
        }
        s
    }

    /// `/proc/gpuedid`: real display query via the RM's NV04_DISPLAY_COMMON.
    fn bringup_edid(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let device_instance = *self.rm_device_instance.lock();
        let Some(device_instance) = device_instance else {
            return alloc::format!(
                "[gpuedid] === {} === skipped (run /proc/gpuinit first)\n",
                self.name
            );
        };
        let _ = writeln!(
            s,
            "[gpuedid] === {} (rm instance {}) ===",
            self.name, device_instance
        );
        nvidia_rm_sys::os_interface::capture_begin();
        let result = nvidia_rm_sys::rm_init::edid(device_instance);
        let captured = nvidia_rm_sys::os_interface::capture_take();
        if let Some(log) = captured {
            for line in log.lines() {
                let _ = writeln!(s, "[gpuedid] | {}", line);
            }
        }
        let phase = |st: u32| -> String {
            match st {
                0 => String::from("OK"),
                0xFFFF_FFFF => String::from("not reached"),
                e => alloc::format!("FAILED NV_STATUS={:#x}", e),
            }
        };
        match result {
            Ok(c) => {
                let _ = writeln!(
                    s,
                    "[gpuedid] --- real display query (RM internal NV04_DISPLAY_COMMON) ---"
                );
                // 0x56 NV_ERR_NOT_SUPPORTED here is the intentional "no
                // display engine on this GPU" early-out, not a failure.
                if c.alloc_status == 0x56 {
                    let _ = writeln!(s, "[gpuedid] DispCommon handle:         none -- no display engine (headless GPU)");
                } else {
                    let _ = writeln!(
                        s,
                        "[gpuedid] DispCommon handle:         {}",
                        phase(c.alloc_status)
                    );
                }
                let _ = writeln!(
                    s,
                    "[gpuedid] GET_SUPPORTED:            {} (outputs={:#x}, DDC-capable={:#x})",
                    phase(c.supported_status),
                    c.display_mask,
                    c.display_mask_ddc
                );
                let _ = writeln!(
                    s,
                    "[gpuedid] GET_CONNECT_STATE:        {} (connected={:#x})",
                    phase(c.connect_status),
                    c.connected_mask
                );
                // The DRM view: connector ids GETRESOURCES/GETCONNECTOR now
                // serve for this GPU (real topology, not the 1001 stub).
                if c.supported_status == 0 && c.display_mask != 0 {
                    let _ = write!(s, "[gpuedid] DRM connectors:");
                    for b in 0..32u32 {
                        if c.display_mask & (1 << b) != 0 {
                            let _ = write!(
                                s,
                                " {}{}",
                                Self::rm_connector_id(device_instance, b),
                                if c.connected_mask & (1 << b) != 0 {
                                    "*"
                                } else {
                                    ""
                                }
                            );
                        }
                    }
                    let _ = writeln!(s, " (*=connected)");
                }
                if c.conn_type_count > 0 {
                    let _ = write!(s, "[gpuedid] connector types:");
                    let n = (c.conn_type_count as usize).min(c.conn_type_display_id.len());
                    for i in 0..n {
                        let bit = c.conn_type_display_id[i].trailing_zeros();
                        let _ = write!(
                            s,
                            " {}={}",
                            Self::rm_connector_id(device_instance, bit),
                            nv_conn_type_name(c.conn_type[i])
                        );
                    }
                    let _ = writeln!(s);
                }
                if c.connected_mask == 0 && c.connect_status == 0 {
                    let _ = writeln!(s, "[gpuedid] no monitor connected to this GPU's outputs (expected on the headless compute GPU)");
                } else if c.edid_status != 0xFFFF_FFFF {
                    let _ = writeln!(
                        s,
                        "[gpuedid] GET_EDID (id={:#x}):         {} ({} bytes, header {})",
                        c.edid_display_id,
                        phase(c.edid_status),
                        c.edid_size,
                        if c.edid_valid == 1 {
                            "VALID"
                        } else {
                            "invalid"
                        }
                    );
                    if c.edid_valid == 1 {
                        // EDID bytes 8-9 = PNP manufacturer id (5-bit packed letters); 10-11 = product code.
                        let m = ((c.edid_head[8] as u16) << 8) | c.edid_head[9] as u16;
                        let l1 = (b'A' - 1 + ((m >> 10) & 0x1f) as u8) as char;
                        let l2 = (b'A' - 1 + ((m >> 5) & 0x1f) as u8) as char;
                        let l3 = (b'A' - 1 + (m & 0x1f) as u8) as char;
                        let prod = ((c.edid_head[11] as u16) << 8) | c.edid_head[10] as u16;
                        let year = 1990u32 + c.edid_head[17] as u32;
                        let _ = writeln!(
                            s,
                            "[gpuedid] MONITOR: {}{}{} product={:#06x} year={} (EDID v{}.{})",
                            l1, l2, l3, prod, year, c.edid_head[18], c.edid_head[19]
                        );
                        let _ = write!(s, "[gpuedid] EDID head:");
                        for b in c.edid_head.iter() {
                            let _ = write!(s, " {:02x}", b);
                        }
                        let _ = writeln!(s);
                    }
                }
            }
            Err(status) => {
                let _ = writeln!(
                    s,
                    "[gpuedid] eclipse_rm_edid FAILED, NV_STATUS={:#x} (run /proc/gpuinit first)",
                    status
                );
            }
        }
        s
    }

    /// Step 18 (`/proc/gpustep18`): the first Eclipse-authored GPU
    /// execution. Writes a method stream (host semaphore RELEASE +
    /// SET_OBJECT(TURING_COMPUTE_A) + compute report semaphore RELEASE)
    /// into the step-17 pushbuffer, submits it (GP entry, GPPut, work-
    /// submit token, usermode doorbell) and CPU-polls both semaphore
    /// landing zones. Host sem OK = ESCHED/PBDMA fetched and ran our
    /// pushbuffer; engine sem OK = the compute engine context-switched
    /// into our channel and processed class methods. Requires gpustep17.
    fn bringup_step18(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let device_instance = *self.rm_device_instance.lock();
        let Some(device_instance) = device_instance else {
            return String::from(
                "[gpustep18] skipped (bring the GPU up first, then gpustep16/17)\n",
            );
        };
        nvidia_rm_sys::os_interface::capture_begin();
        let result = nvidia_rm_sys::rm_init::step18(device_instance);
        let captured = nvidia_rm_sys::os_interface::capture_take();
        if let Some(log) = captured {
            for line in log.lines() {
                let _ = writeln!(s, "[gpustep18] | {}", line);
            }
        }
        let phase = |st: u32| -> String {
            match st {
                0 => String::from("OK"),
                0xFFFF_FFFF => String::from("not reached"),
                0x65 => String::from("TIMEOUT (never landed)"),
                e => alloc::format!("FAILED NV_STATUS={:#x}", e),
            }
        };
        match result {
            Ok(l) => {
                let _ = writeln!(
                    s,
                    "[gpustep18] --- first Eclipse-authored submission on the live channel ---"
                );
                let _ = writeln!(
                    s,
                    "[gpustep18] lookup (chan/buf/USERD):  {}",
                    phase(l.lookup_status)
                );
                let _ = writeln!(
                    s,
                    "[gpustep18] CPU map (buf + USERD):    {}",
                    phase(l.map_status)
                );
                let _ = writeln!(
                    s,
                    "[gpustep18] work-submit token:        {} (token={:#010x}, runlist={})",
                    phase(l.token_status),
                    l.work_token,
                    l.runlist_id
                );
                let _ = writeln!(
                    s,
                    "[gpustep18] submit ({} dw + doorbell): {}",
                    l.push_dwords,
                    phase(l.submit_status)
                );
                let _ = writeln!(
                    s,
                    "[gpustep18] HOST semaphore (PBDMA):   {} (value={:#010x}, {} ms)",
                    phase(l.host_sem_status),
                    l.host_sem_value,
                    l.host_poll_iters
                );
                let _ = writeln!(
                    s,
                    "[gpustep18] ENGINE semaphore (compute FE): {} (value={:#010x}, {} ms)",
                    phase(l.eng_sem_status),
                    l.eng_sem_value,
                    l.eng_poll_iters
                );
                if l.host_sem_status == 0 && l.eng_sem_status == 0 {
                    let _ = writeln!(s, "[gpustep18] --- THE GPU RAN OUR PUSHBUFFER: PBDMA fetch + compute-engine context switch both proven; step19 = QMD + SASS kernel (real compute launch) ---");
                } else if l.host_sem_status == 0 {
                    let _ = writeln!(s, "[gpustep18] --- PBDMA ran our methods but the compute engine never reported: suspect ctxsw/golden-context or SET_OBJECT path ---");
                }
            }
            Err(status) => {
                let _ = writeln!(
                    s,
                    "[gpustep18] eclipse_rm_step18 FAILED, NV_STATUS={:#x} (run gpustep17 first in this boot; GPU state-loaded?)",
                    status
                );
            }
        }
        s
    }

    /// Step 19 (`/proc/gpustep19`): the first real compute launch. Builds a
    /// Turing (Volta V02_02) QMD pointing at a minimal SM75 EXIT kernel and
    /// submits it through the live step-17/18 channel via SEND_PCAS; the
    /// QMD's RELEASE0 semaphore landing in sysmem proves the SMs ran our
    /// program. Requires gpustep17 (and is happiest after gpustep18) first.
    fn bringup_step19(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let device_instance = *self.rm_device_instance.lock();
        let Some(device_instance) = device_instance else {
            return String::from(
                "[gpustep19] skipped (bring the GPU up first, then gpustep16/17)\n",
            );
        };
        nvidia_rm_sys::os_interface::capture_begin();
        let result = nvidia_rm_sys::rm_init::step19(device_instance);
        let captured = nvidia_rm_sys::os_interface::capture_take();
        if let Some(log) = captured {
            for line in log.lines() {
                let _ = writeln!(s, "[gpustep19] | {}", line);
            }
        }
        let phase = |st: u32| -> String {
            match st {
                0 => String::from("OK"),
                0xFFFF_FFFF => String::from("not reached"),
                0x65 => String::from("TIMEOUT (grid never released)"),
                e => alloc::format!("FAILED NV_STATUS={:#x}", e),
            }
        };
        match result {
            Ok(c) => {
                let _ = writeln!(
                    s,
                    "[gpustep19] --- first Eclipse-authored COMPUTE LAUNCH (QMD + SM75 kernel) ---"
                );
                let _ = writeln!(
                    s,
                    "[gpustep19] lookup (chan/buf/USERD):  {}",
                    phase(c.lookup_status)
                );
                let _ = writeln!(
                    s,
                    "[gpustep19] CPU map (buf + USERD):    {}",
                    phase(c.map_status)
                );
                let _ = writeln!(
                    s,
                    "[gpustep19] work-submit token:        {} (token={:#010x}, runlist={})",
                    phase(c.token_status),
                    c.work_token,
                    c.runlist_id
                );
                let _ = writeln!(
                    s,
                    "[gpustep19] QMD @ {:#x}, kernel @ {:#x}",
                    c.qmd_va, c.kernel_va
                );
                let _ = writeln!(
                    s,
                    "[gpustep19] launch ({} dw + SEND_PCAS + doorbell): {}",
                    c.push_dwords,
                    phase(c.submit_status)
                );
                let _ = writeln!(
                    s,
                    "[gpustep19] post-PCAS host fence:     {} (value={:#010x}, {} ms)",
                    phase(c.fence_status),
                    c.fence_value,
                    c.fence_iters
                );
                let _ = writeln!(
                    s,
                    "[gpustep19] QMD RELEASE0 semaphore:   {} (value={:#010x}, {} ms)",
                    phase(c.sem_status),
                    c.sem_value,
                    c.poll_iters
                );
                if c.sem_status == 0 {
                    let _ = writeln!(
                        s,
                        "[gpustep19] ============================================================"
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep19]  THE 34 SMs RAN OUR SASS KERNEL. Eclipse launched compute"
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep19]  on the TU106 and the grid completed. step20 = kernel that"
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep19]  stores a computed result to memory (params + STG)."
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep19] ============================================================"
                    );
                } else if c.fence_status == 0 {
                    let _ = writeln!(s, "[gpustep19] --- PBDMA consumed the whole compute stream (fence landed) but the grid never released: QMD scheduling or SM execution is stuck. The RM/GSP capture above should carry any SM exception. ---");
                } else if c.submit_status == 0 {
                    let _ = writeln!(s, "[gpustep19] --- doorbell rung but the post-PCAS fence never landed: the PBDMA did not consume the compute stream (channel faulted on an earlier method?). ---");
                }
            }
            Err(status) => {
                let _ = writeln!(
                    s,
                    "[gpustep19] eclipse_rm_step19 FAILED, NV_STATUS={:#x} (run gpustep17 first in this boot; GPU state-loaded?)",
                    status
                );
            }
        }
        s
    }

    /// Step 20 (`/proc/gpustep20`): the first kernel that computes an
    /// observable effect for Eclipse — MOV dest/value immediates (patched
    /// into the SASS at runtime) + STG.E.SYS + EXIT on the proven step-19
    /// QMD harness. Triple verification: post-PCAS fence, QMD RELEASE0,
    /// and CPU readback of the stored dword. Requires gpustep17 first.
    fn bringup_step20(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let device_instance = *self.rm_device_instance.lock();
        let Some(device_instance) = device_instance else {
            return String::from(
                "[gpustep20] skipped (bring the GPU up first, then gpustep16/17)\n",
            );
        };
        nvidia_rm_sys::os_interface::capture_begin();
        let result = nvidia_rm_sys::rm_init::step20(device_instance);
        let captured = nvidia_rm_sys::os_interface::capture_take();
        if let Some(log) = captured {
            for line in log.lines() {
                let _ = writeln!(s, "[gpustep20] | {}", line);
            }
        }
        let phase = |st: u32| -> String {
            match st {
                0 => String::from("OK"),
                0xFFFF_FFFF => String::from("not reached"),
                0x65 => String::from("TIMEOUT"),
                e => alloc::format!("FAILED NV_STATUS={:#x}", e),
            }
        };
        match result {
            Ok(c) => {
                let _ = writeln!(s, "[gpustep20] --- kernel STORE: GPU writes a value we chose to memory we chose ---");
                let _ = writeln!(
                    s,
                    "[gpustep20] lookup / CPU map:         {} / {}",
                    phase(c.lookup_status),
                    phase(c.map_status)
                );
                let _ = writeln!(
                    s,
                    "[gpustep20] token:                    {} (token={:#010x}, runlist={})",
                    phase(c.token_status),
                    c.work_token,
                    c.runlist_id
                );
                let _ = writeln!(
                    s,
                    "[gpustep20] QMD @ {:#x}, kernel @ {:#x}, dest @ {:#x}",
                    c.qmd_va, c.kernel_va, c.dest_va
                );
                let _ = writeln!(
                    s,
                    "[gpustep20] launch ({} dw):            {}",
                    c.push_dwords,
                    phase(c.submit_status)
                );
                let _ = writeln!(
                    s,
                    "[gpustep20] post-PCAS host fence:     {} (value={:#010x}, {} ms)",
                    phase(c.fence_status),
                    c.fence_value,
                    c.fence_iters
                );
                let _ = writeln!(
                    s,
                    "[gpustep20] QMD RELEASE0 semaphore:   {} (value={:#010x}, {} ms)",
                    phase(c.sem_status),
                    c.sem_value,
                    c.sem_iters
                );
                let _ = writeln!(
                    s,
                    "[gpustep20] stored dword @ dest:      {} (value={:#010x}, expect 0xec0de520)",
                    phase(c.store_status),
                    c.store_value
                );
                if c.sem_status == 0 && c.store_status == 0 {
                    let _ = writeln!(
                        s,
                        "[gpustep20] ============================================================"
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep20]  THE GPU COMPUTED FOR ECLIPSE: our SASS ran on an SM and"
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep20]  stored our value at our address. MOV+STG+EXIT verified."
                    );
                    let _ = writeln!(s, "[gpustep20]  The compute bring-up ladder is COMPLETE.");
                    let _ = writeln!(
                        s,
                        "[gpustep20] ============================================================"
                    );
                } else if c.sem_status == 0 {
                    let _ = writeln!(s, "[gpustep20] --- grid completed but the store is missing/wrong: STG encoding or GMMU write path suspect ---");
                } else if c.fence_status == 0 {
                    let _ = writeln!(s, "[gpustep20] --- methods consumed but grid never released: MOV/STG encoding suspect (SM trap); RELEASE0 did not land ---");
                }
            }
            Err(status) => {
                let _ = writeln!(
                    s,
                    "[gpustep20] eclipse_rm_step20 FAILED, NV_STATUS={:#x} (run gpustep17 first in this boot)",
                    status
                );
            }
        }
        s
    }

    /// Step 21 (`/proc/gpustep21`): multi-thread computation — 32 threads
    /// each compute out[tid] = tid*3+7 (S2R thread-ID with real write-
    /// barrier scoreboarding, IMAD math, IMAD.WIDE per-thread addressing,
    /// per-thread STG), CPU-verifies all 32 slots. Requires gpustep17.
    fn bringup_step21(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let device_instance = *self.rm_device_instance.lock();
        let Some(device_instance) = device_instance else {
            return String::from(
                "[gpustep21] skipped (bring the GPU up first, then gpustep16/17)\n",
            );
        };
        nvidia_rm_sys::os_interface::capture_begin();
        let result = nvidia_rm_sys::rm_init::step21(device_instance);
        let captured = nvidia_rm_sys::os_interface::capture_take();
        if let Some(log) = captured {
            for line in log.lines() {
                let _ = writeln!(s, "[gpustep21] | {}", line);
            }
        }
        let phase = |st: u32| -> String {
            match st {
                0 => String::from("OK"),
                0xFFFF_FFFF => String::from("not reached"),
                0x65 => String::from("TIMEOUT"),
                e => alloc::format!("FAILED NV_STATUS={:#x}", e),
            }
        };
        match result {
            Ok(c) => {
                let _ = writeln!(
                    s,
                    "[gpustep21] --- 32-THREAD kernel: out[tid] = tid*3 + 7 ---"
                );
                let _ = writeln!(
                    s,
                    "[gpustep21] lookup / CPU map:         {} / {}",
                    phase(c.lookup_status),
                    phase(c.map_status)
                );
                let _ = writeln!(
                    s,
                    "[gpustep21] token:                    {} (token={:#010x}, runlist={})",
                    phase(c.token_status),
                    c.work_token,
                    c.runlist_id
                );
                let _ = writeln!(
                    s,
                    "[gpustep21] QMD @ {:#x}, kernel @ {:#x}, out[] @ {:#x}",
                    c.qmd_va, c.kernel_va, c.out_va
                );
                let _ = writeln!(
                    s,
                    "[gpustep21] launch ({} dw):            {}",
                    c.push_dwords,
                    phase(c.submit_status)
                );
                let _ = writeln!(
                    s,
                    "[gpustep21] post-PCAS host fence:     {} ({} ms)",
                    phase(c.fence_status),
                    c.fence_iters
                );
                let _ = writeln!(
                    s,
                    "[gpustep21] QMD RELEASE0 semaphore:   {} ({} ms)",
                    phase(c.sem_status),
                    c.sem_iters
                );
                let _ = writeln!(
                    s,
                    "[gpustep21] per-thread verification:  {} ({}/32 slots correct)",
                    phase(c.verify_status),
                    c.match_count
                );
                if c.first_bad_idx != 0xFFFF_FFFF {
                    let _ = writeln!(
                        s,
                        "[gpustep21] first mismatch: out[{}]={:#010x} (expected {:#x})",
                        c.first_bad_idx,
                        c.first_bad_val,
                        3 * c.first_bad_idx + 7
                    );
                }
                if c.sem_status == 0 && c.verify_status == 0 {
                    let _ = writeln!(
                        s,
                        "[gpustep21] ============================================================"
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep21]  32 THREADS, 32 CORRECT RESULTS: per-thread IDs, integer"
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep21]  math, per-thread addressing and stores, and real Volta"
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep21]  scoreboarding all verified. Eclipse now runs real"
                    );
                    let _ = writeln!(s, "[gpustep21]  parallel compute on the TU106.");
                    let _ = writeln!(
                        s,
                        "[gpustep21] ============================================================"
                    );
                } else if c.sem_status == 0 {
                    let _ = writeln!(s, "[gpustep21] --- grid completed but results are wrong: math/addressing path suspect (check first mismatch above) ---");
                } else if c.fence_status == 0 {
                    let _ = writeln!(s, "[gpustep21] --- methods consumed but grid never released: S2R/IMAD encoding or scoreboard suspect (SM trap) ---");
                }
            }
            Err(status) => {
                let _ = writeln!(
                    s,
                    "[gpustep21] eclipse_rm_step21 FAILED, NV_STATUS={:#x} (run gpustep17 first in this boot)",
                    status
                );
            }
        }
        s
    }

    /// Step 22 (`/proc/gpustep22`): chip-scale grid — 68 CTAs x 32 threads
    /// = 2176 threads across all 34 SMs (two waves), out[gid] = gid*3+7
    /// with gid = ctaid*32 + tid, all 2176 slots CPU-verified.
    fn bringup_step22(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let device_instance = *self.rm_device_instance.lock();
        let Some(device_instance) = device_instance else {
            return String::from(
                "[gpustep22] skipped (bring the GPU up first, then gpustep16/17)\n",
            );
        };
        nvidia_rm_sys::os_interface::capture_begin();
        let result = nvidia_rm_sys::rm_init::step22(device_instance);
        let captured = nvidia_rm_sys::os_interface::capture_take();
        if let Some(log) = captured {
            for line in log.lines() {
                let _ = writeln!(s, "[gpustep22] | {}", line);
            }
        }
        let phase = |st: u32| -> String {
            match st {
                0 => String::from("OK"),
                0xFFFF_FFFF => String::from("not reached"),
                0x65 => String::from("TIMEOUT"),
                e => alloc::format!("FAILED NV_STATUS={:#x}", e),
            }
        };
        match result {
            Ok(c) => {
                let _ = writeln!(
                    s,
                    "[gpustep22] --- CHIP-SCALE grid: 68 CTAs x 32 threads over all 34 SMs ---"
                );
                let _ = writeln!(
                    s,
                    "[gpustep22] lookup / CPU map:         {} / {}",
                    phase(c.lookup_status),
                    phase(c.map_status)
                );
                let _ = writeln!(
                    s,
                    "[gpustep22] token:                    {} (token={:#010x}, runlist={})",
                    phase(c.token_status),
                    c.work_token,
                    c.runlist_id
                );
                let _ = writeln!(
                    s,
                    "[gpustep22] QMD @ {:#x}, kernel @ {:#x}, out[] @ {:#x}",
                    c.qmd_va, c.kernel_va, c.out_va
                );
                let _ = writeln!(
                    s,
                    "[gpustep22] launch ({} dw):            {}",
                    c.push_dwords,
                    phase(c.submit_status)
                );
                let _ = writeln!(
                    s,
                    "[gpustep22] post-PCAS host fence:     {} ({} ms)",
                    phase(c.fence_status),
                    c.fence_iters
                );
                let _ = writeln!(
                    s,
                    "[gpustep22] QMD RELEASE0 semaphore:   {} ({} ms)",
                    phase(c.sem_status),
                    c.sem_iters
                );
                let _ = writeln!(
                    s,
                    "[gpustep22] per-thread verification:  {} ({}/2176 slots correct)",
                    phase(c.verify_status),
                    c.match_count
                );
                if c.first_bad_idx != 0xFFFF_FFFF {
                    let _ = writeln!(
                        s,
                        "[gpustep22] first mismatch: out[{}]={:#010x} (expected {:#x})",
                        c.first_bad_idx,
                        c.first_bad_val,
                        3 * c.first_bad_idx + 7
                    );
                }
                if c.sem_status == 0 && c.verify_status == 0 {
                    let _ = writeln!(
                        s,
                        "[gpustep22] ============================================================"
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep22]  2176 THREADS, 68 CTAs, ALL 34 SMs: the whole TU106 chip"
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep22]  computed for Eclipse in one dispatch and every result"
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep22]  verified. Chip-scale parallel compute is proven."
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep22] ============================================================"
                    );
                } else if c.sem_status == 0 {
                    let _ = writeln!(s, "[gpustep22] --- grid completed but results wrong (check first mismatch: CTA scheduling/addressing suspect) ---");
                } else if c.fence_status == 0 {
                    let _ = writeln!(s, "[gpustep22] --- methods consumed but grid never released: multi-CTA dispatch suspect ---");
                }
            }
            Err(status) => {
                let _ = writeln!(
                    s,
                    "[gpustep22] eclipse_rm_step22 FAILED, NV_STATUS={:#x} (run gpustep17 first in this boot)",
                    status
                );
            }
        }
        s
    }

    /// Step 23 (`/proc/gpustep23`): integer SAXPY — 32 threads each load
    /// x[tid] and y[tid] from GPU arrays, compute y = a*x + y (LDG global
    /// loads + IMAD + STG), CPU-verified per element. The load-compute-
    /// store canon; the first kernel that reads from memory.
    fn bringup_step23(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let device_instance = *self.rm_device_instance.lock();
        let Some(device_instance) = device_instance else {
            return String::from(
                "[gpustep23] skipped (bring the GPU up first, then gpustep16/17)\n",
            );
        };
        nvidia_rm_sys::os_interface::capture_begin();
        let result = nvidia_rm_sys::rm_init::step23(device_instance);
        let captured = nvidia_rm_sys::os_interface::capture_take();
        if let Some(log) = captured {
            for line in log.lines() {
                let _ = writeln!(s, "[gpustep23] | {}", line);
            }
        }
        let phase = |st: u32| -> String {
            match st {
                0 => String::from("OK"),
                0xFFFF_FFFF => String::from("not reached"),
                0x65 => String::from("TIMEOUT"),
                e => alloc::format!("FAILED NV_STATUS={:#x}", e),
            }
        };
        match result {
            Ok(c) => {
                let _ = writeln!(s, "[gpustep23] --- integer SAXPY: y[i] = 3*x[i] + y[i], x[i]=0x1000+i, y[i]=100+i ---");
                let _ = writeln!(
                    s,
                    "[gpustep23] lookup / CPU map:         {} / {}",
                    phase(c.lookup_status),
                    phase(c.map_status)
                );
                let _ = writeln!(
                    s,
                    "[gpustep23] token:                    {} (token={:#010x}, runlist={})",
                    phase(c.token_status),
                    c.work_token,
                    c.runlist_id
                );
                let _ = writeln!(
                    s,
                    "[gpustep23] QMD @ {:#x}, kernel @ {:#x}, y[] @ {:#x}",
                    c.qmd_va, c.kernel_va, c.out_va
                );
                let _ = writeln!(
                    s,
                    "[gpustep23] launch ({} dw):            {}",
                    c.push_dwords,
                    phase(c.submit_status)
                );
                let _ = writeln!(
                    s,
                    "[gpustep23] post-PCAS host fence:     {} ({} ms)",
                    phase(c.fence_status),
                    c.fence_iters
                );
                let _ = writeln!(
                    s,
                    "[gpustep23] QMD RELEASE0 semaphore:   {} ({} ms)",
                    phase(c.sem_status),
                    c.sem_iters
                );
                let _ = writeln!(
                    s,
                    "[gpustep23] SAXPY verification:       {} ({}/32 elements = 0x3064+4i)",
                    phase(c.verify_status),
                    c.match_count
                );
                if c.fault_ctrl_status != 0xFFFF_FFFF {
                    let _ = writeln!(
                        s,
                        "[gpustep23] MMU fault query:          ctrl={:#x} addr={:#x}_{:08x} type={:#x}",
                        c.fault_ctrl_status, c.fault_addr_hi, c.fault_addr_lo, c.fault_type
                    );
                }
                if c.first_bad_idx != 0xFFFF_FFFF {
                    let _ = writeln!(
                        s,
                        "[gpustep23] first mismatch: y[{}]={:#x} ({}) expected {:#x}",
                        c.first_bad_idx,
                        c.first_bad_val,
                        c.first_bad_val,
                        0x3064 + 4 * c.first_bad_idx
                    );
                }
                if c.sem_status == 0 && c.verify_status == 0 {
                    let _ = writeln!(
                        s,
                        "[gpustep23] ============================================================"
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep23]  LOAD-COMPUTE-STORE PROVEN: the GPU read two arrays from"
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep23]  memory, did a*x+y per element, and wrote the results back."
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep23]  Eclipse has the full GPU compute primitive."
                    );
                    let _ = writeln!(
                        s,
                        "[gpustep23] ============================================================"
                    );
                } else if c.sem_status == 0 {
                    let _ = writeln!(s, "[gpustep23] --- grid completed but results wrong: LDG address/data path suspect (check first mismatch) ---");
                } else if c.fence_status == 0 {
                    let _ = writeln!(s, "[gpustep23] --- methods consumed but grid never released: LDG encoding or load scoreboard suspect (SM trap) ---");
                }
            }
            Err(status) => {
                let _ = writeln!(
                    s,
                    "[gpustep23] eclipse_rm_step23 FAILED, NV_STATUS={:#x} (run gpustep17 first in this boot)",
                    status
                );
            }
        }
        s
    }

    /// `/proc/gpubench`: integer-ALU GIOPS benchmark — a big grid of
    /// dependent-IMAD chains timed by the GPU PTIMER. GIOPS is computed
    /// here (u128) to avoid a 64-bit divide in the C.
    fn bringup_bench(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let device_instance = *self.rm_device_instance.lock();
        let Some(device_instance) = device_instance else {
            return String::from("[gpubench] skipped (run /proc/gpuinit first)\n");
        };
        nvidia_rm_sys::os_interface::capture_begin();
        let result = nvidia_rm_sys::rm_init::bench(device_instance);
        let captured = nvidia_rm_sys::os_interface::capture_take();
        if let Some(log) = captured {
            for line in log.lines() {
                let _ = writeln!(s, "[gpubench] | {}", line);
            }
        }
        let phase = |st: u32| -> String {
            match st {
                0 => String::from("OK"),
                0xFFFF_FFFF => String::from("not reached"),
                e => alloc::format!("FAILED NV_STATUS={:#x}", e),
            }
        };
        match result {
            Ok(c) => {
                let _ = writeln!(
                    s,
                    "[gpubench] --- integer-ALU throughput (IMAD.U32 dependent chain) ---"
                );
                let _ = writeln!(
                    s,
                    "[gpubench] lookup/map:   {} / {}",
                    phase(c.lookup_status),
                    phase(c.map_status)
                );
                let _ = writeln!(
                    s,
                    "[gpubench] launch ({} dw): {}",
                    c.push_dwords,
                    phase(c.submit_status)
                );
                let _ = writeln!(
                    s,
                    "[gpubench] grid:         {} threads x {} IMAD = {} ops",
                    c.num_threads, c.imads_per_thread, c.total_ops
                );
                let _ = writeln!(
                    s,
                    "[gpubench] timestamp sem: {} (@{} ms)",
                    phase(c.sem_status),
                    c.sem_iters
                );
                if c.sem_status == 0 && c.elapsed_ns > 0 {
                    // GIOPS = total_ops / elapsed_ns (ops/ns == giga-ops/s).
                    // x1000 for three decimals, u128 to avoid overflow.
                    let giops_milli = (c.total_ops as u128 * 1000u128) / (c.elapsed_ns as u128);
                    let _ = writeln!(
                        s,
                        "[gpubench] elapsed:      {} ns ({}.{:03} ms)",
                        c.elapsed_ns,
                        c.elapsed_ns / 1_000_000,
                        (c.elapsed_ns / 1000) % 1000
                    );
                    let _ = writeln!(
                        s,
                        "[gpubench] ============================================================"
                    );
                    let _ = writeln!(
                        s,
                        "[gpubench]  {}.{:03} GIOPS (integer multiply-add) on the RTX 2060 Super",
                        giops_milli / 1000,
                        giops_milli % 1000
                    );
                    let _ = writeln!(
                        s,
                        "[gpubench] ============================================================"
                    );
                } else if c.sem_status == 0 {
                    let _ = writeln!(
                        s,
                        "[gpubench] grid ran but timestamps were zero (t0={:#x} t1={:#x})",
                        c.t0_ns, c.t1_ns
                    );
                } else {
                    let _ = writeln!(
                        s,
                        "[gpubench] --- grid did not signal within poll window ---"
                    );
                }
            }
            Err(status) => {
                let _ = writeln!(
                    s,
                    "[gpubench] eclipse_rm_bench FAILED, NV_STATUS={:#x} (run /proc/gpuinit first)",
                    status
                );
            }
        }
        s
    }

    /// Step 2: instance block + GMMU flush — the first GPU register writes.
    /// TEMPORARY: the secondary (non-console) GPU has its own unrelated
    /// problems (USB breaks in Eclipse when it's made primary; likely never
    /// got a VBIOS devinit replay since it's never POSTed), so for now we
    /// target the ONLY GPU available — the one driving the console — and
    /// skip the other one instead. This trades away the original safety net
    /// (a hang here now means losing the only display and a hard reboot);
    /// the user has explicitly accepted that risk. Opt-in (`/proc/gpustep2`).
    fn bringup_step2(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        if !self.drives_boot_display() {
            let _ = writeln!(
                s,
                "[gpustep2] {} ({}) SKIPPED — not the console GPU (bar1_phys={:#x}); only testing the single available GPU",
                self.name, self.gpu_model, self.bar1_phys
            );
            return s;
        }

        let mut g = self.bringup.lock();
        if g.is_none() {
            *g = GpuBringup::build(0x0020_0000, 0x0300_0000);
        }
        let b = match g.as_ref() {
            Some(b) => b,
            None => {
                let _ = writeln!(s, "[gpustep2] {} alloc_coherent FAILED", self.name);
                return s;
            }
        };

        let _ = writeln!(
            s,
            "[gpustep2] === {} ({}) — Step 2: instance block + GMMU flush ===",
            self.name, self.gpu_model
        );

        // Part 1: read-only PRAMIN accessibility ladder. PRAMIN works (rt@0
        // round-tripped) but VRAM at 2 GiB read back the 0xBAD0ACxx PRI-error
        // sentinel, so probe which offsets the window actually reaches. An
        // inaccessible offset reads the sentinel; real VRAM does not. No writes.
        let ladder = [
            ("0", 0u64),
            ("1M", 0x10_0000),
            ("4M", 0x40_0000),
            ("16M", 0x100_0000),
            ("64M", 0x400_0000),
            ("256M", 0x1000_0000),
            ("512M", 0x2000_0000),
            ("1G", 0x4000_0000),
            ("2G", 0x8000_0000),
        ];
        let _ = write!(s, "[gpustep2]  PRAMIN ladder:");
        let mut last_ok = 0u64;
        for (name, off) in ladder {
            let v = self.pramin_r32(off);
            let bad = (v & 0xFFFF_FF00) == 0xBAD0_AC00;
            if !bad {
                last_ok = off;
            }
            let _ = write!(s, " {}={}", name, if bad { "BAD" } else { "ok" });
        }
        let _ = writeln!(s, " (highest ok={:#x})", last_ok);

        let inst = b.inst_vram();
        let st = self.pramin_r32(inst);
        let pramin_ok = (st & 0xFFFF_FF00) != 0xBAD0_AC00;
        self.write_instance_block_vram(b);
        let rb = |off: u64| self.pramin_r32(inst + off);
        let _ = writeln!(
            s,
            "[gpustep2]  PRAMIN self-test={} inst@VRAM {:#x}",
            pramin_ok, inst
        );
        let _ = writeln!(
            s,
            "[gpustep2]  inst@0x200={:08x}{:08x} @0x208={:08x}{:08x} userd@0x008={:08x}{:08x}",
            rb(0x204),
            rb(0x200),
            rb(0x20c),
            rb(0x208),
            rb(0x00c),
            rb(0x008)
        );
        let _ = writeln!(
            s,
            "[gpustep2]  CE ctx (disarmed): inst@0x220={:08x}{:08x} @0x0ac={:08x}",
            rb(0x224),
            rb(0x220),
            rb(0x0ac),
        );
        // Arm the HUB MMU fault buffer (the likely root cause) and report it.
        let (fb_count, fb_lo, fb_hi, fb_size) = self.setup_fault_buffer(b);
        let _ = writeln!(
            s,
            "[gpustep2]  FAULT_BUF: hw_count={:#x} buf_phys={:#x} LO(0xb83000)={:#010x} HI={:#010x} SIZE(0xb83010)={:#010x}",
            fb_count,
            b.fault_buf.paddr(),
            fb_lo,
            fb_hi,
            fb_size
        );
        // Make BAR2 live and report the bind, plus the PCE map (CE buffer size).
        let (b2_before, b2_after, b2_wait) = self.setup_bar2(b);
        let pce_map = unsafe { core::ptr::read_volatile((self._bar0 + 0x0010_4028) as *const u32) };
        let _ = writeln!(
            s,
            "[gpustep2]  BAR2(0xb80f48) {:#010x}->{:#010x} wait(0xb80f50)={:#010x} PCE_MAP(0x104028)={:#010x}",
            b2_before, b2_after, b2_wait, pce_map
        );

        // Part 2: the only GPU register write — flush our PDB.
        let root_phys = b.root.paddr() as u64;
        let (pre, post, ok) = self.gmmu_flush(root_phys);
        let _ = writeln!(
            s,
            "[gpustep2]  flush PDB=(root>>8)={:#x}  trigger(0xb830b0) pre={:#010x} post={:#010x} bit31_cleared={}",
            root_phys >> 8,
            pre,
            post,
            ok
        );
        if ok {
            let _ = writeln!(
                s,
                "[gpustep2]  OK — GMMU accepted the PDB, MMU not wedged. Ready for Step 3 (runlist + doorbell)."
            );
        } else if pre & 0x8000_0000 != 0 {
            let _ = writeln!(
                s,
                "[gpustep2]  ABORTED — a flush was already in flight (bit31 set); no write performed."
            );
        } else {
            let _ = writeln!(
                s,
                "[gpustep2]  TIMEOUT — bit31 never cleared. Suspect bad PDB; inspect /proc/gpudbg fault regs (do NOT re-trigger)."
            );
        }
        s
    }

    /// Step 3: doorbell-enable + runlist commit + channel enable (empty GPFIFO).
    /// Auto-skips the console GPU. Opt-in (`/proc/gpustep3`). Requires Step 2 to
    /// have built the instance block; runs it here if not already done.
    fn bringup_step3(&self) -> String {
        use core::fmt::Write;
        // runlist 0 (GR/CE runlist) and channel 0.
        const RUNL_ID: u32 = 0;
        const CHID: u32 = 0;

        let mut s = String::new();
        // TEMPORARY: targeting the console GPU instead of skipping it — see
        // the comment on bringup_step2 for why.
        if !self.drives_boot_display() {
            let _ = writeln!(
                s,
                "[gpustep3] {} SKIPPED — not the console GPU; only testing the single available GPU",
                self.name
            );
            return s;
        }

        let mut g = self.bringup.lock();
        if g.is_none() {
            *g = GpuBringup::build(0x0020_0000, 0x0300_0000);
        }
        let b = match g.as_ref() {
            Some(b) => b,
            None => {
                let _ = writeln!(s, "[gpustep3] {} alloc_coherent FAILED", self.name);
                return s;
            }
        };

        let _ = writeln!(
            s,
            "[gpustep3] === {} ({}) — Step 3: doorbell + runlist commit (empty GPFIFO) ===",
            self.name, self.gpu_model
        );

        // Ensure the instance block + runlist exist in VRAM (idempotent).
        self.write_instance_block_vram(b);
        self.write_runlist_vram(b);
        let runlist_vram = b.runlist_vram();

        let bar0 = self._bar0;
        let rd =
            |off: u32| unsafe { core::ptr::read_volatile((bar0 + off as usize) as *const u32) };
        let wr = |off: u32, v: u32| unsafe {
            core::ptr::write_volatile((bar0 + off as usize) as *mut u32, v)
        };

        // 1) Enable the doorbell (mask bit31).
        let db_before = rd(0x00b6_5000);
        wr(0x00b6_5000, db_before | 0x8000_0000);
        let db_after = rd(0x00b6_5000);
        let _ = writeln!(
            s,
            "[gpustep3]  doorbell-en(0xb65000) {:#010x} -> {:#010x} (bit31={})",
            db_before,
            db_after,
            db_after >> 31
        );

        // 2) Commit the runlist (base lo/hi + count=2 submits; poll bit15).
        let base = 0x0000_2b00 + RUNL_ID * 0x10;
        wr(base, runlist_vram as u32);
        wr(base + 4, (runlist_vram >> 32) as u32);
        wr(base + 8, 2); // 2 entries (cgrp + chan) — this write submits
        let mut cfg_post = rd(base + 0xc);
        let mut commit_ok = false;
        for _ in 0..5_000_000u64 {
            cfg_post = rd(base + 0xc);
            if cfg_post & 0x0000_8000 == 0 {
                commit_ok = true;
                break;
            }
            core::hint::spin_loop();
        }
        let _ = writeln!(
            s,
            "[gpustep3]  runlist@{:#x} commit RUNL{} cfg(0x{:x})={:#010x} pending_cleared={}",
            runlist_vram,
            RUNL_ID,
            base + 0xc,
            cfg_post,
            commit_ok
        );

        // 3) Enable the channel in the scheduler (mask 0x400).
        let ce = 0x0080_0004 + CHID * 8;
        let chan_before = rd(ce);
        wr(ce, chan_before | 0x0000_0400);
        let chan_after = rd(ce);
        let _ = writeln!(
            s,
            "[gpustep3]  chan{}-cfg(0x{:x}) {:#010x} -> {:#010x}",
            CHID, ce, chan_before, chan_after
        );

        if commit_ok {
            let _ = writeln!(
                s,
                "[gpustep3]  OK — scheduler accepted the runlist, no fault. Ready for Step 4 (ring doorbell, empty PB)."
            );
        } else {
            let _ = writeln!(
                s,
                "[gpustep3]  TIMEOUT — runlist pending bit never cleared. Inspect /proc/gpudbg; runl_id 0 may be wrong (do NOT re-commit)."
            );
        }
        s
    }

    /// Step 4: ring the doorbell with a SET_OBJECT(0xC5B5) pushbuffer. Exercises
    /// doorbell -> PBDMA -> GMMU-translated pushbuffer fetch -> method parse.
    /// Auto-skips the console GPU. Opt-in (`/proc/gpustep4`).
    fn bringup_step4(&self) -> String {
        use core::fmt::Write;
        const CHID: u32 = 0;

        let mut s = String::new();
        // TEMPORARY: targeting the console GPU instead of skipping it — see
        // the comment on bringup_step2 for why.
        if !self.drives_boot_display() {
            let _ = writeln!(
                s,
                "[gpustep4] {} SKIPPED — not the console GPU; only testing the single available GPU",
                self.name
            );
            return s;
        }

        let mut g = self.bringup.lock();
        if g.is_none() {
            *g = GpuBringup::build(0x0020_0000, 0x0300_0000);
        }
        let b = match g.as_ref() {
            Some(b) => b,
            None => {
                let _ = writeln!(s, "[gpustep4] {} alloc_coherent FAILED", self.name);
                return s;
            }
        };

        let _ = writeln!(
            s,
            "[gpustep4] === {} ({}) — Step 4: ring doorbell with SET_OBJECT(0xC5B5) ===",
            self.name, self.gpu_model
        );

        // PMC_ENABLE before/after: confirms whether FIFO (mask 0x100) was
        // actually sitting in reset before setup_channel's reset pulse.
        let pmc_pre = unsafe { core::ptr::read_volatile((self._bar0 + 0x0000_0200) as *const u32) };

        // Bring the channel live (idempotent; covers a fresh boot). Volta+
        // gives every engine its OWN runlist id, discovered via PTOP — using
        // a hardcoded runlist 0 was an unverified assumption (it might
        // belong to GR instead of CE); setup_channel now discovers the
        // actual CE runlist id and commits to that.
        let (commit_ok, runl_id) = self.setup_channel(b);
        let pmc_post =
            unsafe { core::ptr::read_volatile((self._bar0 + 0x0000_0200) as *const u32) };
        let _ = writeln!(
            s,
            "[gpustep4]  PMC_ENABLE(0x200) pre={:#010x} post={:#010x} (FIFO bit 0x100: pre={} post={})",
            pmc_pre,
            pmc_post,
            (pmc_pre >> 8) & 1,
            (pmc_post >> 8) & 1
        );
        let ce = self.find_ce_runlist();
        let engine_id = ce.map(|(_, e)| e).unwrap_or(u32::MAX);
        let _ = writeln!(
            s,
            "[gpustep4]  PTOP-discovered CE runlist id={} engine_id={} (fallback-to-0={}) channel setup: runlist_commit={}",
            runl_id,
            engine_id,
            ce.is_none(),
            commit_ok
        );
        let _ = writeln!(s, "[gpustep4]  PTOP entries:{}", self.ptop_report());

        // PCE_MAP (0x104028): maps each LOGICAL copy engine (what PTOP/runlist
        // enumerate, e.g. our engine_id=8) to a PHYSICAL copy engine, or marks
        // it unmapped. Already read in bringup_step2 but never shown here —
        // across two real-hardware runs PBDMA9 (runl8's PBDMA) was COMPLETELY
        // inert (its aggregate PFIFO_PBDMA_STATUS read bit-for-bit identical
        // both times, unlike PBDMA0/1 which changed), i.e. the host scheduler
        // never touched it even once. If engine_id=8's nibble here reads as
        // the unmapped sentinel, that would explain why nothing ever gets
        // scheduled regardless of how correctly the runlist/channel is set up.
        let pce_map = unsafe { core::ptr::read_volatile((self._bar0 + 0x0010_4028) as *const u32) };
        let _ = writeln!(
            s,
            "[gpustep4]  PCE_MAP(0x104028)={:#010x} (raw; per-LCE nibble layout not yet decoded)",
            pce_map
        );

        // Real nouveau (nvkm subdev/devinit/tu102.c, tu102_devinit_wait): on
        // Turing+, devinit's VBIOS init-table execution runs on a HARDWARE
        // sequencer automatically at POST, before any OS/driver runs at all.
        // The host driver's only job is to *wait* for it, checking exactly:
        //   (rd(0x118128) & 1) != 0 && (rd(0x118234) & 0xff) == 0xff
        // We have NEVER checked this. If it never completed (e.g. this OS's
        // boot path skipped something a full firmware POST normally does),
        // downstream engines could be left un-floorplanned/un-clocked —
        // which would explain a logical CE that never faults, never shows
        // scheduler activity, and whose PBDMA is never touched at all,
        // regardless of how correctly we set up the channel/runlist on top.
        // Read-only; safe to check every time.
        let di_128 = unsafe { core::ptr::read_volatile((self._bar0 + 0x0011_8128) as *const u32) };
        let di_234 = unsafe { core::ptr::read_volatile((self._bar0 + 0x0011_8234) as *const u32) };
        let devinit_done = (di_128 & 1) != 0 && (di_234 & 0xff) == 0xff;
        let _ = writeln!(
            s,
            "[gpustep4]  DEVINIT_WAIT: 0x118128={:#010x}(bit0={}) 0x118234={:#010x}(low8={:#04x}) devinit_done={}",
            di_128,
            di_128 & 1,
            di_234,
            di_234 & 0xff,
            devinit_done
        );

        // NV_PFIFO_SCHED_STATUS (0x263c): global scheduler status — is the
        // runlist-fetch unit even busy/idle, is a channel switch in
        // progress. NV_PFIFO_ENGINE_STATUS(engine_id) (0x2640+id*8): the
        // per-ENGINE (a third id space, distinct from runlist id and PBDMA
        // index) scheduler status — CTX_STATUS, FAULTED, ENGINE busy/idle,
        // currently-loaded ID. Neither had ever been read before.
        let sched_status =
            unsafe { core::ptr::read_volatile((self._bar0 + 0x0000_263c) as *const u32) };
        let _ = writeln!(
            s,
            "[gpustep4]  SCHED_STATUS(0x263c)={:#010x} chsw_in_progress={} runlist_fetch_busy={}",
            sched_status,
            (sched_status >> 1) & 1,
            (sched_status >> 2) & 1
        );
        if engine_id != u32::MAX {
            let eoff = engine_id as usize * 8;
            let eng_status = unsafe {
                core::ptr::read_volatile((self._bar0 + 0x0000_2640 + eoff) as *const u32)
            };
            let eng_debug = unsafe {
                core::ptr::read_volatile((self._bar0 + 0x0000_2644 + eoff) as *const u32)
            };
            let _ = writeln!(
                s,
                "[gpustep4]  ENGINE_STATUS(engine{})={:#010x} ctx_status={} id={:#x} id_type={} engine_busy={} faulted={} eng_reload={}  DEBUG={:#010x}",
                engine_id,
                eng_status,
                (eng_status >> 13) & 0x7,
                eng_status & 0xfff,
                (eng_status >> 12) & 1,
                (eng_status >> 31) & 1,
                (eng_status >> 30) & 1,
                (eng_status >> 29) & 1,
                eng_debug
            );
        }

        // Build the method stream (sysmem pushbuffer) + a GPFIFO launch entry at
        // the current PUT slot. The GPFIFO entry points at the pushbuffer GPU VA.
        let n = b.write_setobject_pushbuffer();
        let pb_va = b.va_base + 0x3000;
        // USERD lives in VRAM — GP_PUT/GP_GET are accessed via PRAMIN.
        let userd = b.userd_vram();
        let put_before = self.pramin_r32(userd + 0x8c);
        let get_before = self.pramin_r32(userd + 0x88);
        let ring_entries = (b.gpfifo.byte_len() / 8) as u32;
        let slot = (put_before % ring_entries) as usize;
        b.write_gpfifo_entry(slot, pb_va, n);
        let target = put_before + 1;

        // Clear any latched MMU fault so the one we read after is OURS, not
        // stale (write bit31 to the fault-clear reg 0xb83094).
        unsafe { core::ptr::write_volatile((self._bar0 + 0x00b8_3094) as *mut u32, 0x8000_0000) };

        // PFIFO_INTR_0 before the ring — did a prior interrupt condition
        // latch (e.g. a scheduler/runlist-update completion) that we never
        // acked, possibly stalling forward progress.
        let intr0_pre =
            unsafe { core::ptr::read_volatile((self._bar0 + 0x0000_2100) as *const u32) };

        // Advance GP_PUT (VRAM USERD, via PRAMIN), fence, ring the doorbell.
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.pramin_w32(userd + 0x8c, target);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let token = (runl_id << 16) | CHID;
        unsafe { core::ptr::write_volatile((self._bar0 + 0x00bb_0090) as *mut u32, token) };

        // Poll GP_GET (VRAM USERD) catching up to GP_PUT.
        let mut get_after = get_before;
        let mut advanced = false;
        for _ in 0..5_000_000u64 {
            get_after = self.pramin_r32(userd + 0x88);
            if get_after == target {
                advanced = true;
                break;
            }
            core::hint::spin_loop();
        }

        let intr0_post =
            unsafe { core::ptr::read_volatile((self._bar0 + 0x0000_2100) as *const u32) };
        let _ = writeln!(
            s,
            "[gpustep4]  PFIFO_INTR_0(0x2100) pre={:#010x} post={:#010x} (new bits={:#010x})",
            intr0_pre,
            intr0_post,
            intr0_post & !intr0_pre
        );

        // Speculative retry: ack any latched interrupt, re-commit the
        // runlist (idempotent), and ring the doorbell again — on the off
        // chance the very first commit on a cold/never-scheduled-before
        // FIFO needs a second nudge to actually wake the arbiter, even
        // though the register-level sequence matches real driver source
        // exactly. Cheap and safe (everything here is designed by NVIDIA
        // to be re-entrant/idempotent); only attempted if the first try
        // timed out.
        let mut retried = false;
        let mut retry_advanced = false;
        if !advanced {
            unsafe {
                core::ptr::write_volatile((self._bar0 + 0x0000_2100) as *mut u32, intr0_post);
            }
            let (retry_commit_ok, _) = self.setup_channel(b);
            unsafe { core::ptr::write_volatile((self._bar0 + 0x00bb_0090) as *mut u32, token) };
            retried = true;
            for _ in 0..2_000_000u64 {
                get_after = self.pramin_r32(userd + 0x88);
                if get_after == target {
                    retry_advanced = true;
                    advanced = true;
                    break;
                }
                core::hint::spin_loop();
            }
            let _ = writeln!(
                s,
                "[gpustep4]  retry: ack_intr + re-commit({}) + re-ring -> advanced={}",
                retry_commit_ok, retry_advanced
            );
        }
        let _ = (retried, retry_advanced);

        // SCHED_STATUS was sampled ONCE, before the ring (runlist_fetch_busy=1
        // in the last real-hardware run). A single snapshot can't tell a
        // fetch unit that is genuinely wedged apart from one merely caught
        // mid-cycle — those point at different bugs (a broken runlist-fetch
        // memory path vs. a fetch that completes fine but still never loads
        // the channel). Poll it here so the next run distinguishes the two.
        let mut fetch_busy_cleared = false;
        let mut fetch_busy_iters = 0u64;
        let mut sched_status_repoll = sched_status;
        for i in 0..2_000_000u64 {
            sched_status_repoll =
                unsafe { core::ptr::read_volatile((self._bar0 + 0x0000_263c) as *const u32) };
            if (sched_status_repoll >> 2) & 1 == 0 {
                fetch_busy_cleared = true;
                fetch_busy_iters = i;
                break;
            }
            core::hint::spin_loop();
        }
        let _ = writeln!(
            s,
            "[gpustep4]  SCHED_STATUS re-poll(0x263c)={:#010x} runlist_fetch_busy_cleared={} after_iters={}",
            sched_status_repoll, fetch_busy_cleared, fetch_busy_iters
        );

        // Read the MMU fault THIS step generated (cleared just before the ring).
        let rd = |off: u32| unsafe {
            core::ptr::read_volatile((self._bar0 + off as usize) as *const u32)
        };
        let f_info1 = rd(0x00b8_3090);
        let f_alo = rd(0x00b8_3080);
        let f_ahi = rd(0x00b8_3084);
        let f_info0 = rd(0x00b8_3088);
        let _ = writeln!(
            s,
            "[gpustep4]  fresh fault: INFO1={:#010x} valid={} access={} reason={} VA={:#x}{:08x} eng={:#x}",
            f_info1,
            f_info1 >> 31,
            (f_info1 >> 16) & 0xf,
            f_info1 & 0x1f,
            f_ahi,
            f_alo & 0xffff_f000,
            f_info0 & 0xff,
        );

        let chan_cfg =
            unsafe { core::ptr::read_volatile((self._bar0 + 0x0080_0004) as *const u32) };
        let _ = writeln!(
            s,
            "[gpustep4]  pb_va={:#x} n={} slot={} GP_PUT {}->{} GP_GET {}->{} advanced={} doorbell=0xbb0090 token={:#x}",
            pb_va, n, slot, put_before, target, get_before, get_after, advanced, token
        );
        let _ = writeln!(
            s,
            "[gpustep4]  chan{}-cfg(0x800004)={:#010x} status={}",
            CHID,
            chan_cfg,
            (chan_cfg >> 24) & 0xf
        );
        // PBDMA state: did the init un-SUSPEND them (STATUS != 0x10011111), who
        // serves runlist 0 (PBDMA_MAP RUNLISTS mask), is our channel loaded?
        let _ = writeln!(
            s,
            "[gpustep4]  PBDMA0 st(0x40100)={:#010x} ch={:#010x}  PBDMA1 st(0x42100)={:#010x} ch={:#010x}",
            rd(0x0004_0100),
            rd(0x0004_0120),
            rd(0x0004_2100),
            rd(0x0004_2120)
        );
        // PBDMA0/1 above are stale from the runlist-0 era and, per the last
        // real-hardware run, are NOT the PBDMA our channel goes through
        // (PBDMA_MAP showed only p9 serving a runl_id=8). Their own block
        // registers (STATUS/CHANNEL/GP_GET/GP_PUT/GET/INTR_0 — same offsets
        // as debug_dump's Step-1 report) had never actually been read for
        // whichever PBDMA(s) serve runl_id. Dump them here, dynamically.
        let _ = write!(s, "[gpustep4]  PBDMA(runl{}'s, raw block):", runl_id);
        for i in 0..12u32 {
            let map = rd(0x0000_2390 + i * 4) & 0xffff;
            if map & (1 << runl_id) == 0 {
                continue;
            }
            let pb = 0x0004_0000 + i * 0x2000;
            let _ = write!(
                s,
                " p{}[STATUS={:#010x} CHANNEL={:#010x} GP_GET={:#010x} GP_PUT={:#010x} GET={:#010x} INTR_0={:#010x}]",
                i,
                rd(pb + 0x100),
                rd(pb + 0x120),
                rd(pb + 0x14),
                rd(pb),
                rd(pb + 0x18),
                rd(pb + 0x108),
            );
        }
        let _ = writeln!(s);
        // 0x040100 is NV_PPBDMA_STATUS — all-SUSPENDED (0x10011111) is just the
        // idle/reset value, not a fault signal; nouveau's actual liveness check
        // (gk104_runq_idle) polls NV_PFIFO_PBDMA_STATUS at 0x003080+id*4,
        // CHAN_STATUS = bits 15:13 (0=INVALID/idle,1=VALID,5=LOAD,6=SAVE,7=SWITCH),
        // ID = bits 11:0 (loaded chid).
        let pfs0 = rd(0x0000_3080);
        let pfs1 = rd(0x0000_3084);
        let _ = writeln!(
            s,
            "[gpustep4]  PFIFO_PBDMA_STATUS q0(0x3080)={:#010x} chan_status={} id={:#x}  q1(0x3084)={:#010x} chan_status={} id={:#x}",
            pfs0,
            (pfs0 >> 13) & 0x7,
            pfs0 & 0xfff,
            pfs1,
            (pfs1 >> 13) & 0x7,
            pfs1 & 0xfff
        );
        // Same status register, but for whichever PBDMA index(es) actually
        // serve our runl_id (may not be q0/q1 at all for a non-zero runlist).
        let _ = write!(
            s,
            "[gpustep4]  PFIFO_PBDMA_STATUS(runl{}'s PBDMAs):",
            runl_id
        );
        for i in 0..12u32 {
            let m = rd(0x0000_2390 + i * 4) & 0xffff;
            if m & (1 << runl_id) != 0 {
                let v = rd(0x0000_3080 + i * 4);
                let _ = write!(
                    s,
                    " q{}={:#010x}(chan_status={} id={:#x})",
                    i,
                    v,
                    (v >> 13) & 0x7,
                    v & 0xfff
                );
            }
        }
        let _ = writeln!(s);
        // NV_PFIFO_PBDMA_MAP has up to 12 entries (__SIZE_1=12 per NVIDIA's
        // manual) — we'd only ever looked at p0-p3. If our discovered
        // runl_id (8/9/10, a standalone CE) isn't served by ANY of them,
        // that's a dead end: no hardware PBDMA route exists for it at all.
        let _ = write!(s, "[gpustep4]  PBDMA_MAP servers-of-runl{}:", runl_id);
        let mut any_serves = false;
        for i in 0..12u32 {
            let m = rd(0x0000_2390 + i * 4) & 0xffff;
            if m & (1 << runl_id) != 0 {
                let _ = write!(s, " p{}", i);
                any_serves = true;
            }
        }
        if !any_serves {
            let _ = write!(s, " NONE(!)");
        }
        let _ = write!(s, "  all-nonzero:");
        for i in 0..12u32 {
            let m = rd(0x0000_2390 + i * 4) & 0xffff;
            if m != 0 {
                let _ = write!(s, " p{}={:#06x}", i, m);
            }
        }
        let _ = writeln!(s);
        // Scheduler gate + the runlist entries as the host sees them in VRAM.
        let rl = b.runlist_vram();
        let _ = writeln!(
            s,
            "[gpustep4]  SCHED_DISABLE(0x2630)={:#010x} (runl{} bit={})  runlist@{:#x} cgrp[{:08x} {:08x} {:08x} {:08x}] chan[{:08x} {:08x} {:08x} {:08x}]",
            rd(0x0000_2630),
            runl_id,
            (rd(0x0000_2630) >> runl_id) & 1,
            rl,
            self.pramin_r32(rl),
            self.pramin_r32(rl + 0x4),
            self.pramin_r32(rl + 0x8),
            self.pramin_r32(rl + 0xc),
            self.pramin_r32(rl + 0x10),
            self.pramin_r32(rl + 0x14),
            self.pramin_r32(rl + 0x18),
            self.pramin_r32(rl + 0x1c)
        );
        if advanced {
            let _ = writeln!(
                s,
                "[gpustep4]  OK — channel fetched the pushbuffer via GMMU and bound the copy class, no fault. Ready for Step 5 (real copy)."
            );
        } else {
            let _ = writeln!(
                s,
                "[gpustep4]  TIMEOUT — GP_GET did not advance; PBDMA likely faulted (GPFIFO/pushbuffer mapping). Inspect /proc/gpudbg (do NOT re-ring)."
            );
        }
        s
    }

    fn get_caps(&self) -> DrmCaps {
        DrmCaps {
            has_3d: true,
            has_cursor: true,
            max_width: self.info.width,
            max_height: self.info.height,
        }
    }

    fn has_hardware_kms(&self) -> bool {
        // This driver does NOT have a working hardware-KMS presentation path on
        // this hardware. `page_flip` is a no-op (see below) because a real
        // display modeset/scanout on these GPUs wedges (the isochronous-scanout
        // dead-end documented in the display bring-up work). Claiming hardware
        // KMS here is actively harmful:
        //   * it disables `software_kms_active()` (linux-object drm.rs), so the
        //     dumb-buffer -> UEFI-GOP-framebuffer scanout blit — the ONLY path
        //     that actually lights up the panel — never runs (black screen);
        //   * it makes wlroots treat the node as a real KMS GPU and take the
        //     GLES2/GBM path, which hangs the whole OS at GL FBO creation on
        //     this stub (no usable GL/GBM). pixman + software scanout is the
        //     only combination that works here.
        // Return false so the software-KMS path drives the output. The KMS
        // framebuffer machinery above (create_fb/present_kms_fb) is left in
        // place, dormant, for the day a real modeset path exists.
        false
    }

    fn nouveau_gem_close(&self, handle: u32) -> bool {
        let removed = {
            let mut gem = self.nouveau_gem.lock();
            gem.iter()
                .position(|o| o.handle == handle)
                .map(|pos| gem.remove(pos))
        };
        let Some(obj) = removed else {
            return false;
        };
        // Drop the CPU-mmap registration FIRST: once GEM_CLOSE returns,
        // nothing should be able to newly mmap() this handle even if the
        // RM free below fails partway -- a stale physical mapping handed
        // to a new caller after the underlying VRAM is freed/reused is
        // exactly the kind of dangling-mapping bug this ordering avoids.
        if obj.phys_addr.is_some() {
            crate::scheme::gem_mmap::unregister(handle);
        }
        // Drain any VM_BIND mappings still referencing this handle BEFORE
        // freeing the backing memory below -- the real nouveau contract
        // expects UNMAP before CLOSE, but a caller that skips it shouldn't
        // leak the VA reservation (h_virt) in RM forever.
        self.drain_vm_mappings(&alloc::format!("GEM_CLOSE handle={}", handle), |m| {
            m.gem_handle == handle
        });
        let status = match *self.rm_device_instance.lock() {
            Some(device_instance) => Some(nvidia_rm_sys::rm_init::gem_free(device_instance, obj.h_memory)),
            None => None,
        };
        log::info!(
            "[nouveau-uapi] GEM_CLOSE handle={} h_memory={:#010x} -> gem_free status={:?}",
            handle,
            obj.h_memory,
            status
        );
        true
    }

    fn get_connector_edid(&self, id: u32) -> Option<[u8; 128]> {
        let (instance, d) = self.rm_display_state()?;
        let bit = id.checked_sub(1001 + 100 * instance)?;
        if bit >= 32 || d.display_mask & (1u32 << bit) == 0 {
            return None;
        }
        let did = 1u32 << bit;
        let connected = d.connected_mask & did != 0;
        if connected && d.edid_valid == 1 && d.edid_display_id == did {
            if let Some((boot_e, boot_len)) = boot_edid() {
                if boot_len >= 128 && boot_e[8..12] == d.edid_head[8..12] {
                    return Some(boot_e);
                }
            }
            let mut edid = [0u8; 128];
            edid[..32].copy_from_slice(&d.edid_head);
            return Some(edid);
        }
        None
    }

    fn import_buffer(&self, handle: GemHandle) -> bool {
        let mut handles = self.imported_handles.lock();
        if let Some(existing) = handles.iter_mut().find(|h| h.id == handle.id) {
            *existing = ImportedGemHandle {
                id: handle.id,
                phys_addr: handle.phys_addr,
                size: handle.size,
            };
            return true;
        }
        handles.push(ImportedGemHandle {
            id: handle.id,
            phys_addr: handle.phys_addr,
            size: handle.size,
        });
        true
    }

    fn free_buffer(&self, handle: GemHandle) {
        self.imported_handles.lock().retain(|h| h.id != handle.id);
        let removed_ids: Vec<u32> = {
            let mut fbs = self.kms_framebuffers.lock();
            let ids: Vec<u32> = fbs
                .iter()
                .filter(|fb| fb.handle_id == handle.id)
                .map(|fb| fb.id)
                .collect();
            fbs.retain(|fb| fb.handle_id != handle.id);
            ids
        };
        if !removed_ids.is_empty() {
            let mut state = self.kms_state.lock();
            if removed_ids.iter().any(|id| *id == state.crtc_fb) {
                state.crtc_fb = 0;
            }
            if removed_ids.iter().any(|id| *id == state.plane_fb) {
                state.plane_fb = 0;
            }
        }
        if let Some(ref mut a) = *self.vram_allocator.lock() {
            a.free(handle.phys_addr, handle.size);
        }
    }

    fn create_fb(&self, handle_id: u32, width: u32, height: u32, pitch: u32) -> Option<u32> {
        let handle = self.imported_handle(handle_id)?;
        if width == 0 || height == 0 || pitch == 0 {
            return None;
        }
        let size = (pitch as usize).checked_mul(height as usize)?;
        if size == 0 || size > handle.size {
            return None;
        }
        let fb_id = self.next_kms_fb_id.fetch_add(1, Ordering::Relaxed);
        self.kms_framebuffers.lock().push(NvidiaKmsFramebuffer {
            id: fb_id,
            handle_id,
            width,
            height,
            pitch,
            phys_addr: handle.phys_addr,
            size,
        });
        Some(fb_id)
    }

    fn page_flip(&self, _fb_id: u32) -> bool {
        // This stub cannot perform a real hardware page-flip / scanout for
        // wlroots' dumb-buffer + pixman path. Returning `true` here would be a
        // lie: it short-circuits the DRM layer's `driver.page_flip(fb) ||
        // scanout(fb)` fallback (drm.rs) and the framebuffer never gets the
        // blit, leaving the screen black. Return `false` so the software
        // scanout path always runs when this driver is the primary one.
        false
    }

    fn set_cursor(&self, _crtc_id: u32, _x: i32, _y: i32, _handle: u32, flags: u32) -> bool {
        const DRM_CURSOR_MOVE: u32 = 0x02;
        if (flags & DRM_CURSOR_MOVE) != 0 {
            // Potential software cursor update here if supported
            return true;
        }
        false
    }

    fn wait_vblank(&self, _crtc_id: u32) -> bool {
        const FRAME_US: u64 = 1_000_000 / 60;
        let state = self.kms_state.lock();
        let now = unsafe { crate::bus::drivers_timer_now_as_micros() };
        let target = if state.last_vblank_us == 0 {
            now.saturating_add(FRAME_US)
        } else {
            state.last_vblank_us.saturating_add(FRAME_US)
        };
        drop(state);
        while unsafe { crate::bus::drivers_timer_now_as_micros() } < target {
            core::hint::spin_loop();
        }
        self.kms_state.lock().last_vblank_us = target;
        true
    }

    fn get_resources(&self) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
        // Real connector topology straight from the RM (NV0073
        // GET_SUPPORTED): one connector per physical output bit. Until this
        // GPU's bring-up chain has run, fall back to the legacy synthetic
        // connector so pre-init userspace behaviour is unchanged.
        if let Some((instance, d)) = self.rm_display_state() {
            let conns: Vec<u32> = (0..32u32)
                .filter(|b| d.display_mask & (1 << b) != 0)
                .map(|b| Self::rm_connector_id(instance, b))
                .collect();
            return (Vec::new(), alloc::vec![2001], conns);
        }
        (Vec::new(), alloc::vec![2001], alloc::vec![1001])
    }

    fn get_connector(&self, id: u32) -> Option<DrmConnector> {
        if let Some((instance, d)) = self.rm_display_state() {
            let bit = id.checked_sub(1001 + 100 * instance)?;
            if bit >= 32 || d.display_mask & (1u32 << bit) == 0 {
                return None;
            }
            let did = 1u32 << bit;
            let connected = d.connected_mask & did != 0;
            // EDID bytes 21/22 = max image size in cm; only known for the
            // output whose EDID the RM actually read.
            let (mm_width, mm_height) =
                if connected && d.edid_valid == 1 && d.edid_display_id == did {
                    (
                        u32::from(d.edid_head[21]) * 10,
                        u32::from(d.edid_head[22]) * 10,
                    )
                } else {
                    (0, 0)
                };
            return Some(DrmConnector {
                id,
                connected,
                mm_width,
                mm_height,
                connector_type: Self::rm_conn_type(&d, did)
                    .map(nv_conn_type_to_drm)
                    .unwrap_or(0),
            });
        }
        if id == 1001 {
            Some(DrmConnector {
                id,
                connected: true,
                mm_width: 0,
                mm_height: 0,
                connector_type: 11,
            })
        } else {
            None
        }
    }

    fn get_crtc(&self, id: u32) -> Option<DrmCrtc> {
        if id == 2001 {
            let state = self.kms_state.lock();
            Some(DrmCrtc {
                id,
                fb_id: state.crtc_fb,
                x: 0,
                y: 0,
            })
        } else {
            None
        }
    }

    fn get_plane(&self, id: u32) -> Option<DrmPlane> {
        if id == 3001 {
            let state = self.kms_state.lock();
            Some(DrmPlane {
                id,
                crtc_id: 2001,
                fb_id: state.plane_fb,
                possible_crtcs: 1,
                plane_type: 1,
            })
        } else {
            None
        }
    }

    fn get_planes(&self) -> Vec<u32> {
        alloc::vec![3001]
    }

    fn set_plane(
        &self,
        plane_id: u32,
        _crtc_id: u32,
        fb_id: u32,
        _x: i32,
        _y: i32,
        _w: u32,
        _h: u32,
        _src_x: u32,
        _src_y: u32,
        _src_w: u32,
        _src_h: u32,
    ) -> bool {
        if plane_id != 3001 {
            return false;
        }
        if fb_id == 0 {
            self.kms_state.lock().plane_fb = 0;
            return true;
        }
        let ok = self.present_kms_fb(fb_id);
        if ok {
            self.kms_state.lock().plane_fb = fb_id;
        }
        ok
    }

    fn ioctl(&self, request: u32, arg: usize) -> Result<usize, i32> {
        // No known caller pid on this path -- see `ioctl_owned`. `nouveau_ioctl`
        // only actually uses it for CHANNEL_ALLOC's ownership bookkeeping, so
        // callers that only reach `ioctl` (bypassing the pid-aware dispatch in
        // `linux-object`'s `drm_scheme.rs`, if any exist) just get an
        // unreclaimable-on-exit channel -- the same behavior this driver had
        // before `nouveau_release_process` existed.
        self.ioctl_owned(request, arg, 0)
    }

    fn ioctl_owned(&self, request: u32, arg: usize, owner_pid: u64) -> Result<usize, i32> {
        match request {
            0x10DE0001 => {
                // Get Temperature
                if let Some(t) = self.temperature() {
                    Ok(t as usize)
                } else {
                    Err(22) // EINVAL
                }
            }
            0x10DE0002 => {
                // Get VRAM size MB
                Ok(self.vram_size_mb as usize)
            }
            0x10DE0010 => {
                // Fill Rect (arg is pointer to [u32; 5]: x, y, w, h, color)
                let p = arg as *const u32;
                unsafe {
                    self.fill_rect(*p, *p.add(1), *p.add(2), *p.add(3), *p.add(4));
                }
                Ok(0)
            }
            0x10DE0011 => {
                // Blit Rect (arg is pointer to [u32; 6]: sx, sy, dx, dy, w, h)
                let p = arg as *const u32;
                unsafe {
                    self.blit_rect(*p, *p.add(1), *p.add(2), *p.add(3), *p.add(4), *p.add(5));
                }
                Ok(0)
            }
            _ => self.nouveau_ioctl(request, arg, owner_pid),
        }
    }

    fn nouveau_release_process(&self, pid: u64) {
        if pid == 0 {
            return;
        }
        // Drop every channel this process owned. Only the RM-backed one carries
        // real GPU state, so a process that merely enumerated (discovery
        // channels) is reclaimed without touching the RM or the shared VAS.
        let had_rm_backed = {
            let mut chans = self.nouveau_channels.lock();
            let before = chans.len();
            let mut rm_backed = false;
            chans.retain(|c| {
                if c.owner_pid == pid {
                    rm_backed |= c.rm_backed;
                    false
                } else {
                    true
                }
            });
            if before == chans.len() {
                return;
            }
            rm_backed
        };
        if !had_rm_backed {
            log::info!(
                "[nouveau-uapi] process exit pid={}: released discovery channel(s) only",
                pid
            );
            return;
        }
        // Same drain used by CHANNEL_FREE -- this driver models a single
        // VAS, so reclaiming the channel means every VM_BIND in it goes.
        self.drain_vm_mappings(&alloc::format!("process exit pid={}", pid), |_| true);
        let gem_objects = core::mem::take(&mut *self.nouveau_gem.lock());
        let device_instance = *self.rm_device_instance.lock();
        let mut freed_bytes = 0u64;
        for obj in &gem_objects {
            if obj.phys_addr.is_some() {
                crate::scheme::gem_mmap::unregister(obj.handle);
            }
            if let Some(device_instance) = device_instance {
                let status = nvidia_rm_sys::rm_init::gem_free(device_instance, obj.h_memory);
                if status != 0 {
                    log::warn!(
                        "[nouveau-uapi] process exit pid={}: gem_free handle={} h_memory={:#010x} failed, NV_STATUS={:#x}",
                        pid, obj.handle, obj.h_memory, status
                    );
                }
            }
            freed_bytes += obj.size;
        }
        log::info!(
            "[nouveau-uapi] process exit pid={}: released nouveau channel + {} GEM object(s), {} KiB",
            pid,
            gem_objects.len(),
            freed_bytes / 1024
        );
    }
}

/// Nouveau-compatible driver-specific ioctls -- see `nouveau_uapi.rs` for
/// the ioctl numbers/structs and the module doc there for what is real vs.
/// deliberately refused in this milestone. Entirely opt-in
/// (`nvidia.nouveau_uapi`); returns the same ENOSYS as before when off.
impl NvidiaGpu {
    /// Drains every `nouveau_vm_mappings` entry for which `matches` returns
    /// true and `vm_bind_unmap`s each one via RM -- the same real RM call
    /// VM_BIND's own UNMAP op uses (see `DRM_IOCTL_NOUVEAU_VM_BIND`'s
    /// `VM_BIND_OP_UNMAP` arm below), just applied in bulk instead of to
    /// one caller-named mapping. Shared by `nouveau_gem_close` (only
    /// entries for the handle being closed) and `CHANNEL_FREE` (every
    /// entry -- this driver models a single VAS, so freeing the one
    /// channel orphans all of them). `context` is a short label prefixed
    /// onto each log line so it's clear which caller triggered the drain.
    fn drain_vm_mappings(
        &self,
        context: &str,
        mut matches: impl FnMut(&super::nouveau_uapi::NouveauVmMapping) -> bool,
    ) {
        let device_instance = *self.rm_device_instance.lock();
        let stale = {
            let mut maps = self.nouveau_vm_mappings.lock();
            let mut drained = Vec::new();
            let mut i = 0;
            while i < maps.len() {
                if matches(&maps[i]) {
                    drained.push(maps.remove(i));
                } else {
                    i += 1;
                }
            }
            drained
        };
        for mapping in stale {
            let Some(device_instance) = device_instance else {
                log::warn!(
                    "[nouveau-uapi] {}: VA={:#x} (gem_handle={}) leaked -- GPU not attached to RM, can't vm_bind_unmap",
                    context, mapping.va, mapping.gem_handle
                );
                continue;
            };
            let status = nvidia_rm_sys::rm_init::vm_bind_unmap(
                device_instance,
                mapping.h_virt,
                mapping.size,
                mapping.va,
            );
            log::info!(
                "[nouveau-uapi] {}: dropped stale VM_BIND VA={:#x} (gem_handle={}) -> vm_bind_unmap status={:#x}",
                context, mapping.va, mapping.gem_handle, status
            );
        }
    }

    /// Applies a single `VM_BIND` op (`MAP` or `UNMAP`). Factored out so
    /// `DRM_IOCTL_NOUVEAU_VM_BIND` can loop it over an `op_count > 1`
    /// array -- see that arm's own comment on why this isn't atomic
    /// across ops.
    fn vm_bind_op(
        &self,
        device_instance: u32,
        op: &super::nouveau_uapi::DrmNouveauVmBindOp,
    ) -> Result<(), i32> {
        use super::nouveau_uapi as nv;
        match op.op {
            nv::VM_BIND_OP_MAP => {
                let h_memory = {
                    let gem = self.nouveau_gem.lock();
                    let Some(obj) = gem.iter().find(|o| o.handle == op.handle) else {
                        return Err(nv::ENOENT);
                    };
                    obj.h_memory
                };
                match nvidia_rm_sys::rm_init::vm_bind_map(device_instance, h_memory, op.range, op.addr) {
                    Ok(b) if b.map_status == 0 => {
                        self.nouveau_vm_mappings.lock().push(nv::NouveauVmMapping {
                            gem_handle: op.handle,
                            h_virt: b.h_virt,
                            va: b.actual_va,
                            size: op.range,
                        });
                        log::info!(
                            "[nouveau-uapi] VM_BIND MAP handle={} -> VA={:#x} ({} bytes)",
                            op.handle,
                            b.actual_va,
                            op.range
                        );
                        Ok(())
                    }
                    Ok(b) => {
                        log::warn!(
                            "[nouveau-uapi] VM_BIND MAP failed: virt={:#x} map={:#x}",
                            b.virt_status,
                            b.map_status
                        );
                        Err(nv::EIO)
                    }
                    Err(status) => {
                        log::warn!("[nouveau-uapi] VM_BIND MAP failed, NV_STATUS={:#x}", status);
                        Err(nv::EIO)
                    }
                }
            }
            nv::VM_BIND_OP_UNMAP => {
                let mapping = {
                    let mut maps = self.nouveau_vm_mappings.lock();
                    maps.iter()
                        .position(|m| m.gem_handle == op.handle && m.va == op.addr)
                        .map(|i| maps.remove(i))
                };
                let Some(mapping) = mapping else {
                    return Err(nv::ENOENT);
                };
                let status = nvidia_rm_sys::rm_init::vm_bind_unmap(
                    device_instance,
                    mapping.h_virt,
                    mapping.size,
                    mapping.va,
                );
                if status == 0 {
                    log::info!(
                        "[nouveau-uapi] VM_BIND UNMAP handle={} VA={:#x}",
                        op.handle,
                        mapping.va
                    );
                    Ok(())
                } else {
                    log::warn!("[nouveau-uapi] VM_BIND UNMAP failed, NV_STATUS={:#x}", status);
                    Err(nv::EIO)
                }
            }
            other => {
                log::warn!("[nouveau-uapi] VM_BIND: unknown op {:#x}", other);
                Err(nv::EINVAL)
            }
        }
    }

    /// Submits a single pushbuffer with no fence -- shared by `EXEC`'s
    /// `sig_count == 0` path (every push, since none needs a fence) and
    /// its `sig_count > 0` path (every push but the last, which gets
    /// `exec_submit_signaled` instead -- see that arm's own comment).
    fn submit_push_plain(
        &self,
        device_instance: u32,
        push: &super::nouveau_uapi::DrmNouveauExecPush,
    ) -> Result<(), i32> {
        use super::nouveau_uapi as nv;
        match nvidia_rm_sys::rm_init::exec_submit(device_instance, push.va, push.va_len) {
            Ok(r) if r.submit_status == 0 => {
                log::info!(
                    "[nouveau-uapi] EXEC pushVA={:#x} len={} -> submitted (ring slot after={})",
                    push.va,
                    push.va_len,
                    r.gp_put_after
                );
                Ok(())
            }
            Ok(r) => {
                log::warn!(
                    "[nouveau-uapi] EXEC submit failed: lookup={:#x} map={:#x} token={:#x} submit={:#x}",
                    r.lookup_status,
                    r.map_status,
                    r.token_status,
                    r.submit_status
                );
                Err(nv::EIO)
            }
            Err(status) => {
                log::warn!("[nouveau-uapi] EXEC failed, NV_STATUS={:#x}", status);
                Err(nv::EIO)
            }
        }
    }

    /// Whether the calling process holds an RM-backed channel.
    ///
    /// A discovery channel reserves an id and answers class enumeration, but
    /// no GR channel or GPFIFO was ever built for it (`step16`/`step17` run
    /// only in CHANNEL_ALLOC's RM branch). Submitting against it would push
    /// into a ring that does not exist, so every path that touches hardware
    /// has to check THIS, not just `rm_device_instance` -- the two disagree
    /// exactly when a client allocated its channel before the RM was attached
    /// and the operator attached it afterwards.
    fn nouveau_has_rm_channel(&self, owner_pid: u64) -> bool {
        self.nouveau_channels
            .lock()
            .iter()
            .any(|c| c.rm_backed && (c.owner_pid == owner_pid || owner_pid == 0))
    }

    /// This GPU's architecture as the HARDWARE reports it.
    ///
    /// `self.architecture` comes from `identify_gpu`, a ~25-entry PCI
    /// device-id table whose default arm is `Unknown` -- so a Super refresh, a
    /// laptop part or a Ti variant that is not listed would make
    /// `nouveau_engine_classes` refuse and cost the client every Vulkan GPU,
    /// even though NV_PMC_BOOT_0 identifies the chip perfectly well. Prefer the
    /// register, fall back to the table only when it is unreadable.
    fn nouveau_arch(&self) -> NvidiaArchitecture {
        let boot0 = unsafe { core::ptr::read_volatile(self._bar0 as *const u32) };
        if boot0 != 0xffff_ffff && boot0 != 0 {
            let arch = arch_from_pmc_boot0(boot0);
            if arch != NvidiaArchitecture::Unknown {
                return arch;
            }
        }
        self.architecture
    }

    /// NV_PMC_BOOT_0's chip id -- what real nouveau reports as
    /// `nv_device_info_v0.chipset`, and the ONLY chipset source NVK 26.x uses
    /// (it never issues `GETPARAM_CHIPSET_ID`). Mesa maps it to an SM version
    /// through a `>=`-range table, so the value must be the real one: the
    /// per-architecture `*_MIN` constants land on the datacenter part for
    /// Ampere (0x170 = GA100 -> SM80), which mis-targets every consumer GA10x
    /// (those need >= 0x172 -> SM86) and would make NAK emit wrong code.
    fn nouveau_chipset_id(&self) -> u16 {
        // BAR0+0 is NV_PMC_BOOT_0 -- the same plain 32-bit read the probe and
        // the GSP recovery path already do.
        let boot0 = unsafe { core::ptr::read_volatile(self._bar0 as *const u32) };
        // 9-bit chip-id field, per nouveau: (boot0 & 0x1ff00000) >> 20.
        let chip = ((boot0 >> regs::PMC_BOOT0_CHIP_ID_SHIFT) & 0x1ff) as u16;
        if boot0 != 0xffff_ffff && chip != 0 {
            return chip;
        }
        // Device off the bus / register unreadable: fall back to a
        // REPRESENTATIVE CONSUMER id for the architecture rather than a value
        // that decodes to the wrong SM.
        match self.nouveau_arch() {
            NvidiaArchitecture::Turing => 0x162,      // TU102
            NvidiaArchitecture::Ampere => 0x172,      // GA102 (SM86, not GA100)
            NvidiaArchitecture::AdaLovelace => 0x192, // AD102
            NvidiaArchitecture::Hopper => 0x180,      // GH100
            NvidiaArchitecture::Blackwell => 0x1b2,   // GB202
            NvidiaArchitecture::Unknown => 0,
        }
    }

    /// NV_PMC_BOOT_0's revision nibble (`nv_device_info_v0.revision`).
    fn nouveau_chip_revision(&self) -> u8 {
        let boot0 = unsafe { core::ptr::read_volatile(self._bar0 as *const u32) };
        if boot0 == 0xffff_ffff {
            0
        } else {
            (boot0 & 0xff) as u8
        }
    }

    /// `NOUVEAU_GETPARAM_GRAPH_UNITS`, packed exactly like Linux's
    /// `gf100_gr_units()`: `gpc_nr | tpc_total << 8 | rop_nr << 32`.
    ///
    /// Mesa unpacks `gpc_count = v & 0xff` and `tpc_count = (v >> 8) & 0xffff`
    /// and sizes shader-local memory from them, and this getparam is
    /// **enumeration-fatal** (`goto out_err` on failure), so it can never
    /// return EINVAL as an earlier milestone did.
    fn nouveau_graph_units(&self) -> u64 {
        // Real topology, straight from the live GSP-RM, whenever the GPU is
        // attached (this is the same GR_GET_GPC_MASK/TPC_MASK probe as
        // `/proc/gpustep15`).
        // Copy the instance out and DROP the guard before the FFI call: an
        // `if let` scrutinee keeps its temporary guard alive for the whole
        // block, which would hold this IRQ-disabling spinlock across a GSP-RM
        // control round-trip. Every other call site does it this way.
        let dev = *self.rm_device_instance.lock();
        if let Some(dev) = dev {
            if let Ok(p) = nvidia_rm_sys::rm_init::step15(dev) {
                if p.gpc_mask_status == 0 && p.tpc_mask_status == 0 && p.num_gpc > 0 {
                    return (p.num_gpc as u64 & 0xff) | ((p.total_tpc as u64 & 0xffff) << 8);
                }
            }
        }
        // No RM yet: report the FULL-DIE configuration for the architecture.
        // Erring high is the safe direction -- Mesa sizes the shader TLS from
        // these, so over-reporting merely over-allocates, while
        // under-reporting leaves real SMs without scratch and faults the GPU.
        let (gpc, tpc) = match self.nouveau_arch() {
            NvidiaArchitecture::Turing => (6u64, 36u64),      // TU102
            NvidiaArchitecture::Ampere => (7, 42),            // GA102
            NvidiaArchitecture::AdaLovelace => (12, 72),      // AD102
            NvidiaArchitecture::Hopper => (8, 72),            // GH100
            NvidiaArchitecture::Blackwell => (12, 96),        // GB202
            NvidiaArchitecture::Unknown => (8, 64),
        };
        log::warn!(
            "[nouveau-uapi] GRAPH_UNITS: RM not attached -- reporting the full-die \
             {:?} topology (gpc={} tpc={}) instead of the floorswept truth; attach \
             the RM (/proc/gpustep14) for the real GR probe",
            self.nouveau_arch(),
            gpc,
            tpc
        );
        (gpc & 0xff) | ((tpc & 0xffff) << 8)
    }

    /// The engine classes advertised through `NVIF SCLASS`.
    ///
    /// Mesa picks, per engine type, the HIGHEST class whose LOW BYTE matches:
    /// 0xb5 copy, 0x2d 2d, 0x97 3d, 0x40 (else 0x39) m2mf, 0xc0 compute. A
    /// type with no match yields oclass 0, which mesa turns into -EINVAL and
    /// the device is dropped -- so all five must be present.
    fn nouveau_engine_classes(&self) -> Option<[i32; 5]> {
        use super::nouveau_uapi as nv;
        let (eng3d, compute, copy) = match self.nouveau_arch() {
            NvidiaArchitecture::Turing => nv::CLASSES_TURING,
            NvidiaArchitecture::Ampere => nv::CLASSES_AMPERE,
            NvidiaArchitecture::AdaLovelace => nv::CLASSES_ADA,
            NvidiaArchitecture::Hopper => nv::CLASSES_HOPPER,
            NvidiaArchitecture::Blackwell => nv::CLASSES_BLACKWELL,
            // Refuse rather than guess: a wrong 3D class means Mesa encodes
            // methods this chip does not implement, which faults the GPU. An
            // unadvertised class makes NVK skip the device -- the honest
            // outcome for hardware this driver does not recognize.
            NvidiaArchitecture::Unknown => {
                log::warn!(
                    "[nouveau-uapi] NVIF SCLASS: unknown GPU architecture -- refusing to \
                     guess engine classes (NVK will skip this GPU)"
                );
                return None;
            }
        };
        Some([
            nv::CLASS_FERMI_TWOD_A,
            nv::CLASS_KEPLER_INLINE_TO_MEMORY_B,
            eng3d,
            compute,
            copy,
        ])
    }

    /// `DRM_NOUVEAU_NVIF` (nr 0x47) -- nouveau's generic object-model ioctl.
    ///
    /// NVK's winsys needs this during *physical-device enumeration*, long
    /// before any rendering: `nouveau_ws_device_new()` allocates an NV_DEVICE
    /// object, reads its INFO (the sole source of chipset/VRAM/type), then per
    /// channel enumerates engine classes (SCLASS) and allocates five
    /// subchannel objects. Every one of those is fatal on failure, so an
    /// unimplemented NVIF meant zero Vulkan GPUs.
    ///
    /// Objects here are pure bookkeeping: mesa passes its OWN pointers as
    /// `token`/`object` cookies and never asks the kernel to mint handles, so
    /// accepting NEW/DEL without allocating hardware state is faithful for
    /// this path (real per-object state is created by CHANNEL_ALLOC/EXEC).
    fn nouveau_nvif(&self, arg: usize, size: usize) -> Result<usize, i32> {
        use super::nouveau_uapi as nv;
        const HDR: usize = core::mem::size_of::<nv::NvifIoctlV0>();
        if size < HDR {
            log::warn!("[nouveau-uapi] NVIF: payload {} < 24-byte header", size);
            return Err(nv::EINVAL);
        }
        // Read, never reference: `arg` is a raw userspace pointer with no
        // alignment guarantee, and forming a reference to a misaligned address
        // is UB even if the read would have worked.
        let hdr = unsafe { core::ptr::read_unaligned(arg as *const nv::NvifIoctlV0) };
        let body = arg + HDR;
        let body_len = size - HDR;

        match hdr.type_ {
            nv::NVIF_IOCTL_V0_NEW => {
                if body_len < core::mem::size_of::<nv::NvifIoctlNewV0>() {
                    return Err(nv::EINVAL);
                }
                let new = unsafe { core::ptr::read_unaligned(body as *const nv::NvifIoctlNewV0) };
                if new.oclass == nv::NVIF_CLASS_NV_DEVICE {
                    // The NEW body is followed by class data -- `nv_device_v0`
                    // for NV_DEVICE, whose `device` selects which GPU the
                    // client wants (mesa passes ~0 = "client default", i.e.
                    // the device behind this fd, which is the only one this
                    // node exposes).
                    let sel = if body_len - core::mem::size_of::<nv::NvifIoctlNewV0>()
                        >= core::mem::size_of::<nv::NvDeviceV0>()
                    {
                        let d = unsafe {
                            core::ptr::read_unaligned(
                                (body + core::mem::size_of::<nv::NvifIoctlNewV0>())
                                    as *const nv::NvDeviceV0,
                            )
                        };
                        d.device
                    } else {
                        u64::MAX
                    };
                    if sel != u64::MAX {
                        log::warn!(
                            "[nouveau-uapi] NVIF NEW NV_DEVICE: selector {:#x} is not the client \
                             default (~0); this node exposes exactly one GPU",
                            sel
                        );
                        return Err(nv::EINVAL);
                    }
                    log::warn!(
                        "[nouveau-uapi] NVIF NEW NV_DEVICE (token={:#x}) -- device object accepted",
                        new.token
                    );
                } else if new.oclass == 0 {
                    // Mesa only reaches this if SCLASS gave it nothing usable.
                    log::warn!("[nouveau-uapi] NVIF NEW with oclass=0 -- rejecting");
                    return Err(nv::EINVAL);
                } else {
                    log::warn!(
                        "[nouveau-uapi] NVIF NEW subchannel oclass={:#06x} on channel token={} \
                         -- accepted",
                        new.oclass,
                        hdr.token
                    );
                }
                Ok(0)
            }

            nv::NVIF_IOCTL_V0_MTHD => {
                const MB: usize = core::mem::size_of::<nv::NvifIoctlMthdV0>();
                if body_len < MB {
                    return Err(nv::EINVAL);
                }
                let mthd = unsafe { core::ptr::read_unaligned(body as *const nv::NvifIoctlMthdV0) };
                if mthd.method != nv::NV_DEVICE_V0_INFO {
                    log::warn!(
                        "[nouveau-uapi] NVIF MTHD: method {:#04x} not implemented",
                        mthd.method
                    );
                    return Err(nv::ENOSYS);
                }
                if body_len - MB < core::mem::size_of::<nv::NvDeviceInfoV0>() {
                    return Err(nv::EINVAL);
                }
                // `vram_size_mb` comes from the per-model PCI-id table, which
                // returns 0 for a board it does not list. Reporting ram_user=0
                // makes mesa set vram_size_B=0, and NVK then skips its whole
                // `if vram_size_B > 0` block: the device comes up with NO
                // DEVICE_LOCAL memory type at all, which violates the Vulkan
                // spec and fails later at allocation instead of here. Fall back
                // to the BAR1 aperture, which is a real, measured lower bound.
                let mut vram_bytes = (self.vram_size_mb as u64) * 1024 * 1024;
                if vram_bytes == 0 {
                    vram_bytes = self.info.fb_size as u64;
                    log::warn!(
                        "[nouveau-uapi] NVIF INFO: this board is not in the VRAM table -- \
                         reporting the BAR1 aperture ({} MiB) as VRAM. It is a lower bound, \
                         not the truth.",
                        vram_bytes / (1024 * 1024)
                    );
                }
                let chipset = self.nouveau_chipset_id();
                let mut info = nv::NvDeviceInfoV0 {
                    version: 0,
                    // PCI/AGP/PCIE all map to NV_DEVICE_TYPE_DIS (discrete) in
                    // mesa, which NVK's conformance gate requires; IGP/SOC do
                    // not. Every GPU this driver binds is a discrete PCIe part.
                    platform: nv::NV_DEVICE_INFO_V0_PCIE,
                    chipset,
                    revision: self.nouveau_chip_revision(),
                    // `family` only enumerates pre-Pascal families upstream and
                    // mesa does not read it; 0 is honest.
                    family: 0,
                    pad06: [0; 2],
                    ram_size: vram_bytes,
                    // `ram_user` is what mesa takes as vram_size_B.
                    ram_user: vram_bytes,
                    chip: [0; 16],
                    name: [0; 64],
                };
                // Display strings only (mesa copies them verbatim into
                // device_name/chipset_name).
                let chip_tag = match self.nouveau_arch() {
                    NvidiaArchitecture::Turing => b"TU1xx".as_slice(),
                    NvidiaArchitecture::Ampere => b"GA1xx".as_slice(),
                    NvidiaArchitecture::AdaLovelace => b"AD1xx".as_slice(),
                    NvidiaArchitecture::Hopper => b"GH1xx".as_slice(),
                    NvidiaArchitecture::Blackwell => b"GB2xx".as_slice(),
                    NvidiaArchitecture::Unknown => b"NV".as_slice(),
                };
                let n = chip_tag.len().min(info.chip.len() - 1);
                info.chip[..n].copy_from_slice(&chip_tag[..n]);
                let name_src = self.name.as_bytes();
                let n = name_src.len().min(info.name.len() - 1);
                info.name[..n].copy_from_slice(&name_src[..n]);

                unsafe {
                    core::ptr::write_unaligned((body + MB) as *mut nv::NvDeviceInfoV0, info);
                }
                log::warn!(
                    "[nouveau-uapi] NVIF MTHD NV_DEVICE_V0_INFO -> chipset={:#05x} rev={:#04x} \
                     vram={} MiB platform=PCIE",
                    chipset,
                    info.revision,
                    self.vram_size_mb
                );
                Ok(0)
            }

            nv::NVIF_IOCTL_V0_SCLASS => {
                const SB: usize = core::mem::size_of::<nv::NvifIoctlSclassV0>();
                const EB: usize = core::mem::size_of::<nv::NvifSclassOclassV0>();
                if body_len < SB {
                    return Err(nv::EINVAL);
                }
                let mut sclass = unsafe { core::ptr::read_unaligned(body as *const nv::NvifIoctlSclassV0) };
                // Class enumeration is per CHANNEL: mesa sends route=0xff with
                // token=<channel> straight after CHANNEL_ALLOC, mirroring real
                // nouveau where these objects are children of the channel.
                // Answering without one would advertise engines on a channel
                // that does not exist.
                // Class enumeration is per CHANNEL. Mesa sends route=0xff and
                // token=<the channel id CHANNEL_ALLOC handed back>, mirroring
                // real nouveau, where these objects are children of the channel
                // object (`nouveau_abi16_ioctl_sclass` resolves ioctl->token to
                // an abi16 channel and rejects anything else). Resolve it for
                // real rather than assuming there is exactly one.
                if hdr.route != 0xff {
                    log::warn!(
                        "[nouveau-uapi] NVIF SCLASS: route={:#04x}, expected 0xff (channel-scoped)",
                        hdr.route
                    );
                    return Err(nv::EINVAL);
                }
                let chan = self.nouveau_channels.lock();
                let Some(st) = chan.iter().find(|c| c.id >= 0 && c.id as u64 == hdr.token) else {
                    log::warn!(
                        "[nouveau-uapi] NVIF SCLASS: no channel with token={} -- CHANNEL_ALLOC \
                         must come first",
                        hdr.token
                    );
                    return Err(nv::EINVAL);
                };
                let (h_vas, h_notifier, rm_backed) = (st.h_vas, st.notifier_handle, st.rm_backed);
                drop(chan);
                let Some(classes) = self.nouveau_engine_classes() else {
                    return Err(nv::EINVAL);
                };
                // Honour the caller's advertised slot count, the real payload
                // length, AND the protocol's own ceiling.
                let room = (sclass.count as usize)
                    .min((body_len - SB) / EB)
                    .min(nv::NVIF_SCLASS_MAX);
                let n = classes.len().min(room);
                let arr = (body + SB) as *mut nv::NvifSclassOclassV0;
                for i in 0..n {
                    unsafe {
                        core::ptr::write_unaligned(
                            arr.add(i),
                            nv::NvifSclassOclassV0 {
                                oclass: classes[i],
                                minver: 0,
                                maxver: 0,
                            },
                        );
                    }
                }
                // Mesa reads ALL `NOUVEAU_WS_CONTEXT_MAX_CLASSES` slots
                // regardless of the count we report, so leave no stale entries
                // behind in the tail.
                for i in n..room {
                    unsafe {
                        core::ptr::write_unaligned(
                            arr.add(i),
                            nv::NvifSclassOclassV0 {
                                oclass: 0,
                                minver: 0,
                                maxver: 0,
                            },
                        );
                    }
                }
                sclass.count = n as u8;
                unsafe { core::ptr::write_unaligned(body as *mut nv::NvifIoctlSclassV0, sclass) };
                log::warn!(
                    "[nouveau-uapi] NVIF SCLASS on {} channel token={} (hVas={:#010x} \
                     hNotifier={:#010x}) -> {} classes {:#06x?}",
                    if rm_backed { "RM-backed" } else { "discovery" },
                    hdr.token,
                    h_vas,
                    h_notifier,
                    n,
                    &classes[..n]
                );
                Ok(0)
            }

            nv::NVIF_IOCTL_V0_DEL => Ok(0),

            other => {
                log::warn!(
                    "[nouveau-uapi] NVIF: type {:#04x} not implemented -- returning ENOSYS",
                    other
                );
                Err(nv::ENOSYS)
            }
        }
    }

    fn nouveau_ioctl(&self, request: u32, arg: usize, owner_pid: u64) -> Result<usize, i32> {
        use super::nouveau_uapi as nv;
        if !nv::enabled() {
            return Err(nv::ENOSYS);
        }
        // Name every distinct ioctl the first time Mesa issues it, so one
        // real-hardware boot reveals the full vocabulary and, above all, the
        // submission path (legacy GEM_PUSHBUF vs new EXEC). Bounded/de-duped.
        nv::trace_first_sight(request);
        // Dispatch by NR, exactly like Linux:
        //   nouveau_drm.c: `switch (_IOC_NR(cmd) - DRM_COMMAND_BASE)`
        // The caller's direction and size bits are ADVISORY. Matching the full
        // request number (as this driver used to) silently loses any ioctl
        // whose encoding differs from ours -- which is precisely what happened
        // with VM_INIT: mesa issues it through drmCommandWrite (_IOW,
        // 0x40106450) while we only accepted the _IOWR form (0xC0106450), so
        // it fell through to ENOSYS, mesa cleared `has_vm_bind`, and NVK
        // dropped the GPU with VK_ERROR_INCOMPATIBLE_DRIVER -- zero Vulkan
        // devices, no diagnostic. NVIF makes NR dispatch mandatory anyway: it
        // multiplexes five different payload sizes and directions onto nr 0x47.
        // `nouveau_ioctl` is the fall-through for ANY unrecognised ioctl on
        // /dev/dri/*, not just DRM ones, so NR alone is not a safe key: the
        // terminal/file families collide (FIONCLEX 0x5450 -> VM_INIT's nr,
        // FIOCLEX 0x5451 -> VM_BIND, FIOASYNC 0x5452 -> EXEC). Linux never has
        // this problem because drm_ioctl() only ever sees type 'd'. Require it.
        if (request >> 8) & 0xff != 0x64 {
            return Err(nv::ENOSYS);
        }
        let (_dir, nr, size) = nv::decode_ioc(request);
        // Dispatching by NR deliberately ignores the caller's DIRECTION bits,
        // but the SIZE still has to be honoured: every arm below casts `arg` to
        // a fixed struct and writes results back into it, so a caller that
        // declared a shorter payload than our struct would have memory written
        // past the end of its buffer. The old full-request match rejected those
        // implicitly (the size is baked into the request number); with NR
        // dispatch that guard has to be explicit. Linux does the same thing --
        // drm_ioctl() copies in/out against its own table's size, never the
        // caller's word.
        if let Some(need) = nv::min_payload_for_nr(nr) {
            if (size as usize) < need {
                log::warn!(
                    "[nouveau-uapi] {} (nr={:#04x}): caller payload {} < {} required -- \
                     refusing rather than writing past the caller's buffer",
                    nv::nouveau_ioctl_name(nr),
                    nr,
                    size,
                    need
                );
                return Err(nv::EINVAL);
            }
        }
        match nr {
            nv::NR_GETPARAM => {
                let req = unsafe { &mut *(arg as *mut nv::DrmNouveauGetparam) };
                let vram_bytes = (self.vram_size_mb as u64) * 1024 * 1024;
                req.value = match req.param {
                    nv::NOUVEAU_GETPARAM_PCI_VENDOR => 0x10de,
                    nv::NOUVEAU_GETPARAM_PCI_DEVICE => self.device_id as u64,
                    // Real nouveau distinguishes AGP/PCI/PCIE; every GPU this
                    // driver recognizes (Turing+) is PCIe-only.
                    nv::NOUVEAU_GETPARAM_BUS_TYPE => 2,
                    nv::NOUVEAU_GETPARAM_FB_SIZE => vram_bytes,
                    // The BAR1 *aperture*, which is NOT the VRAM size on a
                    // non-ReBAR system (typically 256 MiB). NVK compares the
                    // two to decide whether to expose a second, smaller
                    // host-visible heap; reporting them equal makes it treat
                    // ALL VRAM as CPU-mappable and mmap past the aperture.
                    nv::NOUVEAU_GETPARAM_VRAM_BAR_SIZE => self.info.fb_size as u64,
                    nv::NOUVEAU_GETPARAM_AGP_SIZE => 0,
                    // The REAL chip id, not the architecture's lower bound:
                    // gallium's nouveau GL still reads this, and a bound value
                    // decodes to the wrong SM (0x170 is GA100, not GA10x).
                    nv::NOUVEAU_GETPARAM_CHIPSET_ID => self.nouveau_chipset_id() as u64,
                    nv::NOUVEAU_GETPARAM_HAS_BO_USAGE => 0,
                    nv::NOUVEAU_GETPARAM_HAS_PAGEFLIP => 0,
                    nv::NOUVEAU_GETPARAM_HAS_VMA_TILEMODE => 0,
                    // No live usage counter in `NvidiaVramAllocator` yet --
                    // report 0 rather than guessing.
                    nv::NOUVEAU_GETPARAM_VRAM_USED => 0,
                    // A monotonically-rising nanosecond counter. Real nouveau
                    // returns the GPU's PTIMER; Mesa uses this for GL_TIMESTAMP,
                    // which only needs a rising clock, so a CPU-derived
                    // monotonic (safe -- no BAR0 read) is an honest stand-in.
                    nv::NOUVEAU_GETPARAM_PTIMER_TIME => {
                        (unsafe { crate::bus::drivers_timer_now_as_micros() } as u64) * 1000
                    }
                    // This driver's EXEC ioctl caps at 64 pushbuffers per call.
                    nv::NOUVEAU_GETPARAM_EXEC_PUSH_MAX => 64,
                    // Enumeration-fatal in mesa (`goto out_err`), so this can
                    // never be EINVAL: see `nouveau_graph_units`, which uses the
                    // live GSP-RM GR probe when the GPU is attached.
                    nv::NOUVEAU_GETPARAM_GRAPH_UNITS => self.nouveau_graph_units(),
                    _ => {
                        // warn, not debug: at the default LOG=warn boot level a
                        // real client (NVK) querying a param this milestone
                        // doesn't know about would otherwise fail EINVAL with
                        // zero trace -- exactly the case a first real-hardware
                        // run most needs visible.
                        log::warn!(
                            "[nouveau-uapi] GETPARAM: unknown param {:#x} -- returning EINVAL",
                            req.param
                        );
                        return Err(nv::EINVAL);
                    }
                };
                Ok(0)
            }

            nv::NR_CHANNEL_ALLOC => {
                let mut chan = self.nouveau_channels.lock();
                if chan.len() >= nv::MAX_CHANNELS {
                    log::warn!(
                        "[nouveau-uapi] CHANNEL_ALLOC: {} channels already live",
                        chan.len()
                    );
                    return Err(nv::EBUSY);
                }
                // Lowest free id.
                let new_id = (0i32..).find(|i| !chan.iter().any(|c| c.id == *i)).unwrap_or(0);
                // Only ONE channel can be RM-backed: step16+step17 build a
                // single GR channel on the hardware. A second concurrent
                // client (typically `vulkaninfo` run beside a compositor that
                // already holds the real one) gets a discovery channel so it
                // can still enumerate -- mesa allocates a throwaway channel
                // during vkEnumeratePhysicalDevices purely to ask SCLASS which
                // engine classes exist. Returning EBUSY there would cost it
                // every GPU.
                let rm_taken = chan.iter().any(|c| c.rm_backed);
                if rm_taken {
                    chan.push(nv::NouveauChannelState {
                        id: new_id,
                        h_vas: 0,
                        notifier_handle: 0,
                        rm_backed: false,
                        owner_pid,
                    });
                    drop(chan);
                    let req = unsafe { &mut *(arg as *mut nv::DrmNouveauChannelAlloc) };
                    req.channel = new_id;
                    req.notifier_handle = 0;
                    req.pushbuf_domains = nv::NOUVEAU_GEM_DOMAIN_VRAM;
                    req.nr_subchan = 0;
                    log::warn!(
                        "[nouveau-uapi] CHANNEL_ALLOC owner_pid={} -> channel={} DISCOVERY ONLY \
                         (the RM-backed channel is held by another client; class enumeration \
                         works, submission does not)",
                        owner_pid,
                        new_id
                    );
                    return Ok(0);
                }
                let Some(device_instance) = *self.rm_device_instance.lock() else {
                    // No RM yet. NVK allocates a channel during *enumeration*
                    // (nouveau_ws_context_create inside nouveau_ws_device_new)
                    // only to run NVIF SCLASS and five subchannel NEWs, then
                    // frees it -- it never submits. Refusing here therefore
                    // costs the whole physical device (vkEnumeratePhysicalDevices
                    // reports 0 GPUs) even though nothing about that sequence
                    // needs hardware. Serve it from software instead, and let
                    // the paths that DO need the RM (GEM_NEW/VM_BIND/EXEC) fail
                    // with their own explicit ENODEV.
                    //
                    // Attaching the RM implicitly here is deliberately NOT done:
                    // the ladder boots GSP-RM and does real bring-up that can
                    // hang the machine, so it stays an explicit operator action
                    // (`cat /proc/gpustep14`).
                    chan.push(nv::NouveauChannelState {
                        id: new_id,
                        h_vas: 0,
                        notifier_handle: 0,
                        rm_backed: false,
                        owner_pid,
                    });
                    drop(chan);
                    let req = unsafe { &mut *(arg as *mut nv::DrmNouveauChannelAlloc) };
                    req.channel = new_id;
                    req.notifier_handle = 0;
                    req.pushbuf_domains = nv::NOUVEAU_GEM_DOMAIN_VRAM;
                    req.nr_subchan = 0;
                    log::warn!(
                        "[nouveau-uapi] CHANNEL_ALLOC owner_pid={} -> channel={} DISCOVERY ONLY \
                         (GPU not attached to the RM; class enumeration works, but GEM/VM_BIND/\
                         EXEC will return ENODEV until `cat /proc/gpustep14` runs)",
                        owner_pid,
                        new_id
                    );
                    return Ok(0);
                };
                nvidia_rm_sys::os_interface::capture_begin();
                let ladder = nvidia_rm_sys::rm_init::step16(device_instance);
                let _ = nvidia_rm_sys::os_interface::capture_take();
                let ladder = match ladder {
                    Ok(g) if g.ctxshare_status == 0 => g,
                    Ok(g) => {
                        log::warn!(
                            "[nouveau-uapi] CHANNEL_ALLOC: GR allocation ladder incomplete (ctxshare status {:#x})",
                            g.ctxshare_status
                        );
                        return Err(nv::ENODEV);
                    }
                    Err(status) => {
                        log::warn!(
                            "[nouveau-uapi] CHANNEL_ALLOC: step16 failed, NV_STATUS={:#x}",
                            status
                        );
                        return Err(nv::ENODEV);
                    }
                };
                nvidia_rm_sys::os_interface::capture_begin();
                let channel = nvidia_rm_sys::rm_init::step17(device_instance);
                let _ = nvidia_rm_sys::os_interface::capture_take();
                let channel = match channel {
                    Ok(c) if c.sched_status == 0 => c,
                    Ok(c) => {
                        log::warn!(
                            "[nouveau-uapi] CHANNEL_ALLOC: compute channel incomplete (sched status {:#x})",
                            c.sched_status
                        );
                        return Err(nv::ENODEV);
                    }
                    Err(status) => {
                        log::warn!(
                            "[nouveau-uapi] CHANNEL_ALLOC: step17 failed, NV_STATUS={:#x}",
                            status
                        );
                        return Err(nv::ENODEV);
                    }
                };
                chan.push(nv::NouveauChannelState {
                    id: new_id,
                    h_vas: ladder.h_vas,
                    notifier_handle: channel.h_notifier,
                    rm_backed: true,
                    owner_pid,
                });
                drop(chan);
                let req = unsafe { &mut *(arg as *mut nv::DrmNouveauChannelAlloc) };
                req.channel = new_id;
                req.notifier_handle = channel.h_notifier;
                req.pushbuf_domains = nv::NOUVEAU_GEM_DOMAIN_VRAM;
                req.nr_subchan = 0;
                log::info!(
                    "[nouveau-uapi] CHANNEL_ALLOC owner_pid={} -> channel={} (reused the existing step16+step17 bring-up ladder; hVas={:#010x} hNotifier={:#010x})",
                    owner_pid,
                    new_id,
                    ladder.h_vas,
                    channel.h_notifier
                );
                Ok(0)
            }

            nv::NR_CHANNEL_FREE => {
                let req = unsafe { &*(arg as *const nv::DrmNouveauChannelFree) };
                let was_rm_backed = {
                    let mut chans = self.nouveau_channels.lock();
                    let Some(pos) = chans.iter().position(|c| c.id == req.channel) else {
                        log::warn!(
                            "[nouveau-uapi] CHANNEL_FREE: no such channel {}",
                            req.channel
                        );
                        return Err(nv::EINVAL);
                    };
                    chans.remove(pos).rm_backed
                };
                if !was_rm_backed {
                    // Nothing was ever bound in a discovery channel.
                    return Ok(0);
                }
                // Clears Eclipse's own bookkeeping only: nvidia-rm-sys has no
                // teardown entry point for the step16/step17 ladder (its own
                // doc calls it "idempotent", i.e. built to be created once
                // per boot, not freed and rebuilt). A second CHANNEL_ALLOC
                // after this will re-run step16/step17, which just returns
                // their cached, still-alive allocation -- not a fresh one.
                // The channel's VAS itself isn't really torn down (see
                // above), but from userspace's point of view it's gone --
                // drop every VM_BIND mapping still living in it so a new
                // CHANNEL_ALLOC starts from an empty VM, and so nothing
                // left behind (e.g. by a caller that skipped VM_BIND
                // UNMAP/GEM_CLOSE) keeps its VA reservation forever.
                self.drain_vm_mappings("CHANNEL_FREE", |_| true);
                Ok(0)
            }

            nv::NR_VM_INIT => {
                // VM_INIT initialises the GPU VA space for THIS drm file and is
                // the FIRST driver-private ioctl NVK issues: its
                // nouveau_ws_device_new() -> nouveau_ws_device_alloc() calls it
                // during physical-device creation, BEFORE any CHANNEL_ALLOC.
                // Requiring a channel here (as an earlier milestone did) makes
                // that call fail EINVAL, so nouveau_ws_device_new() aborts and
                // vkEnumeratePhysicalDevices returns 0 GPUs with no further
                // trace -- exactly the "NVK sees nothing" symptom on real
                // hardware. VM_INIT is standalone by design; accept it here.
                //
                // It is DRM_IOW (client -> kernel): the client passes the VA
                // sub-range it wants the KERNEL to manage (the rest it manages
                // itself). Real per-mapping VA carving happens in VM_BIND against
                // the RM; VM_INIT only has to acknowledge the reservation, so read
                // the requested range for the log and return success. Do NOT
                // write the struct back (write-only ioctl).
                let req = unsafe { &*(arg as *const nv::DrmNouveauVmInit) };
                log::warn!(
                    "[nouveau-uapi] VM_INIT kernel_managed_addr={:#x} size={:#x} -> accepted (standalone, no channel required)",
                    req.kernel_managed_addr,
                    req.kernel_managed_size
                );
                Ok(0)
            }

            nv::NR_VM_BIND => {
                if !self.nouveau_has_rm_channel(owner_pid) {
                    log::warn!(
                        "[nouveau-uapi] VM_BIND: this client's channel is DISCOVERY-ONLY (no GR \
                         channel/GPFIFO was ever built for it) -- refusing to submit against \
                         uninitialised hardware; free it and CHANNEL_ALLOC again now that the \
                         RM is attached"
                    );
                    return Err(nv::ENODEV);
                }
                let req = unsafe { &*(arg as *const nv::DrmNouveauVmBind) };
                if req.wait_count != 0 || req.sig_count != 0 {
                    log::warn!(
                        "[nouveau-uapi] VM_BIND: wait_count/sig_count must be 0 -- VM_BIND ops complete synchronously within this ioctl (real RM calls, not queued GPU work), so there is nothing async to wait for or signal after (got wait_count={} sig_count={})",
                        req.wait_count, req.sig_count
                    );
                    return Err(nv::EOPNOTSUPP);
                }
                const MAX_VM_BIND_OPS: u32 = 64;
                if req.op_count == 0 || req.op_ptr == 0 {
                    return Err(nv::EINVAL);
                }
                if req.op_count > MAX_VM_BIND_OPS {
                    log::warn!(
                        "[nouveau-uapi] VM_BIND: op_count={} exceeds the {} this milestone supports per call",
                        req.op_count, MAX_VM_BIND_OPS
                    );
                    return Err(nv::EOPNOTSUPP);
                }
                let Some(device_instance) = *self.rm_device_instance.lock() else {
                    log::warn!("[nouveau-uapi] VM_BIND: GPU not attached to the RM yet");
                    return Err(nv::ENODEV);
                };
                // Ops are applied in order, one real RM call each -- NOT
                // atomic across the array: if op[i] fails, op[0..i] already
                // happened and stay applied, and op[i+1..] never run. Real
                // nouveau's own VM_BIND jobs behave the same way (each op
                // is validated/applied as it's processed, not as a single
                // all-or-nothing transaction).
                let ops = unsafe {
                    core::slice::from_raw_parts(
                        req.op_ptr as *const nv::DrmNouveauVmBindOp,
                        req.op_count as usize,
                    )
                };
                for (i, op) in ops.iter().enumerate() {
                    if let Err(e) = self.vm_bind_op(device_instance, op) {
                        if req.op_count > 1 {
                            log::warn!(
                                "[nouveau-uapi] VM_BIND: op[{}] of {} failed, stopping ({} earlier op(s) already applied)",
                                i, req.op_count, i
                            );
                        }
                        return Err(e);
                    }
                }
                Ok(0)
            }

            nv::NR_EXEC => {
                if !self.nouveau_has_rm_channel(owner_pid) {
                    log::warn!(
                        "[nouveau-uapi] EXEC: this client's channel is DISCOVERY-ONLY (no GR \
                         channel/GPFIFO was ever built for it) -- refusing to submit against \
                         uninitialised hardware; free it and CHANNEL_ALLOC again now that the \
                         RM is attached"
                    );
                    return Err(nv::ENODEV);
                }
                let req = unsafe { &*(arg as *const nv::DrmNouveauExec) };
                const MAX_EXEC_PUSH: u32 = 64;
                const MAX_EXEC_SYNC: u32 = 64;
                if req.wait_count > MAX_EXEC_SYNC || (req.wait_count > 0 && req.wait_ptr == 0) {
                    log::warn!(
                        "[nouveau-uapi] EXEC: wait_count={} exceeds the {} this milestone supports (or wait_ptr is null)",
                        req.wait_count, MAX_EXEC_SYNC
                    );
                    return Err(nv::EOPNOTSUPP);
                }
                if req.sig_count > MAX_EXEC_SYNC || (req.sig_count > 0 && req.sig_ptr == 0) {
                    log::warn!(
                        "[nouveau-uapi] EXEC: sig_count={} exceeds the {} this milestone supports (or sig_ptr is null)",
                        req.sig_count, MAX_EXEC_SYNC
                    );
                    return Err(nv::EOPNOTSUPP);
                }
                if req.push_count == 0 || req.push_count > MAX_EXEC_PUSH || req.push_ptr == 0 {
                    log::warn!(
                        "[nouveau-uapi] EXEC: push_count={} must be between 1 and {} (got push_ptr={:#x})",
                        req.push_count, MAX_EXEC_PUSH, req.push_ptr
                    );
                    return Err(nv::EOPNOTSUPP);
                }
                if req.channel != 0 {
                    return Err(nv::EINVAL);
                }
                let pushes = unsafe {
                    core::slice::from_raw_parts(
                        req.push_ptr as *const nv::DrmNouveauExecPush,
                        req.push_count as usize,
                    )
                };
                for push in pushes {
                    if push.va_len == 0 || push.va_len % 4 != 0 {
                        return Err(nv::EINVAL);
                    }
                }
                let Some(device_instance) = *self.rm_device_instance.lock() else {
                    log::warn!("[nouveau-uapi] EXEC: GPU not attached to the RM yet");
                    return Err(nv::ENODEV);
                };

                // wait_count: block THIS CALL (CPU-side) until ALL wait
                // syncobjs are signaled, before submitting anything. This is
                // NOT what real nouveau does -- real hardware makes the
                // GPU's own channel execute a semaphore-ACQUIRE method
                // before the caller's pushbuffer, so the CPU submit call
                // returns immediately and independent submissions can
                // overlap. Here the ioctl itself blocks first and only then
                // submits, so from a single synchronous caller's point of
                // view the observable contract is the same ("this EXEC does
                // not start executing before every wait fence is signaled")
                // but concurrent/overlapping submissions do not behave like
                // real hardware scheduling. Bounded by a fixed timeout
                // (never an indefinite kernel-side wait); nothing is
                // submitted if not all of them are satisfied in time.
                if req.wait_count > 0 {
                    let waits = unsafe {
                        core::slice::from_raw_parts(
                            req.wait_ptr as *const nv::DrmNouveauSync,
                            req.wait_count as usize,
                        )
                    };
                    let handles: Vec<u32> = waits.iter().map(|s| s.handle).collect();
                    let points: Vec<u64> = waits
                        .iter()
                        .map(|s| {
                            let timeline = s.flags & nv::SYNC_TYPE_MASK == nv::SYNC_TIMELINE_SYNCOBJ;
                            if timeline { s.timeline_value } else { 1 }
                        })
                        .collect();
                    const WAIT_TIMEOUT_US: u64 = 1_000_000; // 1 s
                    let deadline_us =
                        unsafe { crate::bus::drivers_timer_now_as_micros() } + WAIT_TIMEOUT_US;
                    match crate::scheme::syncobj::wait(&handles, Some(&points), true, deadline_us) {
                        crate::scheme::syncobj::WaitOutcome::Signaled { .. } => {
                            log::info!(
                                "[nouveau-uapi] EXEC: all {} wait syncobj(s) reached their target point -- proceeding to submit",
                                req.wait_count
                            );
                        }
                        crate::scheme::syncobj::WaitOutcome::Timeout => {
                            log::warn!(
                                "[nouveau-uapi] EXEC: not all {} wait syncobj(s) reached their target within {}us -- NOT submitting",
                                req.wait_count, WAIT_TIMEOUT_US
                            );
                            return Err(nv::EIO);
                        }
                        crate::scheme::syncobj::WaitOutcome::Invalid => {
                            return Err(nv::ENOENT);
                        }
                    }
                }

                if req.sig_count == 0 {
                    // No fence needed -- submit every push plainly, in order.
                    for push in pushes {
                        self.submit_push_plain(device_instance, push)?;
                    }
                    log::info!(
                        "[nouveau-uapi] EXEC: {} push(es) submitted (no signal)",
                        req.push_count
                    );
                    return Ok(0);
                }

                // sig_count > 0: submit every push but the last plainly, then
                // append the kernel's own tracking fence to the LAST one and
                // poll it (see eclipse_rm_exec_submit_signaled's doc for
                // exactly what a landed fence does and does not prove --
                // PBDMA fetch, not necessarily engine completion). GPFIFO is
                // strictly ordered, so a fence queued after the last push
                // only lands once every earlier push was fetched too -- one
                // fence still honestly covers the whole batch. Only once
                // that's confirmed do we advance the syncobjs -- never
                // before, so a signaled syncobj is never a lie. (Signaling
                // itself is NOT atomic across sig_count > 1: if syncobj i
                // has gone-bad handle, syncobjs before it are already
                // signaled and syncobjs after it never get a chance --
                // same as a single bad handle already behaved before this
                // milestone, just now with more than one to potentially fail.)
                let (last, rest) = pushes
                    .split_last()
                    .expect("push_count > 0 already checked above");
                for push in rest {
                    self.submit_push_plain(device_instance, push)?;
                }
                const TIMEOUT_MS: u32 = 1000;
                let fence_payload = nv::next_fence_payload();
                match nvidia_rm_sys::rm_init::exec_submit_signaled(
                    device_instance,
                    last.va,
                    last.va_len,
                    fence_payload,
                    TIMEOUT_MS,
                ) {
                    Ok(r) if r.submit_status == 0 && r.fence_submit_status == 0 && r.fence_wait_status == 0 => {
                        let sigs = unsafe {
                            core::slice::from_raw_parts(
                                req.sig_ptr as *const nv::DrmNouveauSync,
                                req.sig_count as usize,
                            )
                        };
                        for sig in sigs {
                            let timeline = sig.flags & nv::SYNC_TYPE_MASK == nv::SYNC_TIMELINE_SYNCOBJ;
                            let target = if timeline { sig.timeline_value } else { 1 };
                            if !crate::scheme::syncobj::timeline_signal(sig.handle, target) {
                                log::warn!(
                                    "[nouveau-uapi] EXEC: GPU work completed but signaling syncobj handle={} failed (unknown handle)",
                                    sig.handle
                                );
                                return Err(nv::ENOENT);
                            }
                        }
                        log::info!(
                            "[nouveau-uapi] EXEC: {} push(es) submitted and fence confirmed ({} syncobj(s) signaled)",
                            req.push_count, req.sig_count
                        );
                        Ok(0)
                    }
                    Ok(r) => {
                        log::warn!(
                            "[nouveau-uapi] EXEC (signaled) failed: lookup={:#x} map={:#x} token={:#x} submit={:#x} fenceSubmit={:#x} fenceWait={:#x} (fence value={:#x} expected={:#x})",
                            r.lookup_status, r.map_status, r.token_status, r.submit_status,
                            r.fence_submit_status, r.fence_wait_status, r.fence_value, fence_payload
                        );
                        Err(nv::EIO)
                    }
                    Err(status) => {
                        log::warn!("[nouveau-uapi] EXEC (signaled) failed, NV_STATUS={:#x}", status);
                        Err(nv::EIO)
                    }
                }
            }

            nv::NR_GEM_NEW => {
                let req = unsafe { &mut *(arg as *mut nv::DrmNouveauGemNew) };
                if req.info.domain & nv::NOUVEAU_GEM_DOMAIN_VRAM == 0 {
                    log::warn!(
                        "[nouveau-uapi] GEM_NEW: only DOMAIN_VRAM is supported in this milestone (requested domain={:#x})",
                        req.info.domain
                    );
                    return Err(nv::EOPNOTSUPP);
                }
                if req.info.size == 0 || req.info.size > u32::MAX as u64 {
                    return Err(nv::EINVAL);
                }
                let Some(device_instance) = *self.rm_device_instance.lock() else {
                    log::warn!("[nouveau-uapi] GEM_NEW: GPU not attached to the RM yet");
                    return Err(nv::ENODEV);
                };
                let alloc = match nvidia_rm_sys::rm_init::gem_alloc_vram(device_instance, req.info.size) {
                    Ok(a) if a.alloc_status == 0 => a,
                    Ok(a) => {
                        log::warn!("[nouveau-uapi] GEM_NEW: RM alloc failed, status={:#x}", a.alloc_status);
                        return Err(nv::ENOMEM);
                    }
                    Err(status) => {
                        log::warn!("[nouveau-uapi] GEM_NEW: gem_alloc_vram failed, NV_STATUS={:#x}", status);
                        return Err(nv::ENOMEM);
                    }
                };
                let handle = self.nouveau_gem_next_handle.fetch_add(1, Ordering::Relaxed);
                // Real BAR1-relative CPU physical address for this
                // allocation, if RM will give us one. Failure here doesn't
                // fail GEM_NEW itself: the object is still valid for
                // VM_BIND/EXEC, just not CPU-mmap-able, exactly like real
                // nouveau leaves map_handle absent for some domains.
                let phys_addr = match nvidia_rm_sys::rm_init::gem_map_cpu(device_instance, alloc.h_memory) {
                    // ADDR_FBMEM (2), not 0. `memdescGetAddressSpace` returns
                    // the aperture the object lives in, and VRAM is ADDR_FBMEM;
                    // 0 is ADDR_UNKNOWN. Requiring 0 here rejected EVERY valid
                    // VRAM allocation, so `map_handle` came back 0, mesa's
                    // `mmap(fd, ..., bo->map_handle)` had nothing to resolve and
                    // vkCreateDevice died with VK_ERROR_MEMORY_MAP_FAILED. The C
                    // side already refuses anything that is not ADDR_FBMEM (it
                    // sets lookup_status = NV_ERR_NOT_SUPPORTED), so this is
                    // belt-and-braces on a value it has already validated.
                    Ok(m)
                        if m.lookup_status == 0
                            && m.address_space == nvidia_rm_sys::rm_init::ADDR_FBMEM =>
                    {
                        Some(m.phys_addr)
                    }
                    Ok(m) => {
                        log::warn!(
                            "[nouveau-uapi] GEM_NEW handle={}: gem_map_cpu lookup_status={:#x} address_space={} -- not CPU-mmap-able",
                            handle, m.lookup_status, m.address_space
                        );
                        None
                    }
                    Err(status) => {
                        log::warn!(
                            "[nouveau-uapi] GEM_NEW handle={}: gem_map_cpu failed, NV_STATUS={:#x} -- not CPU-mmap-able",
                            handle, status
                        );
                        None
                    }
                };
                let map_handle = if let Some(pa) = phys_addr {
                    crate::scheme::gem_mmap::register(handle, pa, req.info.size);
                    (handle as u64) << 12
                } else {
                    0
                };
                self.nouveau_gem.lock().push(nv::NouveauGemObject {
                    handle,
                    h_memory: alloc.h_memory,
                    size: req.info.size,
                    phys_addr,
                });
                req.info.handle = handle;
                req.info.domain = nv::NOUVEAU_GEM_DOMAIN_VRAM;
                // Unbound until VM_BIND MAPs it -- GPU VA and the CPU mmap
                // offset above are independent in real nouveau too.
                req.info.offset = 0;
                req.info.map_handle = map_handle;
                log::info!(
                    "[nouveau-uapi] GEM_NEW handle={} size={} -> RM hMemory={:#010x} phys_addr={:?} map_handle={:#x}",
                    handle,
                    req.info.size,
                    alloc.h_memory,
                    phys_addr,
                    map_handle
                );
                Ok(0)
            }

            nv::NR_GEM_INFO => {
                let req = unsafe { &mut *(arg as *mut nv::DrmNouveauGemInfo) };
                // Scoped and dropped before touching nouveau_vm_mappings
                // below -- same discipline VM_BIND's own MAP op follows,
                // so the two locks are never held nested in either order.
                let (size, phys_addr) = {
                    let gem = self.nouveau_gem.lock();
                    let Some(obj) = gem.iter().find(|o| o.handle == req.handle) else {
                        return Err(nv::ENOENT);
                    };
                    (obj.size, obj.phys_addr)
                };
                // GPU VA, if VM_BIND has mapped this object -- bookkeeping
                // independent from nouveau_gem, same as VM_BIND itself.
                let offset = self
                    .nouveau_vm_mappings
                    .lock()
                    .iter()
                    .find(|m| m.gem_handle == req.handle)
                    .map(|m| m.va)
                    .unwrap_or(0);
                let map_handle = phys_addr.map(|_| (req.handle as u64) << 12).unwrap_or(0);
                log::debug!(
                    "[nouveau-uapi] GEM_INFO handle={} -> size={} offset={:#x} map_handle={:#x}",
                    req.handle,
                    size,
                    offset,
                    map_handle
                );
                req.domain = nv::NOUVEAU_GEM_DOMAIN_VRAM;
                req.size = size;
                req.offset = offset;
                req.map_handle = map_handle;
                req.tile_mode = 0;
                req.tile_flags = 0;
                Ok(0)
            }

            nv::NR_GEM_CPU_PREP => {
                let req = unsafe { &*(arg as *const nv::DrmNouveauGemCpuPrep) };
                let gem = self.nouveau_gem.lock();
                // No real fencing yet (EXEC has no sync objects, see above):
                // this only validates the handle exists. A CPU_PREP right
                // after an EXEC that touches this buffer is NOT actually
                // safe to trust -- there is no wait for GPU completion here.
                if gem.iter().any(|o| o.handle == req.handle) {
                    Ok(0)
                } else {
                    Err(nv::ENOENT)
                }
            }

            nv::NR_GEM_CPU_FINI => {
                let req = unsafe { &*(arg as *const nv::DrmNouveauGemCpuFini) };
                let gem = self.nouveau_gem.lock();
                if gem.iter().any(|o| o.handle == req.handle) {
                    Ok(0)
                } else {
                    Err(nv::ENOENT)
                }
            }

            nv::NR_GEM_PUSHBUF => {
                // The submission path of the classic **nvc0 Gallium** driver
                // (Mesa OpenGL for Turing) -- the one the shipped image uses.
                // Real submission is a hardware-validated follow-up (it needs
                // GART-domain GEM, the 3D class bound to the channel, and
                // relocation handling), so this milestone PARSES and LOGS the
                // whole request, then honestly returns EOPNOTSUPP. The dump is
                // the anatomy needed to build real submission: how many buffers
                // (and their domains), relocs, and pushes Mesa hands us per call.
                let pb = unsafe { &*(arg as *const nv::DrmNouveauGemPushbuf) };
                log::warn!(
                    "[nouveau-uapi] GEM_PUSHBUF: channel={} nr_buffers={} nr_relocs={} \
                     nr_push={} suffix0={:#x} suffix1={:#x} vram_avail={:#x} gart_avail={:#x}",
                    pb.channel,
                    pb.nr_buffers,
                    pb.nr_relocs,
                    pb.nr_push,
                    pb.suffix0,
                    pb.suffix1,
                    pb.vram_available,
                    pb.gart_available
                );
                // Log a bounded prefix of each array so the trace shows the shape
                // without flooding: which BOs (handle + domains) and which pushes
                // (BO + offset/len) make up this submission. Bound reads too --
                // never walk an unbounded user-supplied count.
                const DUMP_MAX: usize = 8;
                let nb = (pb.nr_buffers as usize).min(DUMP_MAX);
                if pb.buffers != 0 && nb > 0 {
                    let bos = unsafe {
                        core::slice::from_raw_parts(
                            pb.buffers as *const nv::DrmNouveauGemPushbufBo,
                            nb,
                        )
                    };
                    for (i, bo) in bos.iter().enumerate() {
                        log::warn!(
                            "[nouveau-uapi] GEM_PUSHBUF bo[{}/{}]: handle={} read_dom={:#x} \
                             write_dom={:#x} valid_dom={:#x} presumed(valid={} dom={:#x} off={:#x})",
                            i,
                            pb.nr_buffers,
                            bo.handle,
                            bo.read_domains,
                            bo.write_domains,
                            bo.valid_domains,
                            bo.presumed.valid,
                            bo.presumed.domain,
                            bo.presumed.offset
                        );
                    }
                }
                let np = (pb.nr_push as usize).min(DUMP_MAX);
                if pb.push != 0 && np > 0 {
                    let pushes = unsafe {
                        core::slice::from_raw_parts(
                            pb.push as *const nv::DrmNouveauGemPushbufPush,
                            np,
                        )
                    };
                    for (i, p) in pushes.iter().enumerate() {
                        log::warn!(
                            "[nouveau-uapi] GEM_PUSHBUF push[{}/{}]: bo_index={} offset={:#x} length={:#x}",
                            i,
                            pb.nr_push,
                            p.bo_index,
                            p.offset,
                            p.length
                        );
                    }
                }
                log::warn!(
                    "[nouveau-uapi] GEM_PUSHBUF: not submitted (real submission needs \
                     GART GEM + 3D class + relocs -- follow-up); returning EOPNOTSUPP"
                );
                Err(nv::EOPNOTSUPP)
            }

            nv::NR_NVIF => self.nouveau_nvif(arg, size as usize),

            // GET_ZCULL_INFO: mesa 26.x probes it and TOLERATES failure
            // (`has_zcull_info` just stays false), so ENOSYS is a correct,
            // honest answer -- named here only so the trace does not read as
            // an unknown ioctl.
            nv::NR_GET_ZCULL_INFO => Err(nv::ENOSYS),

            _ => {
                // warn, not debug: same reasoning as GETPARAM's unknown-param
                // arm above -- a real client hitting an ioctl this milestone
                // never implemented at all must be visible at the default
                // boot log level, not silently ENOSYS.
                let name = nv::nouveau_ioctl_name(nr);
                log::warn!(
                    "[nouveau-uapi] unhandled {} request={:#010x} (nr={:#04x} size={}) \
                     -- returning ENOSYS",
                    name,
                    request,
                    nr,
                    size
                );
                Err(nv::ENOSYS)
            }
        }
    }
}

#[allow(dead_code)]
pub struct NvidiaGpuDriverPci;

impl PciDriver for NvidiaGpuDriverPci {
    fn name(&self) -> &str {
        "Nvidia GPU"
    }

    fn matched(&self, vendor_id: u16, _device_id: u16) -> bool {
        vendor_id == 0x10DE
    }

    fn matched_dev(&self, dev: &PCIDevice) -> bool {
        dev.id.vendor_id == 0x10DE && dev.id.class == 0x03
    }

    fn init(
        &self,
        dev: &PCIDevice,
        mapper: &Option<Arc<dyn IoMapper>>,
        _irq: Option<usize>,
    ) -> DeviceResult<Device> {
        #[cfg(target_arch = "x86_64")]
        use crate::bus::pci::{read_bar_addr, PortOpsImpl, PCI_ACCESS};
        use crate::bus::phys_to_virt;
        #[cfg(target_arch = "x86_64")]
        const BAR0: u16 = 0x10;

        // Turing's real BAR0 register aperture is 16 MiB (0x0-0xFFFFFF);
        // used as a fallback only when the PCI-enumerated BAR length is
        // unavailable (e.g. the direct config-space re-read fallback
        // path below has no length to report). Do NOT re-probe BAR sizes
        // here (see the "do not probe BAR size at boot" note below) --
        // `dev.bars[0]`'s length already comes from the bus's own
        // one-time enumeration, same as every other driver's BAR1+
        // handling (e1000e, ixgbe, virtio_pci) already reads directly.
        const NVIDIA_BAR0_APERTURE_FALLBACK: u64 = 16 * 1024 * 1024;

        #[cfg(target_arch = "x86_64")]
        let (bar0_addr, bar0_map_len) = {
            if let Some(BAR::Memory(a, len, _, _)) = dev.bars[0] {
                if a != 0 {
                    (
                        a,
                        if len == 0 {
                            NVIDIA_BAR0_APERTURE_FALLBACK
                        } else {
                            len as u64
                        },
                    )
                } else {
                    let ops = &PortOpsImpl;
                    (
                        unsafe { read_bar_addr(ops, PCI_ACCESS, dev.loc, BAR0) },
                        NVIDIA_BAR0_APERTURE_FALLBACK,
                    )
                }
            } else {
                let ops = &PortOpsImpl;
                (
                    unsafe { read_bar_addr(ops, PCI_ACCESS, dev.loc, BAR0) },
                    NVIDIA_BAR0_APERTURE_FALLBACK,
                )
            }
        };
        #[cfg(not(target_arch = "x86_64"))]
        let (bar0_addr, bar0_map_len) = if let Some(BAR::Memory(a, len, _, _)) = dev.bars[0] {
            (
                a,
                if len == 0 {
                    NVIDIA_BAR0_APERTURE_FALLBACK
                } else {
                    len as u64
                },
            )
        } else {
            (0, NVIDIA_BAR0_APERTURE_FALLBACK)
        };

        if bar0_addr == 0 {
            return Err(DeviceError::NoResources);
        }

        // Wire up nvidia-rm-sys's KernelHooks facade so any real vendored
        // NVIDIA C file that reaches through os-interface.h for PCI config
        // space, MMIO mappings, port I/O, or timing gets Eclipse's actual
        // hardware primitives instead of the crate's safe-default stubs.
        super::nvidia_hooks::install(mapper);

        if let Some(m) = mapper {
            m.query_or_map(bar0_addr as usize, bar0_map_len as usize);
        }
        let bar0_vaddr = phys_to_virt(bar0_addr as usize);

        // Compact the six raw PCI BAR slots into the ordered list of populated
        // *memory* BARs, exactly as NVIDIA's own nv-pci.c does (it walks the PCI
        // resources and assigns each valid memory BAR to nv->bars[j++]). A
        // 64-bit BAR occupies one slot here and leaves the next `None`, so this
        // walk yields the same logical ordering NVIDIA uses:
        //   index 0 = REGS (16 MiB registers), 1 = FB (VRAM window),
        //   index 2 = IMEM (the ~32 MiB instance-memory aperture).
        // Do NOT probe BAR sizes here (writing 0xFFFFFFFF to a BAR register can
        // wedge config space on some GPUs and hang the machine); the lengths
        // already came from the bus's one-time enumeration.
        let mem_bars: Vec<(u64, u64)> = (0..6usize)
            .filter_map(|i| {
                if let Some(BAR::Memory(addr, len, _, _)) = dev.bars[i] {
                    if addr != 0 {
                        return Some((addr, len as u64));
                    }
                }
                None
            })
            .collect();

        // FB is the second memory BAR (index 1); fall back to a size-based
        // search for the first >= 16 MiB aperture past REGS if the ordering is
        // unexpected, matching the previous behaviour.
        let fb_bar = mem_bars
            .get(1)
            .map(|&(addr, len)| (addr, if len == 0 { 256 * 1024 * 1024 } else { len }))
            .filter(|&(_, len)| len >= (16 * 1024 * 1024))
            .or_else(|| {
                mem_bars.iter().skip(1).find_map(|&(addr, len)| {
                    let actual_len = if len == 0 { 256 * 1024 * 1024 } else { len };
                    (actual_len >= (16 * 1024 * 1024)).then_some((addr, actual_len))
                })
            });

        // IMEM/BAR2 is the third memory BAR (index 2). RM needs its physical
        // base+size as GPUATTACHARG.instPhysAddr/instLength for the BAR2 MMU
        // self-test in gpuStateInit; 0/0 if the GPU somehow exposes fewer than
        // three memory BARs (then the BAR2 test will still fail, but attach and
        // the earlier steps stay intact).
        let (imem_phys, imem_len) = mem_bars
            .get(2)
            .map(|&(addr, len)| (addr, if len == 0 { 32 * 1024 * 1024 } else { len }))
            .unwrap_or((0, 0));

        if let Some((fb_addr, fb_len)) = fb_bar {
            if let Some(m) = mapper {
                m.query_or_map(fb_addr as usize, fb_len as usize);
            }
            let fb_vaddr = phys_to_virt(fb_addr as usize);

            let gpu_name = alloc::format!(
                "nvidia-gpu-{}:{}.{}",
                dev.loc.bus,
                dev.loc.device,
                dev.loc.function
            );
            log::warn!(
                "[NVIDIA] GPU at {} bar0={:#x} fb={:#x} fb_len={:#x} imem={:#x} imem_len={:#x}",
                gpu_name,
                bar0_addr,
                fb_addr,
                fb_len,
                imem_phys,
                imem_len
            );
            let gpu = Arc::new(NvidiaGpu::new(
                gpu_name,
                dev.id.device_id,
                bar0_vaddr,
                fb_vaddr,
                fb_len as usize,
                fb_addr,
                1920,
                1080,
                bar0_addr,
                bar0_map_len,
                imem_phys,
                imem_len,
                0, // PCI domain: Eclipse only tracks bus/device/function, single-segment system
                dev.loc.bus,
                dev.loc.device,
            )?);
            gpu.set_msi_vector(_irq);
            Ok(Device::Drm(gpu))
        } else {
            Err(DeviceError::NoResources)
        }
    }
}
