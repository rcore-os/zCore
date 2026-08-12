//! DRM (Direct Rendering Manager) Subsystem for zCore
//!
//! Provides a unified interface for graphics drivers (NVIDIA, VirtIO, etc.)
//! and handles buffer management (GEM) and mode setting (KMS).

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use core::time::Duration;
use lock::Mutex;

use crate::sync::{Event, EventBus};
use kernel_hal::drivers;
use kernel_hal::mem::phys_to_virt;
pub use zcore_drivers::scheme::drm::{DrmCaps, DrmConnector, DrmCrtc, DrmPlane, GemHandle};
use zcore_drivers::scheme::{DisplayScheme, DrmScheme};
use zircon_object::vm::{pages, MMUFlags, VmObject};

/// Synthetic KMS object IDs used when there is no real DRM/KMS driver — only a
/// dumb framebuffer (`DisplayScheme`, e.g. the UEFI GOP display on bare metal).
/// wlroots' legacy modeset path needs at least one CRTC, connector and encoder
/// to drive an output; we synthesize them around the framebuffer and scan dumb
/// buffers out via [`DisplayScheme::blit_from`].
///
/// The ids must be **distinct across object types**: libdrm identifies objects
/// (for OBJ_GETPROPERTIES etc.) by id alone, often passing obj_type=ANY, so
/// reusing one id for CRTC/connector/plane makes them indistinguishable.
/// Synthetic CRTC id exposed to userspace for the synthetic output.
pub const SYNTH_CRTC_ID: u32 = 1;
const SYNTH_CONNECTOR_ID: u32 = 2;
/// Encoder id exposed to userspace for the synthetic output.
pub const SYNTH_ENCODER_ID: u32 = 3;
/// Primary plane id exposed to userspace for the synthetic output.
pub const SYNTH_PLANE_ID: u32 = 4;

/// Sequence counter for delivered page-flip / vblank events.
static FLIP_SEQ: AtomicU32 = AtomicU32::new(0);

/// One-shot guard so the first scanout logs (every-frame logging would spam).
static SCANOUT_LOGGED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Master switch for the per-frame GPU copy-engine present (`ce_present`).
/// OFF by default: the CE path fires a GPU DMA on EVERY desktop present
/// (sysmem read of the dumb buffer + P2P write into the console GPU's BAR1 +
/// two GMMU-translated semaphore writes into system RAM per submission), and
/// on real hardware it correlated with random SIGSEGVs in freshly-started
/// Wayland clients (lunarbg/lunarbar/foot Done(139)) while the CPU-blit
/// present was rock solid. Until the CE path is proven stable, the proven CPU
/// blit is the default and the CE present is strictly opt-in via the
/// `nvidia.cepresent` kernel cmdline flag (see zCore/src/main.rs).
static CE_PRESENT_ENABLED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Enable/disable the per-frame CE-offloaded present (set once at boot from
/// the `nvidia.cepresent` cmdline flag).
pub fn set_ce_present_enabled(on: bool) {
    CE_PRESENT_ENABLED.store(on, Ordering::Relaxed);
}

/// Master switch for the atomic-modesetting uAPI (`DRM_CLIENT_CAP_ATOMIC` +
/// `DRM_IOCTL_MODE_ATOMIC`). OFF by default: the legacy-KMS path is the one
/// proven on real hardware, so — like nouveau, which shipped its atomic
/// support behind `nouveau.atomic=1` — the atomic path is strictly opt-in via
/// the `drm.atomic` kernel cmdline flag (see zCore/src/main.rs) until it has
/// equivalent mileage. With the flag off, `SET_CLIENT_CAP(ATOMIC)` is refused
/// with `EOPNOTSUPP` exactly like a Linux driver without `DRIVER_ATOMIC`, and
/// compositors fall back to legacy KMS.
static ATOMIC_ENABLED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Whether the current DRM client negotiated `DRM_CLIENT_CAP_ATOMIC`.
///
/// Linux tracks this per `drm_file`; Eclipse's DRM model is single-client
/// (one implicit master), so one global flag mirrors it. Cleared on
/// DROP_MASTER so a later legacy-only compositor session starts clean.
/// Gates the visibility of DRM_MODE_PROP_ATOMIC properties in
/// OBJ_GETPROPERTIES/GETCONNECTOR, exactly like Linux hides atomic
/// properties from non-atomic clients.
static ATOMIC_CLIENT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Enable the atomic uAPI (set once at boot from the `drm.atomic` flag).
pub fn set_atomic_enabled(on: bool) {
    ATOMIC_ENABLED.store(on, Ordering::Relaxed);
}

/// Whether the atomic uAPI is enabled for this boot.
pub fn atomic_enabled() -> bool {
    ATOMIC_ENABLED.load(Ordering::Relaxed)
}

/// Record whether the (single) DRM client negotiated the atomic cap.
pub fn set_atomic_client(on: bool) {
    ATOMIC_CLIENT.store(on, Ordering::Relaxed);
}

/// Whether the current DRM client is an atomic client.
pub fn atomic_client() -> bool {
    ATOMIC_CLIENT.load(Ordering::Relaxed)
}

/// Return the primary framebuffer display, if any.
fn primary_display() -> Option<Arc<dyn DisplayScheme>> {
    drivers::all_display().first()
}

/// Whether the software KMS path should drive the output.
///
/// True when a framebuffer display exists but no registered DRM driver declares
/// hardware-KMS scanout support. In that case we keep the software fallback:
/// dumb-buffer blits to the primary framebuffer display (`blit_from`).
pub fn software_kms_active() -> bool {
    let have_display = primary_display().is_some();
    let driver_can_scanout = get_primary_driver()
        .map(|d| d.has_hardware_kms())
        .unwrap_or(false);
    have_display && !driver_can_scanout
}

/// A DRM Framebuffer object
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct DrmFramebuffer {
    pub id: u32,
    /// Optional driver-private framebuffer id returned by `DrmScheme::create_fb`.
    pub driver_fb_id: Option<u32>,
    /// GEM handle that backs this framebuffer
    pub gem_handle_id: u32,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub phys_addr: u64,
    pub size: usize,
}

struct DrmState {
    drivers: Vec<Arc<dyn DrmScheme>>,
    next_handle_id: u32,
    next_fb_id: u32,
    /// Live GEM objects: `(handle, backing VMO, owning pid)`.
    ///
    /// The pid is what makes these releasable. Linux frees a file's GEM
    /// objects from the DRM fd's release handler; this list had no owner at
    /// all and was only ever shrunk by an explicit DESTROY_DUMB/GEM_CLOSE
    /// ioctl, so anything that died without issuing one -- a crashed Xorg, a
    /// session torn down by its manager, an fd swept away by execve's CLOEXEC
    /// pass -- leaked its buffers forever. Each is a physically CONTIGUOUS
    /// VMO of up to 64 MiB (`MAX_DUMB_SIZE`), and it belongs to no process's
    /// address space, so per-process accounting cannot see it either.
    handles: Vec<(GemHandle, Arc<VmObject>, u64)>,
    framebuffers: Vec<DrmFramebuffer>,
    /// Framebuffer currently bound to the (synthetic) CRTC, reported by GETCRTC.
    crtc_fb: u32,
    /// The VT the compositor owns the display on, established on its first
    /// present. While the active VT differs (the user switched to a text
    /// console with Ctrl+Alt+Fn), the compositor's blits are suppressed so the
    /// text console stays visible, and its input is paused — Eclipse has no
    /// VT_PROCESS release/acquire signalling, so this is what makes VT
    /// switching out of the compositor actually work. Cleared on DROP_MASTER
    /// (compositor exit) so a later text-only session is never gated.
    graphics_vt: Option<usize>,
    /// Pending DRM events (page-flip completions) waiting to be `read()` from
    /// the card fd. Each entry is one fully-encoded `struct drm_event_vblank`.
    events: VecDeque<Vec<u8>>,
    eventbus: Arc<Mutex<EventBus>>,
    /// Monotonic time the next synthetic vblank / page-flip completion is
    /// allowed to fire. A software framebuffer has no real vblank, so a
    /// `DRM_IOCTL_MODE_PAGE_FLIP` used to complete *instantly*: the compositor's
    /// `flip -> poll -> read event -> render -> flip` loop then never blocked
    /// and re-rendered (and full-screen-blitted) as fast as the CPU allowed —
    /// pegging a host core under QEMU. Deferring the flip-complete event to this
    /// deadline paces the loop to ~60 Hz, which is what a real vblank would do.
    next_vblank: Duration,
    /// Kernel-composited hardware cursor (legacy `DRM_IOCTL_MODE_CURSOR`). The
    /// bitmap is a copy of the client's cursor BO (premultiplied ARGB8888,
    /// `w`x`h`), drawn on top of every scanned-out frame at `(x, y)`.
    cursor: CursorState,
    /// KMS property blobs (`CREATEPROPBLOB`/`GETPROPBLOB`), both user-created
    /// (e.g. the `MODE_ID` blob an atomic compositor uploads) and
    /// kernel-created (the current-mode blob echoed via OBJ_GETPROPERTIES).
    blobs: Vec<DrmBlob>,
    /// Next blob id. Starts high above the synthetic KMS ids, fb ids and the
    /// EDID blob ids (20000+connector) so the object-id namespaces never
    /// collide — libdrm identifies blobs purely by id.
    next_blob_id: u32,
    /// Software-KMS state mirrored back to atomic clients (see
    /// [`AtomicKmsState`]).
    atomic: AtomicKmsState,
}

/// A KMS property blob (`struct drm_property_blob`).
struct DrmBlob {
    id: u32,
    /// Whether userspace created it (`CREATEPROPBLOB`). Kernel-created blobs
    /// (current mode, EDID) refuse `DESTROYPROPBLOB` with EPERM like Linux.
    user_created: bool,
    data: Vec<u8>,
}

/// Last-committed atomic state of the synthetic pipeline, echoed back through
/// `OBJ_GETPROPERTIES` so an atomic compositor's state readback matches what
/// it committed. The software scanout itself only consumes the framebuffer id
/// (full-screen blit); the rects are bookkeeping for the uAPI contract.
#[derive(Default, Clone, Copy)]
pub struct AtomicKmsState {
    /// CRTC "ACTIVE" property.
    pub active: bool,
    /// Kernel-owned blob id holding the current mode (0 = none committed).
    pub mode_blob_id: u32,
    /// Plane "CRTC_X"/"CRTC_Y" (output position).
    pub crtc_x: i32,
    /// See [`Self::crtc_x`].
    pub crtc_y: i32,
    /// Plane "CRTC_W"/"CRTC_H" (output size).
    pub crtc_w: u32,
    /// See [`Self::crtc_w`].
    pub crtc_h: u32,
    /// Plane "SRC_X".."SRC_H" in 16.16 fixed point.
    pub src_x: u32,
    /// See [`Self::src_x`].
    pub src_y: u32,
    /// See [`Self::src_x`].
    pub src_w: u32,
    /// See [`Self::src_x`].
    pub src_h: u32,
}

/// State for the kernel-composited cursor. wlroots (forced to legacy KMS) sets
/// the pointer image with `DRM_MODE_CURSOR_BO` and moves it with
/// `DRM_MODE_CURSOR_MOVE`; `scanout()` composites it over the frame.
#[derive(Default)]
struct CursorState {
    visible: bool,
    /// Top-left position of the cursor bitmap in output pixels (already
    /// hotspot-adjusted by the compositor for the legacy path).
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    /// Client cursor pixels (premultiplied ARGB8888, tightly packed). Held in
    /// an `Arc` so every `scanout()` / cursor move can share the pixels without
    /// cloning the Vec — a resize/redraw storm was cloning this on every flip
    /// and amplifying heap churn for ~30–50 min until a null fn-ptr #PF.
    bitmap: Option<Arc<[u32]>>,
    /// The rectangle `(x, y, w, h)` currently composited on the display, or
    /// `None` if nothing is drawn. A cursor move restores exactly this rect from
    /// the CRTC framebuffer before compositing the new one — so a move touches
    /// two ~64x64 windows instead of re-blitting the whole ~16 MB frame. This is
    /// what makes the pointer cheap enough to feel like a hardware cursor.
    drawn: Option<(i32, i32, u32, u32)>,
}

lazy_static::lazy_static! {
    static ref DRM_STATE: Mutex<DrmState> = Mutex::new(DrmState {
        drivers: Vec::new(),
        next_handle_id: 1,
        next_fb_id: 1,
        handles: Vec::new(),
        framebuffers: Vec::new(),
        crtc_fb: 0,
        graphics_vt: None,
        events: VecDeque::new(),
        eventbus: EventBus::new(),
        next_vblank: Duration::ZERO,
        cursor: CursorState {
            visible: false,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            bitmap: None,
            drawn: None,
        },
        blobs: Vec::new(),
        next_blob_id: 30000,
        atomic: AtomicKmsState::default(),
    });
}

/// Register a new DRM driver
pub fn register_driver(driver: Arc<dyn DrmScheme>) {
    let mut state = DRM_STATE.lock();
    if driver.name() == "simplefb" {
        state.drivers.push(driver);
    } else {
        state.drivers.insert(0, driver);
    }
}

/// Get the primary DRM driver
pub fn get_primary_driver() -> Option<Arc<dyn DrmScheme>> {
    DRM_STATE.lock().drivers.first().cloned()
}

/// Hard ceiling on live GEM objects. Contiguous dumb buffers are expensive
/// (up to `MAX_DUMB_SIZE` each) and sit outside process VMAs. Under a QEMU
/// window-resize / redraw storm the compositor can CREATE_DUMB faster than
/// it DESTROYs; without a cap, frame-allocator pressure eventually smashes
/// heap metadata and shows up as a null fn-ptr #PF after tens of minutes.
const MAX_LIVE_GEMS: usize = 64;

/// Allocate a buffer (GEM object).
///
/// Backed by contiguous physical memory so it can both be mmap'd by userspace
/// (the dumb-buffer mapping) and scanned out by a software framebuffer display.
/// When a hardware DRM driver is present it is told about the new buffer; with
/// no driver (plain framebuffer) the buffer is purely software.
pub fn alloc_buffer(size: usize) -> Option<GemHandle> {
    if size == 0 {
        return None;
    }

    // Reserve an id and snapshot the driver under the lock, then RELEASE it.
    // `DRM_STATE`'s `lock::Mutex` is an IRQ-disabling spinlock, so the heavy
    // work below must run with it dropped: zeroing a full-screen dumb buffer
    // (1080p ≈ 8 MiB = 2048 pages) under the lock keeps interrupts off for the
    // whole memset — starving the timer, scheduler and input — and serializes
    // every other DRM ioctl behind it. Calling into the driver under the lock
    // is also a latent deadlock if `import_buffer` ever waits on the GPU.
    // (Reserving the id unconditionally can leave a gap on failure; handle ids
    // only need to be unique, so a skipped id is harmless.)
    let (id, driver) = {
        let mut state = DRM_STATE.lock();
        let live = state.handles.len();
        if live >= MAX_LIVE_GEMS {
            log::error!(
                "[drm] alloc_buffer: {} live GEMs (>= {}), refusing CREATE_DUMB (size={})",
                live,
                MAX_LIVE_GEMS,
                size
            );
            return None;
        }
        if live >= MAX_LIVE_GEMS / 2 {
            log::warn!(
                "[drm] alloc_buffer: elevated GEM pressure ({} live, cap {})",
                live,
                MAX_LIVE_GEMS
            );
        }
        let id = state.next_handle_id;
        state.next_handle_id += 1;
        (id, state.drivers.first().cloned())
    };

    // Allocate contiguous physical memory via VMO (lock released).
    let vmo = VmObject::new_contiguous(pages(size), 12).ok()?;
    let phys_addr = vmo.commit_page(0, MMUFlags::READ).ok()? as u64;

    let handle = GemHandle {
        id,
        size,
        phys_addr,
    };

    // Tell the driver about the new buffer (if any). Without a driver the dumb
    // buffer is software-only and always succeeds.
    let accepted = match driver {
        Some(driver) => driver.import_buffer(handle),
        None => true,
    };
    if accepted {
        // Re-acquire only for the bookkeeping push.
        DRM_STATE.lock().handles.push((handle, vmo, current_pid()));
        Some(handle)
    } else {
        None
    }
}

/// Export a GEM handle for PRIME: return its `(phys_addr, size, backing VMO)`
/// so it can be wrapped in a dma-buf and shared with another DRM node.
pub fn export_handle(handle_id: u32) -> Option<(u64, usize, Arc<VmObject>)> {
    let state = DRM_STATE.lock();
    state
        .handles
        .iter()
        .find(|(h, _, _)| h.id == handle_id)
        .map(|(h, vmo, _)| (h.phys_addr, h.size, vmo.clone()))
}

/// Import a dma-buf (PRIME): register a new GEM handle over the same backing
/// frames and return its id. The `VmObject` keeps the memory alive.
pub fn import_dmabuf(phys_addr: u64, size: usize, vmo: Arc<VmObject>) -> u32 {
    let mut state = DRM_STATE.lock();
    let id = state.next_handle_id;
    state.next_handle_id += 1;
    let handle = GemHandle {
        id,
        size,
        phys_addr,
    };
    state.handles.push((handle, vmo, current_pid()));
    id
}

pub fn get_handle(handle_id: u32) -> Option<GemHandle> {
    DRM_STATE
        .lock()
        .handles
        .iter()
        .find(|(h, _, _)| h.id == handle_id)
        .map(|(h, _, _)| *h)
}

/// Look up a framebuffer object by id (`DRM_IOCTL_MODE_GETFB`/`GETFB2`).
pub fn get_fb(fb_id: u32) -> Option<DrmFramebuffer> {
    DRM_STATE
        .lock()
        .framebuffers
        .iter()
        .find(|f| f.id == fb_id)
        .copied()
}

/// Create a framebuffer from a GEM handle
pub fn create_fb(handle_id: u32, width: u32, height: u32, pitch: u32) -> Option<u32> {
    let handle = get_handle(handle_id)?;

    // The framebuffer must fit within the backing GEM buffer: `scanout()` maps
    // `size` bytes from the buffer's contiguous phys range and blits them, so a
    // fb larger than its buffer would read past the VMO into adjacent physical
    // RAM (info leak / fault). Compute in usize with a checked multiply and
    // reject ADDFB whose dimensions overflow or exceed the buffer.
    let size = (pitch as usize).checked_mul(height as usize)?;
    if size == 0 || size > handle.size {
        return None;
    }

    // Only a hardware-KMS driver needs its own framebuffer object. On the
    // software-KMS path the fb is scanned out purely via the display's
    // `blit_from`, so a `driver.create_fb` (a GSP RPC on the NVIDIA driver) is
    // both useless and leaked — RMFB has no driver-side destroy path.
    let driver_fb_id = if software_kms_active() {
        None
    } else {
        get_primary_driver().and_then(|driver| driver.create_fb(handle_id, width, height, pitch))
    };

    let mut state = DRM_STATE.lock();
    let fb_id = state.next_fb_id;
    state.next_fb_id += 1;

    let fb = DrmFramebuffer {
        id: fb_id,
        driver_fb_id,
        gem_handle_id: handle_id,
        width,
        height,
        pitch,
        phys_addr: handle.phys_addr,
        size,
    };

    state.framebuffers.push(fb);
    Some(fb_id)
}

/// Remove a framebuffer (DRM_IOCTL_MODE_RMFB).
pub fn rmfb(fb_id: u32) -> bool {
    let mut state = DRM_STATE.lock();
    if state.crtc_fb == fb_id {
        state.crtc_fb = 0;
    }
    if let Some(pos) = state.framebuffers.iter().position(|f| f.id == fb_id) {
        state.framebuffers.remove(pos);
        true
    } else {
        false
    }
}

/// Native mode of the primary framebuffer display: `(width, height, pitch)`.
pub fn display_mode() -> Option<(u32, u32, u32)> {
    let info = primary_display()?.info();
    Some((info.width, info.height, info.pitch()))
}

/// Bind a framebuffer to a CRTC (the value reported back by GETCRTC).
pub fn set_crtc_fb(_crtc_id: u32, fb_id: u32) {
    DRM_STATE.lock().crtc_fb = fb_id;
}

/// Rows per [`blit_chunked`] band. 32 rows is ~256 KiB at 1920-wide ARGB —
/// ≲100 µs even at modest write-combining throughput, so the interrupts-off
/// window per band stays far below a timer tick — while the per-band overhead
/// (the intr_off/intr_on pair, the blit_from call) stays negligible next to
/// the copy itself. Each band's copy is short enough that at most one IRQ
/// nests per between-band window, the same worst case the original whole-blit
/// intr_off fix was defending against.
const BLIT_CHUNK_ROWS: u32 = 32;

/// Blit `pixels` (row-major, `src_stride` u32s per row, already offset so
/// `pixels[0]` is the rectangle's top-left) into `display` at
/// `(dst_x, dst_y)`, `width`x`height`, in horizontal bands with interrupts
/// re-enabled between bands.
///
/// IRQs nest on the coroutine stack, so any single blit must run with
/// interrupts fully off end-to-end — that's what stopped the nested-IRQ
/// coroutine-stack overflow (`rip=0x3` / `[rsp0]=0x13486`) seen at labwc
/// bring-up (APIC timer -> xHCI EventListener / DRM timer re-entering
/// mid-scanout). But holding IF clear for an entire full-frame copy means
/// every timer tick, keyboard IRQ and xHCI completion queues up behind it,
/// which was a visible chunk of the "labwc feels slow" gap versus Linux.
/// Chunking bounds the continuous interrupts-off window to one band — pending
/// IRQs are serviced promptly between bands — while each individual blit
/// still runs fully protected, preserving the original crash fix.
fn blit_chunked(
    display: &Arc<dyn DisplayScheme>,
    dst_x: u32,
    dst_y: u32,
    pixels: &[u32],
    src_stride: usize,
    width: u32,
    height: u32,
) {
    let mut row = 0u32;
    while row < height {
        let band_h = BLIT_CHUNK_ROWS.min(height - row);
        let src_off = (row as usize) * src_stride;
        if src_off >= pixels.len() {
            break;
        }
        let irq_on = kernel_hal::interrupt::intr_get();
        if irq_on {
            kernel_hal::interrupt::intr_off();
        }
        display.blit_from(
            dst_x,
            dst_y + row,
            &pixels[src_off..],
            src_stride,
            width,
            band_h,
        );
        if irq_on {
            kernel_hal::interrupt::intr_on();
        }
        row += band_h;
    }
}

/// Copy a framebuffer's pixels to the hardware display ("scan out").
///
/// Used by the software KMS path (no GPU driver): the dumb buffer is contiguous
/// physical memory, which we map and blit into the display framebuffer.
pub fn scanout(fb_id: u32) -> bool {
    scanout_region(fb_id, None)
}

/// Like [`scanout`], but when `rect` (`x, y, width, height`, in the
/// framebuffer's own coordinates) is given, blits only that region instead of
/// the whole frame.
///
/// `DRM_IOCTL_MODE_DIRTYFB` uses this to skip re-copying pixels the client
/// didn't touch — `None` (page-flip / modeset) always repaints everything, as
/// does an out-of-range or degenerate `rect` (matches real DIRTYFB semantics:
/// no usable clip means "everything is dirty").
pub fn scanout_region(fb_id: u32, rect: Option<(u32, u32, u32, u32)>) -> bool {
    let fb = {
        let state = DRM_STATE.lock();
        match state.framebuffers.iter().find(|f| f.id == fb_id) {
            Some(f) => *f,
            None => {
                warn!("[drm] scanout: fb_id={} not found", fb_id);
                return false;
            }
        }
    };
    let display = match primary_display() {
        Some(d) => d,
        None => {
            warn!("[drm] scanout: no display -- software_kms inactive, nothing to blit to");
            return false;
        }
    };
    if fb.phys_addr == 0 || fb.size == 0 {
        return false;
    }
    // Log the first scanout so a console photo confirms pixels are flowing.
    if !SCANOUT_LOGGED.swap(true, Ordering::Relaxed) {
        warn!(
            "[drm] scanout: fb={} {}x{} pitch={} phys={:#x} -> display {}x{}",
            fb_id,
            fb.width,
            fb.height,
            fb.pitch,
            fb.phys_addr,
            display.info().width,
            display.info().height
        );
    }
    let info = display.info();
    let vaddr = phys_to_virt(fb.phys_addr as usize);
    // SAFETY: the buffer is contiguous physical memory of `fb.size` bytes,
    // identity-mapped into the kernel's physmap window at `vaddr`.
    let pixels = unsafe { core::slice::from_raw_parts(vaddr as *const u32, fb.size / 4) };
    let src_stride = (fb.pitch / 4) as usize;
    let fb_width = fb.width.min(info.width);
    let fb_height = fb.height.min(info.height);
    // Clamp the requested damage rect to the framebuffer/display bounds. An
    // empty result (e.g. a fully off-screen clip) just means no pixels moved.
    let (blit_x, blit_y, blit_w, blit_h) = match rect {
        Some((x, y, w, h)) => {
            let x = x.min(fb_width);
            let y = y.min(fb_height);
            (x, y, w.min(fb_width - x), h.min(fb_height - y))
        }
        None => (0, 0, fb_width, fb_height),
    };
    if blit_w == 0 || blit_h == 0 {
        return true;
    }
    // CE-offloaded present: copy the dumb buffer (sysmem) into the scanout FB
    // (the console GPU's VRAM) with the GPU copy engine instead of a CPU
    // memcpy over PCIe. A flat CE copy needs equal strides, so only when the
    // dumb-buffer pitch matches the scanout pitch; and only the console GPU
    // (state-loaded via /proc/gpustep14) accepts it. Any miss -> CPU blit.
    // Only ever attempted for a full-frame scanout: the CE path always copies
    // from `fb.phys_addr` (the buffer's start), so it can't honor a rect
    // offset.
    //
    // OPT-IN: gated on `nvidia.cepresent` (see CE_PRESENT_ENABLED) — the CE
    // DMA per present destabilized the desktop on real hardware, so the
    // default present is the proven CPU blit.
    let mut blitted_by_ce = false;
    if rect.is_none() && CE_PRESENT_ENABLED.load(Ordering::Relaxed) && fb.pitch == info.pitch {
        let size = (info.pitch as u64) * (blit_h as u64);
        for d in kernel_hal::drivers::all_drm().as_vec().iter() {
            if d.ce_present(fb.phys_addr, size) {
                blitted_by_ce = true;
                break;
            }
        }
    }
    if !blitted_by_ce {
        // Banded blit with IRQs briefly re-enabled between bands — see
        // [`blit_chunked`]. Honours a DIRTYFB damage rect when present.
        let src_off = (blit_y as usize)
            .saturating_mul(src_stride)
            .saturating_add(blit_x as usize);
        if src_off < pixels.len() {
            blit_chunked(
                &display,
                blit_x,
                blit_y,
                &pixels[src_off..],
                src_stride,
                blit_w,
                blit_h,
            );
        }
    }
    // Composite the kernel cursor on top of the just-blitted frame, so a
    // page-flip never erases the pointer. Snapshot the cursor under the lock,
    // then blend lock-free (blending reads the slow PCIe framebuffer for
    // antialiased edges, which must not run with the DRM spinlock held).
    let cursor = {
        let mut state = DRM_STATE.lock();
        let c = &state.cursor;
        let snap = if c.visible && c.w > 0 && c.h > 0 {
            c.bitmap
                .as_ref()
                .filter(|b| !b.is_empty())
                .map(|b| (c.x, c.y, c.w, c.h, b.clone()))
        } else {
            None
        };
        // A full scanout repaints the whole frame and composites the cursor at
        // its current position, so that — not whatever the last partial move
        // left — is now what's drawn. Keeping `drawn` in step here stops the
        // next `repaint_for_cursor` from leaving a ghost of the pre-flip cursor.
        state.cursor.drawn = snap.as_ref().map(|(x, y, w, h, _)| (*x, *y, *w, *h));
        snap
    };
    if let Some((cx, cy, cw, ch, bmp)) = cursor {
        display.blit_argb_over(cx, cy, &bmp, cw as usize, cw, ch);
    }
    let _ = display.flush();
    // A DRM client owns the framebuffer now: stop the kernel text console from
    // drawing over it (like fbcon yielding to KMS). Restored on DROP_MASTER.
    kernel_hal::console::set_kd_mode(kernel_hal::console::KD_GRAPHICS);
    true
}

/// Set (or hide) the cursor bitmap from a GEM handle (`DRM_MODE_CURSOR_BO`).
///
/// `handle_id == 0` (or a zero-sized image) hides the cursor. Otherwise the
/// client's cursor BO is a tightly-packed premultiplied-ARGB8888 dumb buffer of
/// `w`x`h` pixels; copy it out so a later GEM_CLOSE / reuse can't tear the image
/// mid-scanout. Returns true if the cursor state changed enough to warrant a
/// repaint.
pub fn set_cursor_bo(handle_id: u32, w: u32, h: u32) -> bool {
    let mut state = DRM_STATE.lock();
    if handle_id == 0 || w == 0 || h == 0 {
        let was_visible = state.cursor.visible;
        state.cursor.visible = false;
        return was_visible;
    }
    let handle = match state.handles.iter().find(|(g, _, _)| g.id == handle_id) {
        Some((g, _, _)) => *g,
        None => {
            state.cursor.visible = false;
            return true;
        }
    };
    let px = (w as usize).saturating_mul(h as usize);
    if px == 0 || handle.size < px * 4 || handle.phys_addr == 0 {
        state.cursor.visible = false;
        return true;
    }
    let vaddr = phys_to_virt(handle.phys_addr as usize);
    // SAFETY: contiguous physical dumb buffer of `handle.size` bytes,
    // identity-mapped at `vaddr`; we read exactly `px` u32 pixels (<= size/4).
    let src = unsafe { core::slice::from_raw_parts(vaddr as *const u32, px) };
    // Fresh Arc shared by subsequent scanouts (no per-flip Vec clone).
    state.cursor.bitmap = Some(Arc::from(src));
    state.cursor.w = w;
    state.cursor.h = h;
    state.cursor.visible = true;
    true
}

/// Move the cursor's top-left to `(x, y)` in output pixels
/// (`DRM_MODE_CURSOR_MOVE`). The compositor has already applied the hotspot.
pub fn move_cursor(x: i32, y: i32) {
    let mut state = DRM_STATE.lock();
    state.cursor.x = x;
    state.cursor.y = y;
}

/// Make a cursor set/move take effect immediately (the legacy cursor ioctls
/// carry no page-flip of their own) WITHOUT re-blitting the whole frame.
///
/// The pointer moves far more often than the scene changes — wlroots issues a
/// `DRM_MODE_CURSOR_MOVE` per input event — so a full `scanout()` per move was
/// blitting ~16 MB every time the mouse twitched, which is precisely why the
/// "software cursor" pegged the CPU on real hardware. Instead, restore just the
/// rectangle the old cursor occupied from the CRTC framebuffer (the composited
/// scene, which has no cursor baked in) and composite the new cursor on top.
/// Only two ~64x64 windows are touched per move.
pub fn repaint_for_cursor() {
    if !software_kms_active() {
        return;
    }
    // Snapshot everything needed under the lock, and record the rect we are
    // about to draw so the *next* move knows what to erase.
    let (fb_id, old_rect, new) = {
        let mut st = DRM_STATE.lock();
        // While a text VT is foreground the compositor's pixels are suppressed;
        // don't scribble a cursor over the console.
        if st.graphics_vt != Some(kernel_hal::console::active_vt()) {
            return;
        }
        let fb_id = st.crtc_fb;
        let old_rect = st.cursor.drawn;
        let c = &st.cursor;
        let new = if c.visible && c.w > 0 && c.h > 0 {
            c.bitmap
                .as_ref()
                .filter(|b| !b.is_empty())
                .map(|b| (c.x, c.y, c.w, c.h, b.clone()))
        } else {
            None
        };
        st.cursor.drawn = new.as_ref().map(|(x, y, w, h, _)| (*x, *y, *w, *h));
        (fb_id, old_rect, new)
    };
    if fb_id == 0 {
        return;
    }
    let fb = {
        let state = DRM_STATE.lock();
        match state.framebuffers.iter().find(|f| f.id == fb_id) {
            Some(f) => *f,
            None => return,
        }
    };
    let display = match primary_display() {
        Some(d) => d,
        None => return,
    };
    if fb.phys_addr == 0 || fb.size == 0 {
        return;
    }
    let info = display.info();
    let (fw, fh) = (info.width, info.height);
    let vaddr = phys_to_virt(fb.phys_addr as usize);
    // SAFETY: contiguous physical framebuffer of `fb.size` bytes, identity
    // mapped at `vaddr`; read as `fb.size / 4` u32 pixels.
    let pixels = unsafe { core::slice::from_raw_parts(vaddr as *const u32, fb.size / 4) };
    let src_stride = (fb.pitch / 4) as usize;
    // Erase the old cursor by restoring the scene under it. Grow the restored
    // rect by one pixel on every side so an antialiased edge from the previous
    // composite is fully overwritten.
    if let Some((ox, oy, ow, oh)) = old_rect {
        restore_rect(
            &*display,
            pixels,
            src_stride,
            fw,
            fh,
            ox - 1,
            oy - 1,
            ow + 2,
            oh + 2,
        );
    }
    // Composite the cursor at its new position.
    if let Some((nx, ny, nw, nh, bmp)) = new {
        display.blit_argb_over(nx, ny, &bmp, nw as usize, nw, nh);
    }
    let _ = display.flush();
}

/// Restore the `(x, y, w, h)` window of the display from the CRTC framebuffer
/// `pixels` (row-major, `src_stride` pixels/row), clipped to the visible
/// `fw`x`fh` area. Used to erase the old cursor before drawing the new one.
fn restore_rect(
    display: &dyn DisplayScheme,
    pixels: &[u32],
    src_stride: usize,
    fw: u32,
    fh: u32,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
) {
    if src_stride == 0 {
        return;
    }
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w as i32).min(fw as i32);
    let y1 = (y + h as i32).min(fh as i32);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let cw = (x1 - x0) as u32;
    let ch = (y1 - y0) as u32;
    let off = y0 as usize * src_stride + x0 as usize;
    if off >= pixels.len() {
        return;
    }
    display.blit_from(x0 as u32, y0 as u32, &pixels[off..], src_stride, cw, ch);
}

/// Page-flip to `fb_id` and queue a completion event for the card fd.
///
/// `crtc_id`/`user_data` come from the page-flip request and are echoed back in
/// the `drm_event_vblank` so libdrm's event loop can match the flip.
/// Synthetic vblank period. 60 Hz is the rate every KMS mode we advertise runs
/// at, so pacing flip completions to it makes an unthrottled compositor loop
/// render at 60 fps instead of thousands — the difference between an idle-ish
/// core and a pegged one under emulation.
const VBLANK_PERIOD: Duration = Duration::from_nanos(16_666_667);

pub fn page_flip(fb_id: u32, crtc_id: u32, user_data: u64) -> bool {
    let flipped = present_now(fb_id, crtc_id);
    if flipped {
        schedule_flip_event(crtc_id, user_data);
    }
    flipped
}

/// Deliver a page-flip completion at the next synthetic vblank boundary rather
/// than immediately. This is the throttle that keeps a Wayland compositor's
/// frame loop from spinning: it renders a frame, page-flips, then blocks in
/// `poll()`/`read()` on the card fd until we post the flip event here — so the
/// loop runs at most once per [`VBLANK_PERIOD`]. If the previous vblank is
/// already in the past (compositor was idle, or renders slower than 60 Hz) the
/// event still fires one period out, never faster.
/// Coalesced pending DRM completions: at most one `timer_set` arm. Labwc /
/// lunarbar used to enqueue a fresh Box per flip/vblank, amplifying IRQ
/// callback churn at desktop bring-up.
#[derive(Clone, Copy)]
enum PendingDrmTimer {
    Flip { crtc_id: u32, user_data: u64 },
    Vblank { seq: u32, signal: u64 },
}

static PENDING_DRM_TIMER: Mutex<Option<PendingDrmTimer>> = Mutex::new(None);
static DRM_TIMER_ARMED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

fn deliver_pending_drm_timer() {
    DRM_TIMER_ARMED.store(false, Ordering::Release);
    let job = PENDING_DRM_TIMER.lock().take();
    match job {
        Some(PendingDrmTimer::Flip { crtc_id, user_data }) => {
            queue_flip_event(crtc_id, user_data);
        }
        Some(PendingDrmTimer::Vblank { seq, signal }) => {
            queue_vblank_event(seq, signal);
        }
        None => {}
    }
    // Something scheduled while we delivered — arm one more shot.
    if PENDING_DRM_TIMER.lock().is_some() {
        arm_coalesced_drm_timer_locked();
    }
}

fn arm_coalesced_drm_timer_locked() {
    // `swap(true)` returns the previous value: if it was already armed, done.
    if DRM_TIMER_ARMED.swap(true, Ordering::AcqRel) {
        return;
    }
    let deadline = next_vblank_deadline();
    kernel_hal::timer::timer_set(deadline, Box::new(move |_| deliver_pending_drm_timer()));
}

fn arm_coalesced_drm_timer(pending: PendingDrmTimer) {
    *PENDING_DRM_TIMER.lock() = Some(pending);
    arm_coalesced_drm_timer_locked();
}

fn schedule_flip_event(crtc_id: u32, user_data: u64) {
    arm_coalesced_drm_timer(PendingDrmTimer::Flip { crtc_id, user_data });
}

/// Deliver a `DRM_EVENT_VBLANK` (from a `WAIT_VBLANK` that asked for an event)
/// at the next synthetic vblank instead of immediately, for the same anti-spin
/// reason as [`schedule_flip_event`]. `signal` is the caller's opaque token
/// echoed back in the event's `user_data`.
pub fn schedule_vblank_event(signal: u64) {
    let seq = vblank_seq_now().wrapping_add(1);
    arm_coalesced_drm_timer(PendingDrmTimer::Vblank { seq, signal });
}

/// Advance the synthetic vblank clock and return the monotonic instant the next
/// completion event may fire at: one [`VBLANK_PERIOD`] past the previous vblank,
/// but never in the past. This is what paces both page-flip and vblank-wait
/// event delivery to ~60 Hz.
fn next_vblank_deadline() -> Duration {
    let now = kernel_hal::timer::timer_now();
    let mut st = DRM_STATE.lock();
    let mut next = st.next_vblank + VBLANK_PERIOD;
    if next <= now {
        next = now + VBLANK_PERIOD;
    }
    st.next_vblank = next;
    next
}

/// Present a framebuffer immediately on `crtc_id` without queuing a DRM flip
/// event. Used by SETCRTC/SETPLANE paths that are not page-flip ioctls.
/// Re-blit the compositor's last frame IF its VT is the active one. Called
/// right after a VT switch so returning to the compositor (Ctrl+Alt+F1) shows
/// its last frame immediately instead of a stale/blank screen; a no-op when the
/// switch was TO a text console (which the kernel repaints itself).
pub fn represent_if_owner_active() -> bool {
    let fb = {
        let st = DRM_STATE.lock();
        if st.graphics_vt != Some(kernel_hal::console::active_vt()) {
            return false;
        }
        st.crtc_fb
    };
    if fb == 0 {
        return false;
    }
    scanout(fb)
}

/// Forget the compositor's VT ownership (on DROP_MASTER / compositor exit) so a
/// subsequent text-only session is never gated off the display or input.
pub fn clear_graphics_owner() {
    DRM_STATE.lock().graphics_vt = None;
}

/// One-shot: log the first present so a black-screen bring-up shows whether the
/// compositor is presenting at all, and via which path.
static PRESENT_LOGGED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
/// One-shot: log the first VT-gated drop so we can tell "compositor never
/// presented" (no present log) from "presents are being suppressed because a
/// text VT is foreground" (this log).
static PRESENT_VT_DROP_LOGGED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

pub fn present_now(fb_id: u32, crtc_id: u32) -> bool {
    present_now_region(fb_id, crtc_id, None)
}

/// Like [`present_now`], but `rect` (`x, y, width, height`) restricts the
/// software-KMS blit to that region instead of the whole frame — see
/// [`scanout_region`]. `DRM_IOCTL_MODE_DIRTYFB`'s clip rects flow through
/// here. Ignored on the hardware-KMS path: a real driver's `page_flip` scans
/// out via its own GPU DMA, not the CPU blit this exists to shrink.
pub fn present_now_region(fb_id: u32, crtc_id: u32, rect: Option<(u32, u32, u32, u32)>) -> bool {
    if !PRESENT_LOGGED.swap(true, Ordering::Relaxed) {
        // Read `graphics_vt` into a local FIRST: `DRM_STATE.lock()` as a direct
        // argument to `warn!` keeps the MutexGuard temporary alive for the
        // WHOLE macro statement (Rust extends a temporary's lifetime to the end
        // of its enclosing statement) -- i.e. across the log line's formatting
        // and serial write, not just the field read. The VT-gating block right
        // below takes the SAME lock again a few lines later; on a slow serial
        // console that window was wide enough to strand another CPU on it for
        // >8s, tripping the deadlock detector (see `drm.rs` HOLDER traces).
        let graphics_vt = DRM_STATE.lock().graphics_vt;
        warn!(
            "[drm] first present: fb_id={} crtc={} active_vt={} graphics_vt={:?} software_kms={}",
            fb_id,
            crtc_id,
            kernel_hal::console::active_vt(),
            graphics_vt,
            software_kms_active(),
        );
    }
    // Establish / enforce compositor VT ownership. The first present claims the
    // active VT; later presents while a *different* VT is foreground (the user
    // switched to a text console) are dropped — reported as complete so the
    // compositor's frame loop keeps running, but not blitted over the console.
    {
        let active = kernel_hal::console::active_vt();
        let mut st = DRM_STATE.lock();
        match st.graphics_vt {
            None => st.graphics_vt = Some(active),
            Some(owner) if owner != active => {
                if !PRESENT_VT_DROP_LOGGED.swap(true, Ordering::Relaxed) {
                    warn!(
                        "[drm] present DROPPED (VT-gated): owner_vt={} active_vt={} -- compositor frames are suppressed because a different VT is foreground",
                        owner, active
                    );
                }
                return true;
            }
            _ => {}
        }
    }
    let flipped = if software_kms_active() {
        // No usable hardware KMS: blit the dumb buffer to the framebuffer.
        scanout_region(fb_id, rect)
    } else if let Some(driver) = get_primary_driver() {
        // Translate core fb id to the driver's private fb id when available.
        let driver_fb_id = DRM_STATE
            .lock()
            .framebuffers
            .iter()
            .find(|f| f.id == fb_id)
            .and_then(|f| f.driver_fb_id)
            .unwrap_or(fb_id);
        // Hardware driver owns scanout; fall back to a software blit if it
        // declines.
        driver.page_flip(driver_fb_id) || scanout_region(fb_id, rect)
    } else {
        scanout_region(fb_id, rect)
    };
    if flipped {
        set_crtc_fb(crtc_id, fb_id);
        // A DRM client owns the framebuffer now: stop text console drawing.
        kernel_hal::console::set_kd_mode(kernel_hal::console::KD_GRAPHICS);
    }
    flipped
}

/// Encode and enqueue a `struct drm_event_vblank` for the card fd.
///
/// Shared by page-flip completions (`DRM_EVENT_FLIP_COMPLETE`) and vblank waits
/// (`DRM_EVENT_VBLANK`), which use the identical 32-byte wire layout — only the
/// `type` field distinguishes them for libdrm's event dispatcher.
fn push_drm_event(ev_type: u32, crtc_id: u32, seq: u32, user_data: u64) {
    let now = kernel_hal::timer::timer_now();
    // struct drm_event_vblank { u32 type; u32 length; u64 user_data;
    //   u32 tv_sec; u32 tv_usec; u32 sequence; u32 crtc_id; }  (32 bytes)
    // Fixed stack buffer — no heap alloc from the timer IRQ path.
    let mut buf = [0u8; 32];
    buf[0..4].copy_from_slice(&ev_type.to_ne_bytes());
    buf[4..8].copy_from_slice(&32u32.to_ne_bytes());
    buf[8..16].copy_from_slice(&user_data.to_ne_bytes());
    buf[16..20].copy_from_slice(&(now.as_secs() as u32).to_ne_bytes());
    buf[20..24].copy_from_slice(&now.subsec_micros().to_ne_bytes());
    buf[24..28].copy_from_slice(&seq.to_ne_bytes());
    buf[28..32].copy_from_slice(&crtc_id.to_ne_bytes());
    let mut state = DRM_STATE.lock();
    state.events.push_back(buf.to_vec());
    state.eventbus.lock().set(Event::READABLE);
}

/// Enqueue a `DRM_EVENT_FLIP_COMPLETE` for a completed page flip.
fn queue_flip_event(crtc_id: u32, user_data: u64) {
    const DRM_EVENT_FLIP_COMPLETE: u32 = 2;
    let seq = FLIP_SEQ.fetch_add(1, Ordering::Relaxed);
    push_drm_event(DRM_EVENT_FLIP_COMPLETE, crtc_id, seq, user_data);
}

/// Enqueue a `DRM_EVENT_VBLANK` for a `WAIT_VBLANK` request that asked for an
/// event (`_DRM_VBLANK_EVENT`) instead of blocking.
pub fn queue_vblank_event(seq: u32, user_data: u64) {
    const DRM_EVENT_VBLANK: u32 = 1;
    push_drm_event(DRM_EVENT_VBLANK, SYNTH_CRTC_ID, seq, user_data);
}

/// Synthetic ~60 Hz vertical-blank counter derived from the monotonic clock.
///
/// A software framebuffer has no real vblank interrupt, but `WAIT_VBLANK`
/// callers expect a monotonically increasing sequence; deriving one from time
/// keeps both absolute and relative queries sane.
pub fn vblank_seq_now() -> u32 {
    let now = kernel_hal::timer::timer_now();
    (now.as_nanos() * 60 / 1_000_000_000) as u32
}

/// Pop one pending DRM event into `buf`, returning the number of bytes copied,
/// or `None` if there are no events queued.
pub fn read_event(buf: &mut [u8]) -> Option<usize> {
    let mut state = DRM_STATE.lock();
    let ev = state.events.front()?;
    if buf.len() < ev.len() {
        // Caller's buffer is too small for a whole event; libdrm always reads
        // with a large buffer, so just report "nothing yet" rather than
        // delivering a truncated, unparsable event.
        return None;
    }
    let n = ev.len();
    buf[..n].copy_from_slice(&ev[..n]);
    state.events.pop_front();
    if state.events.is_empty() {
        state.eventbus.lock().clear(Event::READABLE);
    }
    Some(n)
}

/// Whether any DRM events are queued for reading.
pub fn has_events() -> bool {
    !DRM_STATE.lock().events.is_empty()
}

/// Expose the DRM event bus
pub fn get_eventbus() -> Arc<Mutex<EventBus>> {
    DRM_STATE.lock().eventbus.clone()
}

/// Create a KMS property blob (`DRM_IOCTL_MODE_CREATEPROPBLOB`) and return its
/// id. `user_created` distinguishes client blobs (destroyable) from
/// kernel-owned ones (current mode), mirroring Linux's ownership rule.
pub fn create_blob(data: Vec<u8>, user_created: bool) -> u32 {
    let mut state = DRM_STATE.lock();
    let id = state.next_blob_id;
    state.next_blob_id += 1;
    state.blobs.push(DrmBlob {
        id,
        user_created,
        data,
    });
    id
}

/// Look up a blob's contents (`DRM_IOCTL_MODE_GETPROPBLOB`).
pub fn get_blob(id: u32) -> Option<Vec<u8>> {
    DRM_STATE
        .lock()
        .blobs
        .iter()
        .find(|b| b.id == id)
        .map(|b| b.data.clone())
}

/// Outcome of `DRM_IOCTL_MODE_DESTROYPROPBLOB` (Linux: ENOENT / EPERM split).
pub enum BlobDestroy {
    /// Blob removed.
    Destroyed,
    /// No blob with that id.
    NotFound,
    /// Kernel-created blob (current mode, …): only the creator may destroy.
    KernelOwned,
}

/// Destroy a user-created blob.
pub fn destroy_blob(id: u32) -> BlobDestroy {
    let mut state = DRM_STATE.lock();
    match state.blobs.iter().position(|b| b.id == id) {
        None => BlobDestroy::NotFound,
        Some(pos) if !state.blobs[pos].user_created => BlobDestroy::KernelOwned,
        Some(pos) => {
            state.blobs.remove(pos);
            BlobDestroy::Destroyed
        }
    }
}

/// Snapshot `(atomic KMS state, current CRTC fb id)` for property readback.
pub fn atomic_snapshot() -> (AtomicKmsState, u32) {
    let state = DRM_STATE.lock();
    (state.atomic, state.crtc_fb)
}

/// Staged property updates parsed from one `DRM_IOCTL_MODE_ATOMIC` request
/// against the synthetic (software-KMS) pipeline. `None` = property not in
/// the request; object state not touched persists across commits, as the
/// atomic uAPI requires.
#[derive(Default, Clone, Copy)]
pub struct AtomicUpdate {
    /// Plane "FB_ID" (`Some(0)` disables the plane).
    pub plane_fb_id: Option<u32>,
    /// Plane "CRTC_ID" (`Some(0)` detaches the plane).
    pub plane_crtc_id: Option<u32>,
    /// Plane "CRTC_X".
    pub crtc_x: Option<i32>,
    /// Plane "CRTC_Y".
    pub crtc_y: Option<i32>,
    /// Plane "CRTC_W".
    pub crtc_w: Option<u32>,
    /// Plane "CRTC_H".
    pub crtc_h: Option<u32>,
    /// Plane "SRC_X" (16.16 fixed point).
    pub src_x: Option<u32>,
    /// Plane "SRC_Y" (16.16).
    pub src_y: Option<u32>,
    /// Plane "SRC_W" (16.16).
    pub src_w: Option<u32>,
    /// Plane "SRC_H" (16.16).
    pub src_h: Option<u32>,
    /// CRTC "ACTIVE".
    pub active: Option<bool>,
    /// CRTC "MODE_ID" blob id (`Some(0)` clears the mode).
    pub mode_blob: Option<u32>,
    /// Connector "CRTC_ID" (`Some(0)` detaches the connector).
    pub connector_crtc_id: Option<u32>,
}

/// Why an atomic check/commit was refused. Mapped to errno by the ioctl
/// dispatcher (`Invalid` → EINVAL, `NotFound` → ENOENT, `Device` → EIO).
pub enum AtomicError {
    /// Malformed value, rect out of bounds, or a modeset without
    /// `DRM_MODE_ATOMIC_ALLOW_MODESET`.
    Invalid,
    /// A referenced object (framebuffer, blob, CRTC) does not exist.
    NotFound,
    /// The present itself failed.
    Device,
}

/// Validate and (unless `test_only`) apply an atomic update, queueing one
/// page-flip event when `want_event` — the `DRM_MODE_ATOMIC_TEST_ONLY` /
/// `ALLOW_MODESET` / `PAGE_FLIP_EVENT` semantics of `DRM_IOCTL_MODE_ATOMIC`.
///
/// The pipeline is fixed (one CRTC/connector/plane, one mode), so "check"
/// means: referenced objects exist, the mode blob is a well-formed
/// `drm_mode_modeinfo` matching the native mode, source rects fit the
/// framebuffer, and mode/active changes carry `ALLOW_MODESET`.
pub fn atomic_commit(
    upd: &AtomicUpdate,
    test_only: bool,
    allow_modeset: bool,
    want_event: bool,
    user_data: u64,
) -> Result<(), AtomicError> {
    let (cur, _) = atomic_snapshot();

    // --- Check phase (no state touched) ---
    // CRTC references must name the synthetic CRTC (or 0 = detach).
    for crtc_ref in [upd.plane_crtc_id, upd.connector_crtc_id].iter().flatten() {
        if *crtc_ref != 0 && *crtc_ref != SYNTH_CRTC_ID {
            return Err(AtomicError::NotFound);
        }
    }

    // MODE_ID blob: must exist, be exactly one drm_mode_modeinfo (68 bytes),
    // and name the panel's fixed mode — there is nothing else to program.
    let mut mode_change = false;
    if let Some(blob_id) = upd.mode_blob {
        if blob_id != 0 {
            let data = get_blob(blob_id).ok_or(AtomicError::NotFound)?;
            if data.len() != 68 {
                return Err(AtomicError::Invalid);
            }
            let hdisplay = u16::from_ne_bytes([data[4], data[5]]) as u32;
            let vdisplay = u16::from_ne_bytes([data[14], data[15]]) as u32;
            let (w, h, _) = display_mode().ok_or(AtomicError::Device)?;
            if hdisplay != w || vdisplay != h {
                return Err(AtomicError::Invalid);
            }
            mode_change = cur.mode_blob_id == 0;
        } else {
            mode_change = cur.mode_blob_id != 0;
        }
    }
    let active_change = upd.active.map(|a| a != cur.active).unwrap_or(false);
    if (mode_change || active_change) && !allow_modeset {
        // Linux: "[CRTC] requires full modeset" -> EINVAL without the flag.
        return Err(AtomicError::Invalid);
    }
    // ACTIVE=1 needs a mode (committed now or earlier).
    let ends_with_mode = match upd.mode_blob {
        Some(b) => b != 0,
        None => cur.mode_blob_id != 0,
    };
    if upd.active == Some(true) && !ends_with_mode {
        return Err(AtomicError::Invalid);
    }

    // Plane: an attached framebuffer must exist and contain the source rect.
    if let Some(fb_id) = upd.plane_fb_id {
        if fb_id != 0 {
            let fb = get_fb(fb_id).ok_or(AtomicError::NotFound)?;
            if upd.plane_crtc_id == Some(0) {
                // FB without a CRTC is invalid atomic plane state.
                return Err(AtomicError::Invalid);
            }
            let end_x = (upd.src_x.unwrap_or(cur.src_x) >> 16)
                .saturating_add(upd.src_w.unwrap_or(cur.src_w) >> 16);
            let end_y = (upd.src_y.unwrap_or(cur.src_y) >> 16)
                .saturating_add(upd.src_h.unwrap_or(cur.src_h) >> 16);
            if end_x > fb.width || end_y > fb.height {
                return Err(AtomicError::Invalid);
            }
        }
    }

    if test_only {
        return Ok(());
    }

    // --- Commit phase ---
    {
        let mut state = DRM_STATE.lock();
        if let Some(blob_id) = upd.mode_blob {
            if blob_id == 0 {
                state.atomic.mode_blob_id = 0;
            } else if let Some(data) = state
                .blobs
                .iter()
                .find(|b| b.id == blob_id)
                .map(|b| b.data.clone())
            {
                // Copy the client's mode into a kernel-owned blob: the client
                // may DESTROYPROPBLOB its own right after the commit (wlroots
                // does), and MODE_ID readback must survive that.
                let cur_id = state.atomic.mode_blob_id;
                if let Some(existing) = state.blobs.iter_mut().find(|b| b.id == cur_id) {
                    existing.data = data;
                } else {
                    let id = state.next_blob_id;
                    state.next_blob_id += 1;
                    state.blobs.push(DrmBlob {
                        id,
                        user_created: false,
                        data,
                    });
                    state.atomic.mode_blob_id = id;
                }
            }
        }
        if let Some(a) = upd.active {
            state.atomic.active = a;
        }
        let a = &mut state.atomic;
        if let Some(v) = upd.crtc_x {
            a.crtc_x = v;
        }
        if let Some(v) = upd.crtc_y {
            a.crtc_y = v;
        }
        if let Some(v) = upd.crtc_w {
            a.crtc_w = v;
        }
        if let Some(v) = upd.crtc_h {
            a.crtc_h = v;
        }
        if let Some(v) = upd.src_x {
            a.src_x = v;
        }
        if let Some(v) = upd.src_y {
            a.src_y = v;
        }
        if let Some(v) = upd.src_w {
            a.src_w = v;
        }
        if let Some(v) = upd.src_h {
            a.src_h = v;
        }
    }

    match upd.plane_fb_id {
        Some(0) => set_crtc_fb(SYNTH_CRTC_ID, 0),
        Some(fb_id) if !present_now(fb_id, SYNTH_CRTC_ID) => return Err(AtomicError::Device),
        _ => {}
    }

    if want_event {
        // One event per CRTC in the commit; the pipeline has exactly one.
        // Paced to the synthetic vblank (not delivered now) for the same
        // anti-spin reason as the legacy page-flip path.
        schedule_flip_event(SYNTH_CRTC_ID, user_data);
    }
    Ok(())
}

pub fn get_caps() -> Option<DrmCaps> {
    if !software_kms_active() {
        if let Some(d) = get_primary_driver() {
            return Some(d.get_caps());
        }
    }
    // Software framebuffer fallback.
    let (w, h, _) = display_mode()?;
    Some(DrmCaps {
        has_3d: false,
        has_cursor: false,
        max_width: w,
        max_height: h,
    })
}

/// The pid to charge a new GEM object to, or 0 when there is no current thread
/// (a buffer allocated during boot, which no process exit should ever reclaim).
///
/// `pub(super)`: also called from `drm_scheme.rs`'s ioctl dispatch, to push
/// the caller's pid down into `DrmScheme::ioctl_owned` for drivers with
/// their own per-process resources (see `NvidiaGpu::nouveau_release_process`).
pub(super) fn current_pid() -> u64 {
    use zircon_object::object::KernelObject;
    kernel_hal::thread::get_current_thread()
        .and_then(|t| t.downcast::<zircon_object::task::Thread>().ok())
        .map(|t| t.proc().id())
        .unwrap_or(0)
}

/// Release every GEM object owned by `pid`, plus any framebuffer that was built
/// on one. Called once per process teardown.
///
/// This is the counterpart Linux gets for free from `drm_gem_release()` on the
/// DRM fd's last close. Without it a dumb buffer outlived its creator forever:
/// Xorg's modesetting driver allocates full-screen dumb buffers, the XFCE
/// session dies and respawns, and each generation's buffers stayed committed --
/// physically contiguous, up to 64 MiB each, owned by no address space.
///
/// Safe to do at process teardown specifically: the process's mappings are gone
/// by then, so nothing can still be writing through the `VmObject::new_physical`
/// view that `DrmDev::get_vmo` handed out over the same frames.
///
/// The driver's `free_buffer` runs with `DRM_STATE` RELEASED, for the reason
/// `snapshot_drivers` documents: the lock is an IRQ-disabling spinlock and a
/// driver call is not guaranteed to be cheap.
pub fn release_process(pid: u64) -> usize {
    if pid == 0 {
        return 0;
    }
    let (doomed, driver) = {
        let mut state = DRM_STATE.lock();
        if !state.handles.iter().any(|(_, _, owner)| *owner == pid) {
            return 0;
        }
        let mut doomed = Vec::new();
        state.handles.retain(|(handle, _, owner)| {
            if *owner == pid {
                doomed.push(*handle);
                false
            } else {
                true
            }
        });
        // A framebuffer backed by a handle we just dropped must go too, or
        // scanout would keep presenting freed physical memory.
        state
            .framebuffers
            .retain(|fb| !doomed.iter().any(|h| h.id == fb.gem_handle_id));
        if !state.framebuffers.iter().any(|fb| fb.id == state.crtc_fb) {
            state.crtc_fb = 0;
        }
        let driver = state.drivers.first().cloned();
        (doomed, driver)
    };
    if let Some(d) = driver {
        for handle in &doomed {
            d.free_buffer(*handle);
        }
    }
    let freed: usize = doomed.iter().map(|h| h.size).sum();
    log::info!(
        "[drm] pid={} exited: released {} GEM object(s), {} KiB",
        pid,
        doomed.len(),
        freed / 1024
    );
    doomed.len()
}

pub fn gem_close(handle_id: u32) -> bool {
    let mut state = DRM_STATE.lock();
    if let Some(pos) = state.handles.iter().position(|(h, _, _)| h.id == handle_id) {
        let (handle, _, _) = state.handles[pos];
        let driver = state.drivers.first().cloned();
        let _ = state.handles.remove(pos);
        drop(state);

        if let Some(d) = driver {
            d.free_buffer(handle);
        }
        true
    } else {
        false
    }
}

/// Snapshot the registered drivers so they can be called with `DRM_STATE`
/// RELEASED. `DRM_STATE`'s `lock::Mutex` is an IRQ-disabling spinlock, and a
/// driver query is NOT guaranteed to be cheap: the NVIDIA RM one runs GSP RPCs
/// and a DDC/EDID probe on its first use (labwc's initial GETRESOURCES /
/// GETCONNECTOR), which can take hundreds of milliseconds — or wedge outright
/// on flaky hardware. Holding the spinlock across that stalls this CPU with
/// interrupts off and piles every other DRM caller (scanout, page flips,
/// events) up behind it, each also spinning with interrupts off: the whole
/// machine freezes. Same rule `alloc_buffer` already documents.
fn snapshot_drivers() -> Vec<Arc<dyn DrmScheme>> {
    DRM_STATE.lock().drivers.clone()
}

pub fn get_resources() -> (Vec<u32>, Vec<u32>, Vec<u32>) {
    let (fbs, drivers) = {
        let state = DRM_STATE.lock();
        let fbs: Vec<u32> = state.framebuffers.iter().map(|fb| fb.id).collect();
        (fbs, state.drivers.clone())
    };

    // Software KMS path: synthesize one CRTC + connector so `drmIsKMS()`
    // passes and wlroots drives output through software scanout. Checked
    // FIRST, before touching any driver: `driver.get_resources()` on the
    // NVIDIA driver runs a live NV0073 display-control + DDC/EDID probe over
    // GSP (see nvidia.rs `rm_display_state`), and both GPU instances — the
    // console GPU AND the compute GPU — are registered as DRM drivers. Calling
    // it here would drive display RPCs through the compute GPU on every
    // GETRESOURCES and then throw the result away for the synthetic topology.
    if software_kms_active() {
        debug!(
            "[drm] GETRESOURCES: software KMS -> 1 crtc, 1 connector ({:?})",
            display_mode(),
        );
        return (fbs, vec![SYNTH_CRTC_ID], vec![SYNTH_CONNECTOR_ID]);
    }

    // No framebuffer display: fall back to driver-provided resources. When a
    // hardware-KMS driver is present, only include resources from hardware-KMS
    // drivers. Mixing non-KMS drivers (e.g. VirtIO) alongside them produces a
    // broken DRM topology (multiple CRTCs sharing one synthetic encoder), which
    // makes wlroots fail with "Failed to create DRM backend". If no driver has
    // hardware KMS, include all drivers (e.g. VirtIO-only with no display).
    let has_hardware_kms = drivers.iter().any(|d| d.has_hardware_kms());
    let mut crtcs: Vec<u32> = Vec::new();
    let mut connectors: Vec<u32> = Vec::new();
    for driver in &drivers {
        if !has_hardware_kms || driver.has_hardware_kms() {
            let (_, d_crtcs, d_conns) = driver.get_resources();
            // De-duplicate IDs that can appear across multiple drivers sharing
            // the same global DRM_STATE (e.g. two NVIDIA GPUs that both return
            // the same synthetic CRTC/connector IDs). Duplicate IDs confuse
            // wlroots into creating multiple outputs with identical resource
            // IDs, which leads to 0×0 dumb-buffer allocations and EINVAL.
            for id in d_crtcs {
                if !crtcs.contains(&id) {
                    crtcs.push(id);
                }
            }
            for id in d_conns {
                if !connectors.contains(&id) {
                    connectors.push(id);
                }
            }
        }
    }

    warn!(
        "[drm] GETRESOURCES: crtcs={} connectors={} fbs={} (driver-provided, NO framebuffer display -> scanout will NOT blit)",
        crtcs.len(),
        connectors.len(),
        fbs.len()
    );
    (fbs, crtcs, connectors)
}

pub fn get_connector(id: u32) -> Option<DrmConnector> {
    // Software KMS: serve the synthetic connector directly. Do NOT fall out to
    // the drivers first — `nvidia.get_connector` runs a live GSP/DDC EDID probe
    // (rm_display_state) before it even range-checks the id, so a GETCONNECTOR
    // for the synthetic id would still drive an RPC on both GPUs (incl. the
    // compute GPU) and stall wlroots' bind. See get_resources for detail.
    if !software_kms_active() {
        // Driver calls run with DRM_STATE released — see `snapshot_drivers`.
        for driver in snapshot_drivers() {
            if let Some(conn) = driver.get_connector(id) {
                return Some(conn);
            }
        }
    }
    // Software framebuffer fallback (no driver, or driver without KMS).
    if id != SYNTH_CONNECTOR_ID {
        return None;
    }
    let (w, h, _) = display_mode()?;
    // Prefer the real panel size from the UEFI-captured EDID (bytes 21/22 =
    // max image size in cm); fall back to a ~96 DPI estimate from the mode.
    let (mm_width, mm_height) = match zcore_drivers::display::boot_edid() {
        Some((e, len)) if len >= 23 && (e[21] != 0 || e[22] != 0) => {
            (e[21] as u32 * 10, e[22] as u32 * 10)
        }
        _ => ((w * 254 / 960).max(1), (h * 254 / 960).max(1)),
    };
    Some(DrmConnector {
        id: SYNTH_CONNECTOR_ID,
        connected: true,
        mm_width,
        mm_height,
        connector_type: 11,
    })
}

pub fn get_connector_edid(id: u32) -> Option<[u8; 128]> {
    // Driver calls run with DRM_STATE released — see `snapshot_drivers`.
    for driver in snapshot_drivers() {
        if let Some(edid) = driver.get_connector_edid(id) {
            return Some(edid);
        }
    }
    if id == SYNTH_CONNECTOR_ID {
        if let Some((e, len)) = zcore_drivers::display::boot_edid() {
            if len >= 128 {
                return Some(e);
            }
        }
    }
    None
}

pub fn get_crtc(id: u32) -> Option<DrmCrtc> {
    // Software KMS: serve the synthetic CRTC directly (see get_resources —
    // avoids driving a GSP/EDID probe through the drivers).
    if !software_kms_active() {
        // Driver calls run with DRM_STATE released — see `snapshot_drivers`.
        for driver in snapshot_drivers() {
            if let Some(mut crtc) = driver.get_crtc(id) {
                // Keep userspace-facing fb ids in the DRM core namespace.
                // Read crtc_fb once (a second lock could observe a torn update).
                let crtc_fb = DRM_STATE.lock().crtc_fb;
                if crtc_fb != 0 {
                    crtc.fb_id = crtc_fb;
                }
                return Some(crtc);
            }
        }
    }
    // Software framebuffer fallback (no driver, or driver without KMS).
    if id != SYNTH_CRTC_ID {
        return None;
    }
    display_mode()?;
    let fb_id = DRM_STATE.lock().crtc_fb;
    Some(DrmCrtc {
        id: SYNTH_CRTC_ID,
        fb_id,
        x: 0,
        y: 0,
    })
}

pub fn get_planes() -> Vec<u32> {
    if software_kms_active() {
        // One synthetic primary plane bound to the synthetic CRTC.
        return vec![SYNTH_PLANE_ID];
    }
    // Driver calls run with DRM_STATE released — see `snapshot_drivers`.
    // Mirror the get_resources() filter: when a hardware-KMS driver exists,
    // only expose its planes to avoid a mixed 2-plane topology.
    let drivers = snapshot_drivers();
    let has_hardware_kms = drivers.iter().any(|d| d.has_hardware_kms());
    let mut planes = Vec::new();
    for driver in &drivers {
        if !has_hardware_kms || driver.has_hardware_kms() {
            planes.extend(driver.get_planes());
        }
    }
    planes
}

pub fn get_plane(id: u32) -> Option<DrmPlane> {
    // Software KMS: serve the synthetic plane directly (see get_resources —
    // avoids driving a GSP/EDID probe through the drivers).
    if !software_kms_active() {
        // Driver calls run with DRM_STATE released — see `snapshot_drivers`.
        for driver in snapshot_drivers() {
            if let Some(mut plane) = driver.get_plane(id) {
                let crtc_fb = DRM_STATE.lock().crtc_fb;
                if crtc_fb != 0 {
                    plane.fb_id = crtc_fb;
                }
                return Some(plane);
            }
        }
    }
    if software_kms_active() && id == SYNTH_PLANE_ID {
        return Some(DrmPlane {
            id: SYNTH_PLANE_ID,
            crtc_id: SYNTH_CRTC_ID,
            fb_id: 0,
            possible_crtcs: 1, // bitmask: CRTC index 0
            plane_type: 1,     // DRM_PLANE_TYPE_PRIMARY
        });
    }
    None
}

#[cfg(test)]
mod release_tests {
    use super::*;

    /// Hand-build a GEM entry owned by `pid`, bypassing `alloc_buffer` (which
    /// would need a current thread and real contiguous frames). What is under
    /// test is the bookkeeping: who owns an entry and what releases it.
    fn plant(id: u32, size: usize, pid: u64) {
        let vmo = VmObject::new_paged(1);
        let handle = GemHandle {
            id,
            size,
            phys_addr: 0,
        };
        DRM_STATE.lock().handles.push((handle, vmo, pid));
    }

    fn live_ids() -> Vec<u32> {
        DRM_STATE
            .lock()
            .handles
            .iter()
            .map(|(h, _, _)| h.id)
            .collect()
    }

    #[test]
    fn process_exit_releases_only_that_process_buffers() {
        // Distinct ids/pids so this cannot collide with another test's state.
        plant(9001, 4096, 77_001);
        plant(9002, 8192, 77_001);
        plant(9003, 4096, 77_002);

        // The bug: nothing but an explicit ioctl ever dropped these, so a
        // process that died without one leaked every buffer it had made.
        assert_eq!(release_process(77_001), 2);

        let ids = live_ids();
        assert!(!ids.contains(&9001), "9001 should be gone with its owner");
        assert!(!ids.contains(&9002), "9002 should be gone with its owner");
        assert!(ids.contains(&9003), "another process's buffer must survive");

        // Idempotent: a second teardown for the same pid frees nothing more.
        assert_eq!(release_process(77_001), 0);

        assert_eq!(release_process(77_002), 1);
        assert!(!live_ids().contains(&9003));
    }

    #[test]
    fn unowned_buffers_are_never_reclaimed() {
        // pid 0 means "allocated with no current thread" (boot-time). A process
        // exit must never take those: pid 0 is not a real owner.
        plant(9101, 4096, 0);
        assert_eq!(release_process(0), 0);
        assert!(live_ids().contains(&9101));
        DRM_STATE.lock().handles.retain(|(h, _, _)| h.id != 9101);
    }

    #[test]
    fn releasing_a_buffer_drops_the_framebuffer_built_on_it() {
        plant(9201, 4096, 77_003);
        {
            let mut state = DRM_STATE.lock();
            state.framebuffers.push(DrmFramebuffer {
                id: 9299,
                driver_fb_id: None,
                gem_handle_id: 9201,
                width: 1,
                height: 1,
                pitch: 4,
                phys_addr: 0,
                size: 4096,
            });
            state.crtc_fb = 9299;
        }
        assert_eq!(release_process(77_003), 1);
        let state = DRM_STATE.lock();
        assert!(
            !state.framebuffers.iter().any(|fb| fb.id == 9299),
            "a framebuffer over a freed handle would scan out released memory"
        );
        assert_eq!(state.crtc_fb, 0, "the CRTC must not point at a dropped fb");
    }
}
