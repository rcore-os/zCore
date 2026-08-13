//! DRM (Direct Rendering Manager) Scheme for zCore
//!
//! Exposes the DRM subsystem to userspace via IOCTLs and memory mapping.

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::any::Any;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll as TaskPoll};

use crate::sync::{Event, EventBus};
use lock::Mutex;
use rcore_fs::vfs::*;
use zircon_object::vm::{pages, VmObject};

use super::drm;

/// Parks until the DRM card fd has a queued event. Flat `Future` (no nested
/// `async` state machine) so blocking card reads / leftover `async_poll`
/// callers stay thin on the coroutine stack.
#[must_use = "future does nothing unless polled/`await`-ed"]
struct DrmEventWait<'a> {
    dev: &'a DrmDev,
    bus: Arc<Mutex<EventBus>>,
    sub_id: Option<u64>,
}

impl Drop for DrmEventWait<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.sub_id.take() {
            self.bus.lock().unsubscribe(id);
        }
    }
}

impl Future for DrmEventWait<'_> {
    type Output = Result<PollStatus>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> TaskPoll<Self::Output> {
        let this = self.as_mut().get_mut();
        match this.dev.poll() {
            Ok(status) if status.read => {
                if let Some(id) = this.sub_id.take() {
                    this.bus.lock().unsubscribe(id);
                }
                return TaskPoll::Ready(Ok(status));
            }
            Ok(_) => {}
            Err(e) => {
                if let Some(id) = this.sub_id.take() {
                    this.bus.lock().unsubscribe(id);
                }
                return TaskPoll::Ready(Err(e));
            }
        }
        if this.sub_id.is_none() {
            let waker = cx.waker().clone();
            this.sub_id = this.bus.lock().subscribe(Box::new(move |ev| {
                if (ev & Event::READABLE).is_empty() {
                    return false;
                }
                waker.wake_by_ref();
                true
            }));
        }
        match this.dev.poll() {
            Ok(status) if status.read => {
                if let Some(id) = this.sub_id.take() {
                    this.bus.lock().unsubscribe(id);
                }
                TaskPoll::Ready(Ok(status))
            }
            Ok(_) => TaskPoll::Pending,
            Err(e) => {
                if let Some(id) = this.sub_id.take() {
                    this.bus.lock().unsubscribe(id);
                }
                TaskPoll::Ready(Err(e))
            }
        }
    }
}

/// DRM Device INode
pub struct DrmDev {
    inode_id: usize,
    minor: u32,
}

impl DrmDev {
    pub fn new(minor: u32) -> Self {
        use rcore_fs_devfs::DevFS;
        Self {
            inode_id: DevFS::new_inode_id(),
            minor,
        }
    }

    /// Returns the [`VmObject`] representing the file with given `offset` and `len`.
    pub fn get_vmo(&self, offset: usize, len: usize) -> Result<Arc<VmObject>> {
        // MAP_DUMB handed userspace a page-aligned fake mmap offset that encodes
        // the GEM handle in its upper bits (`handle << PAGE_SHIFT`). musl's
        // `mmap()` rejects a non-page-aligned offset with EINVAL *before* the
        // syscall, so the cookie must be page-aligned; recover the handle by
        // shifting it back down.
        let handle_id = (offset >> 12) as u32;
        if let Some(handle) = drm::get_handle(handle_id) {
            let len = len.min(handle.size);
            Ok(VmObject::new_physical(
                handle.phys_addr as usize,
                pages(len),
            ))
        } else if let Some((phys_addr, size)) = zcore_drivers::scheme::gem_mmap::lookup(handle_id) {
            // Driver-private GEM object (currently: nouveau-uAPI GEM_NEW) --
            // same fake-offset space, different table (see
            // drivers/src/scheme/gem_mmap.rs's module doc for why this
            // driver-owned state can't live in `drm::get_handle`'s table).
            let len = (len as u64).min(size) as usize;
            Ok(VmObject::new_physical(phys_addr as usize, pages(len)))
        } else {
            Err(FsError::InvalidParam)
        }
    }
}

// DRM IOCTL numbers (Linux x86_64)
const DRM_IOCTL_VERSION: u32 = 0xC0406400;
const DRM_IOCTL_GET_UNIQUE: u32 = 0xC0106401;
const DRM_IOCTL_GET_MAGIC: u32 = 0xC0046402;
const DRM_IOCTL_AUTH_MAGIC: u32 = 0x40046411;
const DRM_IOCTL_GET_CAP: u32 = 0xC010640C;
const DRM_IOCTL_SET_CLIENT_CAP: u32 = 0x4010640D;
const DRM_IOCTL_GEM_CLOSE: u32 = 0x40086409;
const DRM_IOCTL_SET_MASTER: u32 = 0x0000641E;
const DRM_IOCTL_DROP_MASTER: u32 = 0x0000641F;

const DRM_IOCTL_MODE_GETRESOURCES: u32 = 0xC04064A0;
const DRM_IOCTL_MODE_GETCRTC: u32 = 0xC06864A1;
const DRM_IOCTL_MODE_SETCRTC: u32 = 0xC06864A2;
const DRM_IOCTL_MODE_GETENCODER: u32 = 0xC01464A6;
const DRM_IOCTL_MODE_GETCONNECTOR: u32 = 0xC05064A7;

const DRM_IOCTL_MODE_CREATE_DUMB: u32 = 0xC02064B2;
const DRM_IOCTL_MODE_MAP_DUMB: u32 = 0xC01064B3;
const DRM_IOCTL_MODE_DESTROY_DUMB: u32 = 0xC00464B4;
const DRM_IOCTL_MODE_ADDFB: u32 = 0xC01C64AE;
const DRM_IOCTL_MODE_ADDFB2: u32 = 0xC06864B8;
const DRM_IOCTL_MODE_RMFB: u32 = 0xC00464AF;
/// `struct drm_mode_closefb { u32 fb_id; u32 pad; }` — Linux 6.6+. wlroots
/// prefers it over RMFB when tearing down framebuffers (CLOSEFB drops the
/// caller's reference WITHOUT disabling the plane/CRTC it may still be on);
/// with it unhandled every fb teardown logged "Failed to close FB" and fell
/// back to RMFB.
const DRM_IOCTL_MODE_CLOSEFB: u32 = 0xC00864D0;
const DRM_IOCTL_MODE_PAGE_FLIP: u32 = 0xC01864B0;

const DRM_IOCTL_MODE_GETPLANERESOURCES: u32 = 0xC01064B5;
const DRM_IOCTL_MODE_GETPLANE: u32 = 0xC02064B6;
const DRM_IOCTL_MODE_SETPLANE: u32 = 0xC03064B7;
const DRM_IOCTL_MODE_OBJ_GETPROPERTIES: u32 = 0xC02064B9;
const DRM_IOCTL_MODE_OBJ_SETPROPERTY: u32 = 0xC01864BA;
const DRM_IOCTL_MODE_GETPROPERTY: u32 = 0xC04064AA;
const DRM_IOCTL_MODE_GETPROPBLOB: u32 = 0xC01064AC;
// Legacy connector property setter (`drmModeConnectorSetProperty`), used by
// wlroots' legacy DRM path to drive the connector DPMS state to "on" during a
// modeset commit. `struct drm_mode_connector_set_property { __u64 value; __u32
// prop_id; __u32 connector_id; }` (16 bytes).
const DRM_IOCTL_MODE_SETPROPERTY: u32 = 0xC01064AB;

// Legacy cursor ioctls (`drmModeSetCursor`/`drmModeMoveCursor`/`...2`). On the
// software-KMS / pixman path there is no hardware cursor plane, so wlroots is
// told to use a software cursor (WLR_NO_HARDWARE_CURSORS=1) and normally never
// issues these. But if that env var is missing, wlroots' legacy backend calls
// drmModeSetCursor during a commit; returning an error (ENOTTY) failed the
// whole frame commit ("Failed to commit frame") and left the screen black.
// Accept them as no-ops so rendering proceeds regardless (the pointer is then
// only visible when the software-cursor path is used).
const DRM_IOCTL_MODE_CURSOR: u32 = 0xC01C64A3;
const DRM_IOCTL_MODE_CURSOR2: u32 = 0xC02464BB;

// Core (non-MODE) vblank wait.
const DRM_IOCTL_WAIT_VBLANK: u32 = 0xC018643A;
// Query an existing framebuffer object.
const DRM_IOCTL_MODE_GETFB: u32 = 0xC01C64AD;
const DRM_IOCTL_MODE_GETFB2: u32 = 0xC06864CE;
// Flush framebuffer damage to the display.
const DRM_IOCTL_MODE_DIRTYFB: u32 = 0xC01864B1;
// Legacy gamma LUT get/set (`struct drm_mode_crtc_lut`, 32 bytes). The Xorg
// modesetting driver reads the CRTC's gamma at startup (to restore on exit) and
// sets an identity ramp during modeset; ENOTTY here made it log an error and
// spin re-issuing it (the SETGAMMA flood on real hardware). The software scanout
// has no gamma hardware, so accept both as no-ops.
const DRM_IOCTL_MODE_GETGAMMA: u32 = 0xC02064A4;
const DRM_IOCTL_MODE_SETGAMMA: u32 = 0xC02064A5;
// Lease enumeration (`struct drm_mode_list_lessees`, 16 bytes). Xorg probes it
// while taking DRM master; there are never any leases here, so report zero.
const DRM_IOCTL_MODE_LIST_LESSEES: u32 = 0xC01064C7;
// Interface-version handshake (`drmSetInterfaceVersion`). The Xorg
// modesetting driver issues it right after open; ENOTTY fails its probe.
const DRM_IOCTL_SET_VERSION: u32 = 0xC0106407;

// Atomic modesetting (`drm-uapi.rst` "Atomic Mode Setting"): one-shot
// multi-object property commit, plus the property-blob objects it rides on
// (`MODE_ID` blobs are created/destroyed by the client per modeset).
const DRM_IOCTL_MODE_ATOMIC: u32 = 0xC03864BC;
const DRM_IOCTL_MODE_CREATEPROPBLOB: u32 = 0xC01064BD;
const DRM_IOCTL_MODE_DESTROYPROPBLOB: u32 = 0xC00464BE;

// DRM sync objects (`drm.h`): core, driver-independent -- their nr range
// (0xBF-0xCF) sits ABOVE `DRM_COMMAND_END` (0xA0), unlike driver-private
// ioctls, so unlike e.g. the nouveau-uAPI numbers these are never offset by
// `DRM_COMMAND_BASE`. See `zcore_drivers::scheme::syncobj` for the actual
// state (lives in `drivers` so a driver's own submission path, e.g.
// `NvidiaGpu`'s nouveau-uAPI `EXEC`, can signal one directly).
const fn drm_iowr_core(nr: u32, size: usize) -> u32 {
    (3u32 << 30) | (0x64u32 << 8) | (nr & 0xff) | (((size as u32) & 0x3fff) << 16)
}
const DRM_IOCTL_SYNCOBJ_CREATE: u32 = drm_iowr_core(0xBF, core::mem::size_of::<DrmSyncobjCreate>());
const DRM_IOCTL_SYNCOBJ_DESTROY: u32 = drm_iowr_core(0xC0, core::mem::size_of::<DrmSyncobjDestroy>());
// 0xC1/0xC2 (HANDLE_TO_FD/FD_TO_HANDLE): NOT dispatched here -- like
// PRIME_HANDLE_TO_FD/FD_TO_HANDLE above, they need process fd table access
// this inode-level `io_control` doesn't have, so `linux-syscall`'s
// `sys_ioctl` intercepts them before they ever reach this match (see
// `sys_drm_syncobj_fd` there, and `linux_object::fs::SyncobjHandle`'s
// module doc for what "export" means given the syncobj table is a single
// global handle space, not per-process).
const DRM_IOCTL_SYNCOBJ_WAIT: u32 = drm_iowr_core(0xC3, core::mem::size_of::<DrmSyncobjWait>());
const DRM_IOCTL_SYNCOBJ_RESET: u32 = drm_iowr_core(0xC4, core::mem::size_of::<DrmSyncobjArray>());
const DRM_IOCTL_SYNCOBJ_SIGNAL: u32 = drm_iowr_core(0xC5, core::mem::size_of::<DrmSyncobjArray>());
const DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT: u32 =
    drm_iowr_core(0xCA, core::mem::size_of::<DrmSyncobjTimelineWait>());
const DRM_IOCTL_SYNCOBJ_QUERY: u32 =
    drm_iowr_core(0xCB, core::mem::size_of::<DrmSyncobjTimelineArray>());
// 0xCC (TRANSFER) not implemented: copying a point between two timelines is
// an edge case real clients rarely hit.
const DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL: u32 =
    drm_iowr_core(0xCD, core::mem::size_of::<DrmSyncobjTimelineArray>());
// 0xCF (EVENTFD) not implemented: needs real eventfd/interrupt plumbing.

#[repr(C)]
struct DrmSyncobjCreate {
    handle: u32,
    flags: u32,
}
const DRM_SYNCOBJ_CREATE_SIGNALED: u32 = 1 << 0;

#[repr(C)]
struct DrmSyncobjDestroy {
    handle: u32,
    #[allow(dead_code)]
    pad: u32,
}

#[repr(C)]
struct DrmSyncobjWait {
    handles: u64,
    timeout_nsec: i64,
    count_handles: u32,
    flags: u32,
    first_signaled: u32,
    #[allow(dead_code)]
    pad: u32,
    #[allow(dead_code)]
    deadline_nsec: u64,
}

#[repr(C)]
struct DrmSyncobjTimelineWait {
    handles: u64,
    points: u64,
    timeout_nsec: i64,
    count_handles: u32,
    flags: u32,
    first_signaled: u32,
    #[allow(dead_code)]
    pad: u32,
    #[allow(dead_code)]
    deadline_nsec: u64,
}
const DRM_SYNCOBJ_WAIT_FLAGS_WAIT_ALL: u32 = 1 << 0;

#[repr(C)]
struct DrmSyncobjArray {
    handles: u64,
    count_handles: u32,
    #[allow(dead_code)]
    pad: u32,
}

#[repr(C)]
struct DrmSyncobjTimelineArray {
    handles: u64,
    points: u64,
    count_handles: u32,
    #[allow(dead_code)]
    flags: u32,
}

// WAIT_VBLANK request type flags (`<drm/drm.h>`).
const _DRM_VBLANK_EVENT: u32 = 0x0400_0000;

// Synthetic KMS property ids (software KMS). Linux allocates property object
// ids from the same idr as every other mode object; here they are fixed small
// ints above the synthetic CRTC/connector/encoder/plane ids. The names and
// semantics follow `drm-kms.rst` "Standard Properties": `type` classifies the
// plane, the connector carries `DPMS`/`link-status`/`non-desktop`/`EDID`, and
// the DRM_MODE_PROP_ATOMIC set (FB_ID..MODE_ID) is only shown to clients that
// negotiated DRM_CLIENT_CAP_ATOMIC, exactly like Linux hides atomic props
// from legacy clients.
const PROP_TYPE: u32 = 10;
const PROP_EDID: u32 = 11;
const PROP_DPMS: u32 = 12;
const PROP_LINK_STATUS: u32 = 13;
const PROP_NON_DESKTOP: u32 = 14;
const PROP_FB_ID: u32 = 15;
/// One property object attached to both the plane and the connector, exactly
/// like Linux's single `prop_crtc_id`.
const PROP_CRTC_ID: u32 = 16;
const PROP_CRTC_X: u32 = 17;
const PROP_CRTC_Y: u32 = 18;
const PROP_CRTC_W: u32 = 19;
const PROP_CRTC_H: u32 = 20;
const PROP_SRC_X: u32 = 21;
const PROP_SRC_Y: u32 = 22;
const PROP_SRC_W: u32 = 23;
const PROP_SRC_H: u32 = 24;
const PROP_ACTIVE: u32 = 25;
const PROP_MODE_ID: u32 = 26;

// Property flags (`drm_mode.h`).
const DRM_MODE_PROP_RANGE: u32 = 1 << 1;
const DRM_MODE_PROP_IMMUTABLE: u32 = 1 << 2;
const DRM_MODE_PROP_ENUM: u32 = 1 << 3;
const DRM_MODE_PROP_BLOB: u32 = 1 << 4;
const DRM_MODE_PROP_OBJECT: u32 = 1 << 6; // DRM_MODE_PROP_TYPE(1)
const DRM_MODE_PROP_SIGNED_RANGE: u32 = 2 << 6; // DRM_MODE_PROP_TYPE(2)
const DRM_MODE_PROP_ATOMIC: u32 = 0x8000_0000;

// KMS object types (`drm_mode.h`).
const DRM_MODE_OBJECT_CRTC: u32 = 0xcccc_cccc;
const DRM_MODE_OBJECT_FB: u32 = 0xfbfb_fbfb;

// DRM client capabilities (DRM_IOCTL_SET_CLIENT_CAP).
const DRM_CLIENT_CAP_ATOMIC: u64 = 3;
const DRM_CLIENT_CAP_WRITEBACK_CONNECTORS: u64 = 5;

// drm_mode_atomic flags (`drm_mode.h`).
const DRM_MODE_PAGE_FLIP_EVENT: u32 = 0x01;
const DRM_MODE_PAGE_FLIP_ASYNC: u32 = 0x02;
const DRM_MODE_ATOMIC_TEST_ONLY: u32 = 0x0100;
const DRM_MODE_ATOMIC_NONBLOCK: u32 = 0x0200;
const DRM_MODE_ATOMIC_ALLOW_MODESET: u32 = 0x0400;
const DRM_MODE_ATOMIC_FLAGS: u32 = DRM_MODE_PAGE_FLIP_EVENT
    | DRM_MODE_PAGE_FLIP_ASYNC
    | DRM_MODE_ATOMIC_TEST_ONLY
    | DRM_MODE_ATOMIC_NONBLOCK
    | DRM_MODE_ATOMIC_ALLOW_MODESET;

/// Whether a DRM ioctl is flagged `DRM_RENDER_ALLOW` in Linux's
/// `drm_ioctl.c` — the only commands a render node (`renderD128`, minor >=
/// 128) accepts. Everything else (modeset, dumb buffers, master/auth) gets
/// EACCES there, per `drm-uapi.rst` "Render nodes": *"no modesetting or
/// privileged ioctls can be issued on render nodes"*.
fn render_allowed(cmd: u32) -> bool {
    let nr = (cmd >> 8) & 0xff;
    matches!(nr,
        0x00        // VERSION
        | 0x09      // GEM_CLOSE
        | 0x0C      // GET_CAP
        | 0x2D      // PRIME_FD_TO_HANDLE (handled in the syscall layer)
        | 0x2E      // PRIME_HANDLE_TO_FD (handled in the syscall layer)
        // Driver-specific command range (DRM_COMMAND_BASE..DRM_COMMAND_END).
        // Linux delegates per-command flags to the driver's own ioctl table;
        // our DrmScheme has no flags concept, so the range is passed through
        // and the driver decides (render/exec ioctls are RENDER_ALLOW in
        // practice).
        | 0x40..=0x9F
        | 0xBF..=0xC5 // SYNCOBJ_CREATE..SYNCOBJ_SIGNAL
        | 0xCA..=0xCD // SYNCOBJ_TIMELINE_WAIT..TIMELINE_SIGNAL
        | 0xCF      // SYNCOBJ_EVENTFD
        | 0xD1      // SET_CLIENT_NAME
        | 0xD2      // GEM_CHANGE_HANDLE
    )
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmVersion {
    version_major: i32,
    version_minor: i32,
    version_patchlevel: i32,
    name_len: usize,
    name: *mut u8,
    date_len: usize,
    date: *mut u8,
    desc_len: usize,
    desc: *mut u8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmUnique {
    unique_len: usize,
    unique: *mut u8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmGetCap {
    capability: u64,
    value: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeCardRes {
    fb_id_ptr: u64,
    crtc_id_ptr: u64,
    connector_id_ptr: u64,
    encoder_id_ptr: u64,
    count_fbs: u32,
    count_crtcs: u32,
    count_connectors: u32,
    count_encoders: u32,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeCreateDumb {
    height: u32,
    width: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeFbCmd {
    fb_id: u32,
    width: u32,
    height: u32,
    pitch: u32,
    bpp: u32,
    depth: u32,
    handle: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeMapDumb {
    handle: u32,
    pad: u32,
    offset: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeGetConnector {
    encoders_ptr: u64,
    modes_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    count_modes: u32,
    count_props: u32,
    count_encoders: u32,
    encoder_id: u32, // current encoder
    connector_id: u32,
    connector_type: u32,
    connector_type_id: u32,
    connection: u32,
    mm_width: u32,
    mm_height: u32,
    subpixel: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeGetEncoder {
    encoder_id: u32,
    encoder_type: u32,
    crtc_id: u32,
    possible_crtcs: u32,
    possible_clones: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeFbCmd2 {
    fb_id: u32,
    width: u32,
    height: u32,
    pixel_format: u32,
    flags: u32,
    handles: [u32; 4],
    pitches: [u32; 4],
    offsets: [u32; 4],
    modifier: [u64; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeGetCrtc {
    set_connectors_ptr: u64,
    count_connectors: u32,
    crtc_id: u32,
    fb_id: u32,
    x: u32,
    y: u32,
    gamma_size: u32,
    mode_valid: u32,
    mode: [u8; 68],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeGetPlaneRes {
    plane_id_ptr: u64,
    count_planes: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeGetPlane {
    plane_id: u32,
    crtc_id: u32,
    fb_id: u32,
    possible_crtcs: u32,
    gamma_size: u32,
    count_format_types: u32,
    format_type_ptr: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeObjGetProperties {
    props_ptr: u64,
    prop_values_ptr: u64,
    count_props: u32,
    obj_id: u32,
    obj_type: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeGetProperty {
    values_ptr: u64,
    enum_blob_ptr: u64,
    prop_id: u32,
    flags: u32,
    name: [u8; 32],
    count_values: u32,
    count_enum_blobs: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeGetBlob {
    blob_id: u32,
    length: u32,
    data: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModePropertyEnum {
    value: u64,
    name: [u8; 32],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeCrtcPageFlip {
    crtc_id: u32,
    fb_id: u32,
    flags: u32,
    reserved: u32,
    user_data: u64,
}

/// `union drm_wait_vblank` (24 bytes). The request side is `{ type, sequence,
/// signal }`; the reply side reuses the trailing 16 bytes as `{ tval_sec,
/// tval_usec }`. We model the union as one struct and read/write the overlap by
/// field.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmWaitVblank {
    typ: u32,
    sequence: u32,
    /// request: `signal`; reply: `tval_sec`.
    val1: u64,
    /// request: unused; reply: `tval_usec`.
    val2: u64,
}

/// `struct drm_mode_set_plane` (48 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeSetPlane {
    plane_id: u32,
    crtc_id: u32,
    fb_id: u32,
    flags: u32,
    crtc_x: i32,
    crtc_y: i32,
    crtc_w: u32,
    crtc_h: u32,
    // Source values are 16.16 fixed point.
    src_x: u32,
    src_y: u32,
    src_h: u32,
    src_w: u32,
}

/// `struct drm_mode_obj_set_property` (24 bytes after u64 alignment padding).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeObjSetProperty {
    value: u64,
    prop_id: u32,
    obj_id: u32,
    obj_type: u32,
}

/// `struct drm_mode_fb_dirty_cmd` (24 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeFbDirtyCmd {
    fb_id: u32,
    flags: u32,
    color: u32,
    num_clips: u32,
    clips_ptr: u64,
}

/// `struct drm_clip_rect` (8 bytes) — one element of the array `clips_ptr`
/// points to. `x2`/`y2` are exclusive, i.e. the rect covers `[x1, x2) x [y1, y2)`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmClipRect {
    x1: u16,
    y1: u16,
    x2: u16,
    y2: u16,
}

/// `struct drm_set_version` (16 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmSetVersion {
    drm_di_major: i32,
    drm_di_minor: i32,
    drm_dd_major: i32,
    drm_dd_minor: i32,
}

/// `struct drm_mode_atomic` (56 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeAtomic {
    flags: u32,
    count_objs: u32,
    objs_ptr: u64,
    count_props_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    reserved: u64,
    user_data: u64,
}

/// `struct drm_mode_create_blob` (16 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DrmModeCreateBlob {
    data: u64,
    length: u32,
    blob_id: u32,
}

// Compile-time guards: each DRM ioctl number encodes `sizeof(struct)` in its
// _IOC size field, so a wrong struct layout silently mismatches the ioctl and
// the handler never fires. Assert the sizes that the constants above depend on.
const _: () = {
    use core::mem::size_of;
    assert!(size_of::<DrmModeGetConnector>() == 80); // DRM_IOCTL_MODE_GETCONNECTOR 0x..50..
    assert!(size_of::<DrmModeGetEncoder>() == 20); // DRM_IOCTL_MODE_GETENCODER  0x..14..
    assert!(size_of::<DrmModeFbCmd2>() == 104); // DRM_IOCTL_MODE_ADDFB2      0x..68..
    assert!(size_of::<DrmModeGetCrtc>() == 104); // DRM_IOCTL_MODE_{GET,SET}CRTC 0x..68..
    assert!(size_of::<DrmModeCrtcPageFlip>() == 24); // DRM_IOCTL_MODE_PAGE_FLIP 0x..18..
    assert!(size_of::<DrmModeObjGetProperties>() == 32); // OBJ_GETPROPERTIES 0x..20..
    assert!(size_of::<DrmModeGetProperty>() == 64); // GETPROPERTY        0x..40..
    assert!(size_of::<DrmModeGetBlob>() == 16); // GETPROPBLOB        0x..10..
    assert!(size_of::<DrmModePropertyEnum>() == 40);
    assert!(size_of::<DrmModeGetPlane>() == 32); // DRM_IOCTL_MODE_GETPLANE 0x..20..
    assert!(size_of::<DrmWaitVblank>() == 24); // DRM_IOCTL_WAIT_VBLANK   0x..18..
    assert!(size_of::<DrmModeSetPlane>() == 48); // DRM_IOCTL_MODE_SETPLANE 0x..30..
    assert!(size_of::<DrmModeObjSetProperty>() == 24); // OBJ_SETPROPERTY  0x..18..
    assert!(size_of::<DrmModeFbDirtyCmd>() == 24); // DRM_IOCTL_MODE_DIRTYFB 0x..18..
    assert!(size_of::<DrmClipRect>() == 8); // drm_clip_rect, via DIRTYFB's clips_ptr
    assert!(size_of::<DrmSetVersion>() == 16); // DRM_IOCTL_SET_VERSION   0x..10..
    assert!(size_of::<DrmModeAtomic>() == 56); // DRM_IOCTL_MODE_ATOMIC   0x..38..
    assert!(size_of::<DrmModeCreateBlob>() == 16); // CREATEPROPBLOB      0x..10..
};

/// Build a `struct drm_mode_modeinfo` (68 bytes) for a simple 60 Hz mode at
/// `w`x`h`. Timings are nominal — a software framebuffer never programs real CRT
/// timings — but wlroots needs a populated, "preferred" mode to drive output.
fn make_modeinfo(w: u32, h: u32) -> [u8; 68] {
    let mut m = [0u8; 68];
    let refresh: u32 = 60;
    // Standard blanking: add 10 % horizontal and 5 % vertical blanking so the
    // pixel clock matches what wlroots expects (clock ≈ htotal * vtotal * refresh
    // / 1000 kHz).  Using only the active area gives an unusably low clock and
    // a calculated refresh of 59.xxx Hz rather than the 59.999 wlroots logs.
    let htotal = (w * 11 / 10) as u16; // +10 % H blanking
    let vtotal = (h * 21 / 20) as u16; // +5 % V blanking
    let clock = (htotal as u64 * vtotal as u64 * refresh as u64 / 1000) as u32; // kHz
    m[0..4].copy_from_slice(&clock.to_ne_bytes());
    let wh = w as u16;
    m[4..6].copy_from_slice(&wh.to_ne_bytes()); // hdisplay
                                                // hsync_start / hsync_end / htotal: use simple 10 % blanking
    let hsync_start = (w + w / 16) as u16;
    let hsync_end = (w + w / 8) as u16;
    m[6..8].copy_from_slice(&hsync_start.to_ne_bytes());
    m[8..10].copy_from_slice(&hsync_end.to_ne_bytes());
    m[10..12].copy_from_slice(&htotal.to_ne_bytes());
    // hskew @12..14 = 0
    let hh = h as u16;
    m[14..16].copy_from_slice(&hh.to_ne_bytes()); // vdisplay
                                                  // vsync_start / vsync_end / vtotal: use simple 5 % blanking
    let vsync_start = (h + h / 40) as u16;
    let vsync_end = (h + h / 20) as u16;
    m[16..18].copy_from_slice(&vsync_start.to_ne_bytes());
    m[18..20].copy_from_slice(&vsync_end.to_ne_bytes());
    m[20..22].copy_from_slice(&vtotal.to_ne_bytes());
    // vscan @22..24 = 0
    m[24..28].copy_from_slice(&refresh.to_ne_bytes()); // vrefresh
                                                       // flags @28..32 = 0
                                                       // type @32..36: DRM_MODE_TYPE_DRIVER(0x40) | DRM_MODE_TYPE_PREFERRED(0x08)
    m[32..36].copy_from_slice(&0x48u32.to_ne_bytes());
    // name @36..68 ("WxH")
    let mut name = [0u8; 32];
    let mut i = 0;
    let put = |buf: &mut [u8; 32], i: &mut usize, val: u32| {
        if val == 0 {
            if *i < buf.len() {
                buf[*i] = b'0';
                *i += 1;
            }
            return;
        }
        let mut digits = [0u8; 10];
        let mut n = 0;
        let mut v = val;
        while v > 0 {
            digits[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        while n > 0 && *i < buf.len() {
            n -= 1;
            buf[*i] = digits[n];
            *i += 1;
        }
    };
    put(&mut name, &mut i, w);
    if i < name.len() {
        name[i] = b'x';
        i += 1;
    }
    put(&mut name, &mut i, h);
    m[36..68].copy_from_slice(&name);
    m
}

const I32_MIN_U64: u64 = i32::MIN as i64 as u64;
const I32_MAX_U64: u64 = i32::MAX as u64;
const U32_MAX_U64: u64 = u32::MAX as u64;

/// Metadata served by `DRM_IOCTL_MODE_GETPROPERTY` for one property object:
/// flags, name, and the value/enum lists per its type (`drm-kms.rst` "KMS
/// Properties"). Range properties list `[min, max]`; object properties list
/// the object type they accept; enums list `(value, name)` pairs.
struct PropSpec {
    name: &'static str,
    flags: u32,
    values: &'static [u64],
    enums: &'static [(u64, &'static str)],
}

/// The property table of the synthetic pipeline. Names, types and ranges
/// match Linux's standard properties (`drm_mode_create_standard_properties`,
/// `drm_plane_create_*`, `drm_connector_create_standard_properties`).
fn prop_spec(prop_id: u32) -> Option<PropSpec> {
    Some(match prop_id {
        PROP_TYPE => PropSpec {
            name: "type",
            flags: DRM_MODE_PROP_ENUM | DRM_MODE_PROP_IMMUTABLE,
            values: &[0, 1, 2],
            enums: &[(0, "Overlay"), (1, "Primary"), (2, "Cursor")],
        },
        PROP_EDID => PropSpec {
            name: "EDID",
            flags: DRM_MODE_PROP_BLOB | DRM_MODE_PROP_IMMUTABLE,
            values: &[],
            enums: &[],
        },
        PROP_DPMS => PropSpec {
            name: "DPMS",
            flags: DRM_MODE_PROP_ENUM,
            values: &[0, 1, 2, 3],
            enums: &[(0, "On"), (1, "Standby"), (2, "Suspend"), (3, "Off")],
        },
        PROP_LINK_STATUS => PropSpec {
            name: "link-status",
            flags: DRM_MODE_PROP_ENUM,
            values: &[0, 1],
            enums: &[(0, "Good"), (1, "Bad")],
        },
        PROP_NON_DESKTOP => PropSpec {
            name: "non-desktop",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_IMMUTABLE,
            values: &[0, 1],
            enums: &[],
        },
        PROP_FB_ID => PropSpec {
            name: "FB_ID",
            flags: DRM_MODE_PROP_OBJECT | DRM_MODE_PROP_ATOMIC,
            values: &[DRM_MODE_OBJECT_FB as u64],
            enums: &[],
        },
        PROP_CRTC_ID => PropSpec {
            name: "CRTC_ID",
            flags: DRM_MODE_PROP_OBJECT | DRM_MODE_PROP_ATOMIC,
            values: &[DRM_MODE_OBJECT_CRTC as u64],
            enums: &[],
        },
        PROP_CRTC_X => PropSpec {
            name: "CRTC_X",
            flags: DRM_MODE_PROP_SIGNED_RANGE | DRM_MODE_PROP_ATOMIC,
            values: &[I32_MIN_U64, I32_MAX_U64],
            enums: &[],
        },
        PROP_CRTC_Y => PropSpec {
            name: "CRTC_Y",
            flags: DRM_MODE_PROP_SIGNED_RANGE | DRM_MODE_PROP_ATOMIC,
            values: &[I32_MIN_U64, I32_MAX_U64],
            enums: &[],
        },
        PROP_CRTC_W => PropSpec {
            name: "CRTC_W",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            values: &[0, I32_MAX_U64],
            enums: &[],
        },
        PROP_CRTC_H => PropSpec {
            name: "CRTC_H",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            values: &[0, I32_MAX_U64],
            enums: &[],
        },
        PROP_SRC_X => PropSpec {
            name: "SRC_X",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            values: &[0, U32_MAX_U64],
            enums: &[],
        },
        PROP_SRC_Y => PropSpec {
            name: "SRC_Y",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            values: &[0, U32_MAX_U64],
            enums: &[],
        },
        PROP_SRC_W => PropSpec {
            name: "SRC_W",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            values: &[0, U32_MAX_U64],
            enums: &[],
        },
        PROP_SRC_H => PropSpec {
            name: "SRC_H",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            values: &[0, U32_MAX_U64],
            enums: &[],
        },
        PROP_ACTIVE => PropSpec {
            name: "ACTIVE",
            flags: DRM_MODE_PROP_RANGE | DRM_MODE_PROP_ATOMIC,
            values: &[0, 1],
            enums: &[],
        },
        PROP_MODE_ID => PropSpec {
            name: "MODE_ID",
            flags: DRM_MODE_PROP_BLOB | DRM_MODE_PROP_ATOMIC,
            values: &[],
            enums: &[],
        },
        _ => return None,
    })
}

/// `(prop_id, value)` pairs attached to the synthetic connector. Atomic
/// properties (CRTC_ID) are only listed for clients that negotiated
/// `DRM_CLIENT_CAP_ATOMIC`, mirroring Linux's atomic-property filtering.
fn connector_props(connector_id: u32) -> alloc::vec::Vec<(u32, u64)> {
    let mut props = alloc::vec::Vec::new();
    // The software scanout is always lit: DPMS "On", link "Good", a desktop
    // display.
    props.push((PROP_DPMS, 0));
    props.push((PROP_LINK_STATUS, 0));
    props.push((PROP_NON_DESKTOP, 0));
    if drm::get_connector_edid(connector_id).is_some() {
        props.push((PROP_EDID, (20000 + connector_id) as u64));
    }
    if drm::atomic_client() {
        let (st, _) = drm::atomic_snapshot();
        let crtc = if st.active { drm::SYNTH_CRTC_ID } else { 0 };
        props.push((PROP_CRTC_ID, crtc as u64));
    }
    props
}

/// `(prop_id, value)` pairs attached to the synthetic CRTC (atomic-only).
fn crtc_props() -> alloc::vec::Vec<(u32, u64)> {
    let mut props = alloc::vec::Vec::new();
    if drm::atomic_client() {
        let (st, _) = drm::atomic_snapshot();
        props.push((PROP_ACTIVE, st.active as u64));
        props.push((PROP_MODE_ID, st.mode_blob_id as u64));
    }
    props
}

/// `(prop_id, value)` pairs attached to a plane: `type` for everyone, plus
/// the atomic plane state for atomic clients.
fn plane_props(plane: &drm::DrmPlane) -> alloc::vec::Vec<(u32, u64)> {
    let mut props = alloc::vec::Vec::new();
    props.push((PROP_TYPE, plane.plane_type as u64));
    if drm::atomic_client() {
        let (st, crtc_fb) = drm::atomic_snapshot();
        let crtc = if crtc_fb != 0 { plane.crtc_id } else { 0 };
        props.push((PROP_FB_ID, crtc_fb as u64));
        props.push((PROP_CRTC_ID, crtc as u64));
        props.push((PROP_CRTC_X, st.crtc_x as i64 as u64));
        props.push((PROP_CRTC_Y, st.crtc_y as i64 as u64));
        props.push((PROP_CRTC_W, st.crtc_w as u64));
        props.push((PROP_CRTC_H, st.crtc_h as u64));
        props.push((PROP_SRC_X, st.src_x as u64));
        props.push((PROP_SRC_Y, st.src_y as u64));
        props.push((PROP_SRC_W, st.src_w as u64));
        props.push((PROP_SRC_H, st.src_h as u64));
    }
    props
}

/// Stage one `(object, property, value)` triple of a `DRM_IOCTL_MODE_ATOMIC`
/// request into the software-KMS update, with Linux's error contract: an
/// unknown object or a property the object doesn't have is ENOENT; an illegal
/// value or a legacy/immutable property in an atomic commit is EINVAL.
fn atomic_stage(upd: &mut drm::AtomicUpdate, obj_id: u32, prop_id: u32, value: u64) -> Result<()> {
    if drm::get_plane(obj_id).is_some() {
        match prop_id {
            PROP_FB_ID => upd.plane_fb_id = Some(value as u32),
            PROP_CRTC_ID => upd.plane_crtc_id = Some(value as u32),
            PROP_CRTC_X => upd.crtc_x = Some(value as i32),
            PROP_CRTC_Y => upd.crtc_y = Some(value as i32),
            PROP_CRTC_W => upd.crtc_w = Some(value as u32),
            PROP_CRTC_H => upd.crtc_h = Some(value as u32),
            PROP_SRC_X => upd.src_x = Some(value as u32),
            PROP_SRC_Y => upd.src_y = Some(value as u32),
            PROP_SRC_W => upd.src_w = Some(value as u32),
            PROP_SRC_H => upd.src_h = Some(value as u32),
            // "type" is immutable.
            PROP_TYPE => return Err(FsError::InvalidParam),
            _ => return Err(FsError::EntryNotFound),
        }
    } else if drm::get_crtc(obj_id).is_some() {
        match prop_id {
            PROP_ACTIVE => {
                if value > 1 {
                    return Err(FsError::InvalidParam);
                }
                upd.active = Some(value != 0);
            }
            PROP_MODE_ID => upd.mode_blob = Some(value as u32),
            _ => return Err(FsError::EntryNotFound),
        }
    } else if drm::get_connector(obj_id).is_some() {
        match prop_id {
            PROP_CRTC_ID => upd.connector_crtc_id = Some(value as u32),
            // DPMS is legacy-only; Linux refuses it inside atomic commits.
            PROP_DPMS => return Err(FsError::InvalidParam),
            _ => return Err(FsError::EntryNotFound),
        }
    } else {
        return Err(FsError::EntryNotFound);
    }
    Ok(())
}

impl INode for DrmDev {
    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> Result<usize> {
        // Deliver queued DRM events (page-flip completions). When none are
        // pending report `Again` so a non-blocking reader gets EAGAIN and an
        // epoll/poll waiter re-checks on the next tick.
        match drm::read_event(buf) {
            Some(n) => Ok(n),
            None => Err(FsError::Again),
        }
    }

    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Ok(_buf.len())
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: drm::has_events(),
            // Keep write=true for now: reporting write=false made labwc's
            // DRM epoll actually park and exposed a #DF at session start
            // (heap corruption while the card fd stopped looking always-
            // ready). Linux semantics are "readable for events"; revisit
            // once the UserContext/#DF path at labwc bring-up is solid.
            write: true,
            error: false,
        })
    }

    fn async_poll<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<PollStatus>> + Send + Sync + 'a>> {
        // Lightweight waiter: do not nest an `async move { loop { ... } }`
        // state machine. Poll/epoll already use sync `poll()`; this path is
        // for blocking reads and any leftover async_poll callers.
        let bus = drm::get_eventbus();
        Box::pin(DrmEventWait {
            dev: self,
            bus,
            sub_id: None,
        })
    }

    fn metadata(&self) -> Result<Metadata> {
        Ok(Metadata {
            dev: 1,
            inode: self.inode_id,
            size: 0,
            blk_size: 0,
            blocks: 0,
            atime: Timespec { sec: 0, nsec: 0 },
            mtime: Timespec { sec: 0, nsec: 0 },
            ctime: Timespec { sec: 0, nsec: 0 },
            type_: FileType::CharDevice,
            mode: 0o660,
            nlinks: 1,
            uid: 0,
            gid: 0,
            rdev: make_rdev(0xe2, self.minor as usize), // 226 is DRM major
        })
    }

    #[allow(unsafe_code)]
    fn io_control(&self, cmd: u32, data: usize) -> Result<usize> {
        // Render nodes only accept DRM_RENDER_ALLOW ioctls (drm-uapi.rst
        // "Render nodes"): modeset, dumb-buffer and master/auth commands get
        // EACCES exactly like Linux, so a client probing `renderD128` sees a
        // render node, not a second KMS device.
        if self.minor >= 128 && !render_allowed(cmd) {
            log::debug!(
                "[drm] render node refused non-RENDER_ALLOW ioctl {:#010x} (drm nr={:#04x})",
                cmd,
                (cmd >> 8) & 0xff
            );
            return Err(FsError::NoPermission);
        }
        match cmd {
            DRM_IOCTL_VERSION => {
                let node = if self.minor >= 128 {
                    "renderD128"
                } else {
                    "card0"
                };
                // When the nouveau experiment is on, make this visible at the
                // default `warn` level and name the primary DRM driver: it tells
                // us in one boot whether NVK/Mesa even reached VERSION on this
                // node (discovery/enumeration OK) and whether the driver behind
                // it is the real NvidiaGpu ("Nvidia GPU") or a software fallback
                // (which would route every nouveau ioctl to the wrong driver).
                if zcore_drivers::display::nouveau_uapi_enabled() {
                    // klog_info!, not log::warn!: hardware boots default to
                    // LOG=error. This one line tells us in a quiet boot whether
                    // NVK/Mesa even reached VERSION on the node (discovery OK) and
                    // whether the driver behind it is the real NvidiaGpu.
                    match drm::get_primary_driver() {
                        Some(d) => kernel_hal::klog_info!(
                            "[drm] VERSION on /dev/dri/{} (minor={}) -> name=\"nouveau\"; primary_driver={:?} (client reached VERSION — DRM discovery OK)",
                            node,
                            self.minor,
                            d.name()
                        ),
                        None => kernel_hal::klog_info!(
                            "[drm] VERSION on /dev/dri/{} (minor={}) -> name=\"nouveau\"; primary_driver=<none>",
                            node,
                            self.minor
                        ),
                    }
                } else {
                    log::debug!(
                        "[drm] VERSION — /dev/dri/{} opened by userspace (minor={})",
                        node,
                        self.minor
                    );
                }
                // Mesa selects the userspace DRI driver from this NAME. With the
                // nouveau uAPI enabled (the RTX experiment: `nvidia.nouveau_uapi`,
                // only ever set alongside a real NVIDIA card — QEMU's virtio-gpu
                // path never sets it, so it keeps the "zcore" identity), report
                // "nouveau" so Mesa loads nouveau_dri.so, and version 1.4.0 —
                // the drm_nouveau version that gates the NEW submission uAPI
                // (VM_INIT/VM_BIND/EXEC), which is what this driver implements
                // (NOT the legacy GEM_PUSHBUF path, absent here). If Mesa's GL
                // winsys still reaches for GEM_PUSHBUF, the boot log's
                // "[nouveau-uapi] unhandled ioctl ... GEM_PUSHBUF" line says so.
                let nouveau = zcore_drivers::display::nouveau_uapi_enabled();
                let v = unsafe { &mut *(data as *mut DrmVersion) };
                if nouveau {
                    v.version_major = 1;
                    v.version_minor = 4;
                    v.version_patchlevel = 0;
                } else {
                    v.version_major = 1;
                    v.version_minor = 0;
                    v.version_patchlevel = 0;
                }

                let name: &[u8] = if nouveau { b"nouveau\0" } else { b"zcore\0" };
                let date = b"20260503\0";
                let desc: &[u8] = if nouveau { b"nouveau\0" } else { b"zCore DRM Driver\0" };

                unsafe {
                    if v.name_len > 0 && !v.name.is_null() {
                        let len = core::cmp::min(v.name_len, name.len());
                        core::ptr::copy_nonoverlapping(name.as_ptr(), v.name, len);
                    }
                    if v.date_len > 0 && !v.date.is_null() {
                        let len = core::cmp::min(v.date_len, date.len());
                        core::ptr::copy_nonoverlapping(date.as_ptr(), v.date, len);
                    }
                    if v.desc_len > 0 && !v.desc.is_null() {
                        let len = core::cmp::min(v.desc_len, desc.len());
                        core::ptr::copy_nonoverlapping(desc.as_ptr(), v.desc, len);
                    }
                }
                v.name_len = name.len();
                v.date_len = date.len();
                v.desc_len = desc.len();
                Ok(0)
            }
            DRM_IOCTL_GET_UNIQUE => {
                let u = unsafe { &mut *(data as *mut DrmUnique) };
                let name = b"zcore-gpu\0";
                unsafe {
                    if u.unique_len > 0 && !u.unique.is_null() {
                        let len = core::cmp::min(u.unique_len, name.len());
                        core::ptr::copy_nonoverlapping(name.as_ptr(), u.unique, len);
                    }
                }
                u.unique_len = name.len();
                Ok(0)
            }
            DRM_IOCTL_GET_CAP => {
                let cap = unsafe { &mut *(data as *mut DrmGetCap) };
                match cap.capability {
                    0x1 => cap.value = 1, // DRM_CAP_DUMB_BUFFER
                    // DRM_CAP_DUMB_PREFERRED_DEPTH: XRGB8888 scanout = 24.
                    0x3 => cap.value = 24,
                    // DRM_CAP_DUMB_PREFER_SHADOW: the dumb buffer lives behind
                    // a CPU blit over PCIe — clients should render to a shadow
                    // and copy, exactly what this cap advises.
                    0x4 => cap.value = 1,
                    // DRM_CAP_PRIME: IMPORT|EXPORT. wlroots' check_drm_features
                    // *requires* DRM_PRIME_CAP_IMPORT or the whole DRM backend
                    // fails ("PRIME import not supported") — it is mandatory for
                    // any output (pixman or GL), not just GBM clients. We now
                    // implement PRIME_HANDLE_TO_FD / FD_TO_HANDLE (dma-buf), so
                    // advertise it.
                    0x5 => cap.value = 3,
                    0x6 => cap.value = 1,  // DRM_CAP_TIMESTAMP_MONOTONIC
                    0x8 => cap.value = 64, // DRM_CAP_CURSOR_WIDTH
                    0x9 => cap.value = 64, // DRM_CAP_CURSOR_HEIGHT
                    0x10 => cap.value = 1, // DRM_CAP_ADDFB2_MODIFIERS
                    // DRM_CAP_CRTC_IN_VBLANK_EVENT: our page-flip event carries
                    // the crtc_id, so report support (wlroots requires it).
                    0x12 => cap.value = 1,
                    // DRM_CAP_SYNCOBJ / DRM_CAP_SYNCOBJ_TIMELINE: real
                    // (create/destroy/wait/signal all work, see
                    // zcore_drivers::scheme::syncobj), but gated on the same
                    // opt-in flag as the rest of this session's nouveau-uAPI
                    // work (`nvidia.nouveau_uapi`) so a driver/setup nothing
                    // here has touched (virtio-gpu, or NVIDIA without the
                    // flag) never sees a capability bit flip by default.
                    0x13 | 0x14 => {
                        cap.value = zcore_drivers::display::nouveau_uapi_enabled() as u64
                    }
                    // Everything else — DRM_CAP_ASYNC_PAGE_FLIP (0x7),
                    // DRM_CAP_PAGE_FLIP_TARGET (0x11) and
                    // DRM_CAP_ATOMIC_ASYNC_PAGE_FLIP (0x15) — is honestly 0.
                    _ => cap.value = 0,
                }
                log::debug!(
                    "[drm] GET_CAP minor={} cap={:#x} -> {}",
                    self.minor,
                    cap.capability,
                    cap.value
                );
                log::debug!("[drm] GET_CAP cap={:#x} -> {}", cap.capability, cap.value);
                Ok(0)
            }
            // A single DRM client on the primary node is implicitly master;
            // accept (drop-)master so seatd/wlroots session activation succeeds.
            // Magic/auth: `drmIsMaster()` authenticates magic 0 and treats
            // success as "this fd is DRM master". wlroots' dumb-buffer allocator
            // (pixman path) requires master, so always succeed — the single
            // client on the primary node is implicitly master here.
            DRM_IOCTL_GET_MAGIC => {
                // struct drm_auth { __u32 magic; }
                unsafe { *(data as *mut u32) = 1 };
                Ok(0)
            }
            DRM_IOCTL_AUTH_MAGIC => Ok(0),
            DRM_IOCTL_SET_MASTER => {
                // Become DRM master, but do NOT switch the console to graphics
                // yet: defer that to the first real scanout (`drm::scanout`). If
                // the client stalls before presenting a frame (e.g. its renderer
                // fails to init), the kernel text console stays usable and its
                // logs visible instead of freezing on a black screen.
                log::debug!("[drm] SET_MASTER (minor={})", self.minor);
                Ok(0)
            }
            DRM_IOCTL_DROP_MASTER => {
                log::debug!(
                    "[drm] DROP_MASTER (minor={}) — restoring text console",
                    self.minor
                );
                kernel_hal::console::set_kd_mode(kernel_hal::console::KD_TEXT);
                // Compositor relinquished the display: forget its VT ownership
                // so text consoles are no longer gated off screen/input, and
                // drop its atomic-client negotiation so the next (possibly
                // legacy-only) client starts with a clean property view.
                drm::clear_graphics_owner();
                drm::set_atomic_client(false);
                Ok(0)
            }
            DRM_IOCTL_SET_VERSION => {
                // drm_setversion: report the current interface (1.4) and
                // driver (1.0) versions, validating any requested majors.
                // -1 means "query only". Xorg's modesetting driver calls this
                // right after open and treats ENOTTY as "not a DRM device".
                let sv = unsafe { &mut *(data as *mut DrmSetVersion) };
                let req_if = (sv.drm_di_major, sv.drm_di_minor);
                let req_dd = sv.drm_dd_major;
                sv.drm_di_major = 1;
                sv.drm_di_minor = 4;
                sv.drm_dd_major = 1;
                sv.drm_dd_minor = 0;
                if req_if.0 != -1 && (req_if.0 != 1 || req_if.1 < 0 || req_if.1 > 4) {
                    return Err(FsError::InvalidParam);
                }
                if req_dd != -1 && req_dd != 1 {
                    return Err(FsError::InvalidParam);
                }
                Ok(0)
            }
            DRM_IOCTL_SET_CLIENT_CAP => {
                // struct drm_set_client_cap { __u64 capability; __u64 value; }
                let cap = unsafe { *(data as *const u64) };
                let value = unsafe { *(data.wrapping_add(8) as *const u64) };
                match cap {
                    DRM_CLIENT_CAP_ATOMIC => {
                        // Linux refuses this with EOPNOTSUPP unless the driver
                        // has DRIVER_ATOMIC. Eclipse's atomic path covers the
                        // software-KMS pipeline and — until it has the same
                        // mileage as the proven legacy path — is opt-in via
                        // the `drm.atomic` cmdline flag (nouveau shipped its
                        // atomic support gated the same way).
                        if !drm::atomic_enabled() || !drm::software_kms_active() {
                            log::debug!(
                                "[drm] SET_CLIENT_CAP ATOMIC -> EOPNOTSUPP (legacy KMS; boot with drm.atomic to enable)"
                            );
                            return Err(FsError::OpNotSupported);
                        }
                        // Linux: values 0..2 (2 = relaxed checking); > 2 EINVAL.
                        if value > 2 {
                            return Err(FsError::InvalidParam);
                        }
                        // Setting atomic also implies universal planes.
                        drm::set_atomic_client(value != 0);
                        log::debug!("[drm] SET_CLIENT_CAP ATOMIC={} -> accepted", value);
                        Ok(0)
                    }
                    DRM_CLIENT_CAP_WRITEBACK_CONNECTORS => {
                        // Linux: atomic clients only. There are no writeback
                        // connectors to expose, so accepting is a no-op.
                        if !drm::atomic_client() || value > 1 {
                            log::debug!("[drm] SET_CLIENT_CAP WRITEBACK -> EINVAL");
                            return Err(FsError::InvalidParam);
                        }
                        Ok(0)
                    }
                    // STEREO_3D, UNIVERSAL_PLANES, ASPECT_RATIO: accept.
                    _ => {
                        log::debug!("[drm] SET_CLIENT_CAP cap={} -> accepted", cap);
                        Ok(0)
                    }
                }
            }
            DRM_IOCTL_MODE_CREATE_DUMB => {
                let info = unsafe { &mut *(data as *mut DrmModeCreateDumb) };
                let bpp = info.bpp.max(32);
                // width/height/bpp are userspace-controlled: compute pitch/size
                // in 64-bit. A 32-bit `width*bpp` or `pitch*height` would wrap
                // (e.g. 50000x50000x32) and under-allocate the buffer while
                // echoing a huge size back, becoming an OOB read at scanout.
                // Bound the result to a sane ceiling (64 MiB — a 4K XRGB frame
                // is ~33 MiB) and require pitch to fit the u32 written back.
                const MAX_DUMB_SIZE: u64 = 64 * 1024 * 1024;
                let pitch64 = (info.width as u64 * bpp as u64 / 8 + 63) & !63;
                let size64 = pitch64.saturating_mul(info.height as u64);
                if pitch64 == 0
                    || pitch64 > u32::MAX as u64
                    || size64 == 0
                    || size64 > MAX_DUMB_SIZE
                {
                    log::warn!(
                        "[drm] CREATE_DUMB {}x{} bpp={} -> rejected (pitch={} size={} out of range)",
                        info.width, info.height, bpp, pitch64, size64
                    );
                    return Err(FsError::InvalidParam);
                }
                let pitch = pitch64 as u32;
                let size = size64 as usize;

                if let Some(handle) = drm::alloc_buffer(size) {
                    info.handle = handle.id;
                    info.pitch = pitch;
                    info.size = size as u64;
                    log::debug!(
                        "[drm] CREATE_DUMB {}x{} bpp={} -> handle={} pitch={} size={}",
                        info.width,
                        info.height,
                        bpp,
                        handle.id,
                        pitch,
                        size
                    );
                    Ok(0)
                } else {
                    log::error!(
                        "[drm] CREATE_DUMB {}x{} bpp={} -> alloc failed (size={})",
                        info.width,
                        info.height,
                        bpp,
                        size
                    );
                    Err(FsError::NoDeviceSpace)
                }
            }
            DRM_IOCTL_MODE_ADDFB => {
                let cmd = unsafe { &mut *(data as *mut DrmModeFbCmd) };
                if let Some(fb_id) = drm::create_fb(cmd.handle, cmd.width, cmd.height, cmd.pitch) {
                    cmd.fb_id = fb_id;
                    Ok(0)
                } else {
                    Err(FsError::DeviceError)
                }
            }
            DRM_IOCTL_MODE_ADDFB2 => {
                let cmd = unsafe { &mut *(data as *mut DrmModeFbCmd2) };
                if let Some(fb_id) =
                    drm::create_fb(cmd.handles[0], cmd.width, cmd.height, cmd.pitches[0])
                {
                    cmd.fb_id = fb_id;
                    Ok(0)
                } else {
                    Err(FsError::DeviceError)
                }
            }
            DRM_IOCTL_MODE_RMFB => {
                let fb_id = unsafe { *(data as *const u32) };
                drm::rmfb(fb_id);
                Ok(0)
            }
            DRM_IOCTL_MODE_CLOSEFB => {
                // Our software-KMS `rmfb` already only drops the fb object —
                // scanout keeps showing the last blitted frame until the next
                // present — which is exactly CLOSEFB's "close without
                // disabling" contract. Reject unknown ids like Linux (EINVAL
                // via DeviceError is close enough for wlroots' fallback).
                let fb_id = unsafe { *(data as *const u32) };
                if drm::rmfb(fb_id) {
                    Ok(0)
                } else {
                    Err(FsError::InvalidParam)
                }
            }
            DRM_IOCTL_MODE_MAP_DUMB => {
                let map = unsafe { &mut *(data as *mut DrmModeMapDumb) };
                // Return a page-aligned fake offset (`handle << PAGE_SHIFT`). The
                // subsequent mmap of the dumb buffer passes this back as the file
                // offset; musl's `mmap()` rejects a non-page-aligned offset with
                // EINVAL, and `get_vmo()` shifts it back to the handle id.
                map.offset = (map.handle as u64) << 12;
                Ok(0)
            }
            DRM_IOCTL_MODE_DESTROY_DUMB => {
                let handle = unsafe { *(data as *const u32) };
                drm::gem_close(handle);
                Ok(0)
            }
            DRM_IOCTL_MODE_SETCRTC => {
                // struct drm_mode_crtc has the same layout as DrmModeGetCrtc.
                let req = unsafe { &mut *(data as *mut DrmModeGetCrtc) };
                if req.fb_id != 0 && !drm::present_now(req.fb_id, req.crtc_id) {
                    return Err(FsError::DeviceError);
                }
                Ok(0)
            }
            DRM_IOCTL_MODE_PAGE_FLIP => {
                let flip = unsafe { *(data as *const DrmModeCrtcPageFlip) };
                if drm::page_flip(flip.fb_id, flip.crtc_id, flip.user_data) {
                    Ok(0)
                } else {
                    Err(FsError::DeviceError)
                }
            }
            DRM_IOCTL_WAIT_VBLANK => {
                // union drm_wait_vblank. A software framebuffer has no real
                // vblank, so synthesize the next sequence from the monotonic
                // clock. If the caller asked for an event (`_DRM_VBLANK_EVENT`)
                // deliver a DRM_EVENT_VBLANK on the card fd; otherwise fill the
                // reply and return immediately instead of blocking.
                let req = unsafe { &mut *(data as *mut DrmWaitVblank) };
                let typ = req.typ;
                let signal = req.val1;
                // Only ask the driver for a real vblank when it has hardware
                // KMS support. Without hardware KMS (e.g. the NVIDIA stub
                // registers has_hardware_kms()=false) wait_vblank is
                // implemented as a busy 16.7 ms spin, and calling it on every
                // WAIT_VBLANK ioctl causes severe CPU starvation on a
                // cooperative async runtime — making the system appear frozen.
                if !drm::software_kms_active() {
                    if let Some(driver) = drm::get_primary_driver() {
                        if driver.has_hardware_kms() {
                            let _ = driver.wait_vblank(0);
                        }
                    }
                }
                let seq = drm::vblank_seq_now().wrapping_add(1);
                if typ & _DRM_VBLANK_EVENT != 0 {
                    // Post the event at the next synthetic vblank, not now:
                    // delivering it instantly turns a vblank-paced client loop
                    // into a busy spin (see `schedule_flip_event`).
                    drm::schedule_vblank_event(signal);
                } else {
                    let now = kernel_hal::timer::timer_now();
                    req.typ = 0; // _DRM_VBLANK_ABSOLUTE
                    req.sequence = seq;
                    req.val1 = now.as_secs(); // tval_sec
                    req.val2 = now.subsec_micros() as u64; // tval_usec
                }
                Ok(0)
            }
            DRM_IOCTL_MODE_SETPLANE => {
                // Primary-plane update: present immediately on the target CRTC.
                // fb_id == 0 disables the plane, which we treat as a no-op.
                let req = unsafe { *(data as *const DrmModeSetPlane) };
                if req.fb_id != 0 && !drm::present_now(req.fb_id, req.crtc_id) {
                    return Err(FsError::DeviceError);
                }
                Ok(0)
            }
            DRM_IOCTL_MODE_GETFB => {
                let cmd = unsafe { &mut *(data as *mut DrmModeFbCmd) };
                if let Some(fb) = drm::get_fb(cmd.fb_id) {
                    cmd.width = fb.width;
                    cmd.height = fb.height;
                    cmd.pitch = fb.pitch;
                    cmd.bpp = 32;
                    cmd.depth = 24;
                    // A single client is implicitly DRM master on the primary
                    // node here, so handing back the backing GEM handle is safe.
                    cmd.handle = fb.gem_handle_id;
                    Ok(0)
                } else {
                    Err(FsError::InvalidParam)
                }
            }
            DRM_IOCTL_MODE_GETFB2 => {
                let cmd = unsafe { &mut *(data as *mut DrmModeFbCmd2) };
                if let Some(fb) = drm::get_fb(cmd.fb_id) {
                    cmd.width = fb.width;
                    cmd.height = fb.height;
                    cmd.pixel_format = 0x3432_5258; // DRM_FORMAT_XRGB8888 ("XR24")
                    cmd.flags = 0;
                    cmd.handles = [fb.gem_handle_id, 0, 0, 0];
                    cmd.pitches = [fb.pitch, 0, 0, 0];
                    cmd.offsets = [0; 4];
                    cmd.modifier = [0; 4];
                    Ok(0)
                } else {
                    Err(FsError::InvalidParam)
                }
            }
            DRM_IOCTL_MODE_DIRTYFB => {
                // Flush accumulated damage by re-scanning the framebuffer out.
                // Clients that keep one persistent FB and signal damage with
                // DIRTYFB (X's modesetting shadow, simple toolkits) rely on this
                // to update the screen. When clip rects are given, blit only
                // their bounding union instead of the whole frame -- DIRTYFB is
                // typically small, frequent damage (a blinking cursor, a
                // repainted widget), and re-scanning the full framebuffer for
                // that was a real chunk of the "feels slow" gap versus Linux.
                let cmd = unsafe { *(data as *const DrmModeFbDirtyCmd) };
                // Bounded well above any real client's clip-rect count; an
                // oversized, zero, or unreadable clip list just means "treat
                // the whole frame as dirty" (also true DIRTYFB semantics for
                // num_clips == 0).
                let rect = if cmd.num_clips > 0 && cmd.num_clips <= 64 && cmd.clips_ptr != 0 {
                    let mut union: Option<(u32, u32, u32, u32)> = None;
                    for i in 0..cmd.num_clips as usize {
                        let clip = unsafe { *(cmd.clips_ptr as *const DrmClipRect).add(i) };
                        if clip.x2 <= clip.x1 || clip.y2 <= clip.y1 {
                            continue;
                        }
                        let (x1, y1, x2, y2) =
                            (clip.x1 as u32, clip.y1 as u32, clip.x2 as u32, clip.y2 as u32);
                        union = Some(match union {
                            Some((ux, uy, uw, uh)) => {
                                let nx = ux.min(x1);
                                let ny = uy.min(y1);
                                let fx = (ux + uw).max(x2);
                                let fy = (uy + uh).max(y2);
                                (nx, ny, fx - nx, fy - ny)
                            }
                            None => (x1, y1, x2 - x1, y2 - y1),
                        });
                    }
                    union
                } else {
                    None
                };
                if !drm::present_now_region(cmd.fb_id, 1, rect) {
                    // Best-effort: a damage flush that can't scan out (e.g. the
                    // fb id is unknown to the software path) is not fatal — the
                    // client keeps its shadow and will re-present. Returning EIO
                    // here made Xorg's modesetting shadow abort its frame loop,
                    // so swallow it rather than failing every DirtyFB.
                    log::debug!("[drm] DIRTYFB fb={} not presented (no-op)", cmd.fb_id);
                }
                Ok(0)
            }
            DRM_IOCTL_MODE_GETGAMMA | DRM_IOCTL_MODE_SETGAMMA => {
                // No programmable gamma on the software scanout: accept and
                // ignore. (Get leaves the caller's ramp buffers untouched, which
                // Xorg treats as the identity it will "restore" on exit — a
                // no-op against our no-op Set.)
                Ok(0)
            }
            DRM_IOCTL_MODE_LIST_LESSEES => {
                // No leases exist; report an empty list. The struct's first u32
                // is `count_lessees` — zero it so the client copies out none.
                unsafe {
                    *(data as *mut u32) = 0;
                }
                Ok(0)
            }
            DRM_IOCTL_MODE_OBJ_SETPROPERTY => {
                // Legacy property writes (connector DPMS, plane rotation, …).
                // The software scanout has no programmable object state, so
                // accept and ignore rather than failing the client's modeset.
                let req = unsafe { *(data as *const DrmModeObjSetProperty) };
                log::debug!(
                    "[drm] OBJ_SETPROPERTY obj={} type={:#x} prop={} val={} (accepted, no-op)",
                    req.obj_id,
                    req.obj_type,
                    req.prop_id,
                    req.value
                );
                Ok(0)
            }
            DRM_IOCTL_MODE_SETPROPERTY => {
                // Legacy connector property write — wlroots sets the DPMS
                // property to "on" as part of committing a modeset. Software
                // scanout is always powered, so accept and ignore rather than
                // failing the commit (which left the screen blank with
                // "Failed to set DPMS property").
                let value = unsafe { *(data as *const u64) };
                let (prop_id, connector_id) = unsafe {
                    (
                        *(data.wrapping_add(8) as *const u32),
                        *(data.wrapping_add(12) as *const u32),
                    )
                };
                log::debug!(
                    "[drm] SETPROPERTY connector={} prop={} val={} (accepted, no-op)",
                    connector_id,
                    prop_id,
                    value
                );
                Ok(0)
            }
            DRM_IOCTL_MODE_CURSOR | DRM_IOCTL_MODE_CURSOR2 => {
                // Kernel-composited hardware cursor. wlroots is forced onto the
                // legacy KMS path (atomic is rejected), so it drives the pointer
                // with these ioctls; `scanout()` draws the bitmap over each
                // frame. The `drm_mode_cursor2` layout begins with the same 28
                // bytes as `drm_mode_cursor` (flags, crtc_id, x, y, width,
                // height, handle) and only appends hot_x/hot_y — which we don't
                // need for drawing, since the compositor pre-adjusts x/y for the
                // hotspot — so one 28-byte view serves both ioctls.
                #[repr(C)]
                struct DrmModeCursor {
                    flags: u32,
                    crtc_id: u32,
                    x: i32,
                    y: i32,
                    width: u32,
                    height: u32,
                    handle: u32,
                }
                const DRM_MODE_CURSOR_BO: u32 = 0x01;
                const DRM_MODE_CURSOR_MOVE: u32 = 0x02;
                let cur = unsafe { &*(data as *const DrmModeCursor) };
                let mut changed = false;
                if cur.flags & DRM_MODE_CURSOR_BO != 0 {
                    changed |= drm::set_cursor_bo(cur.handle, cur.width, cur.height);
                }
                if cur.flags & DRM_MODE_CURSOR_MOVE != 0 {
                    drm::move_cursor(cur.x, cur.y);
                    changed = true;
                }
                if changed {
                    drm::repaint_for_cursor();
                }
                Ok(0)
            }
            DRM_IOCTL_GEM_CLOSE => {
                let handle = unsafe { *(data as *const u32) };
                // linux-object's own CREATE_DUMB/PRIME table first; a miss
                // there might still be a driver-private handle (e.g.
                // nouveau-uAPI GEM_NEW) the driver itself keeps track of.
                if drm::gem_close(handle) {
                    Ok(0)
                } else if drm::get_primary_driver()
                    .map(|d| d.nouveau_gem_close(handle))
                    .unwrap_or(false)
                {
                    Ok(0)
                } else {
                    Err(FsError::InvalidParam)
                }
            }
            DRM_IOCTL_MODE_GETRESOURCES => {
                let res = unsafe { &mut *(data as *mut DrmModeCardRes) };
                let (fbs, crtcs, connectors) = drm::get_resources();

                if res.fb_id_ptr != 0 && res.count_fbs >= fbs.len() as u32 {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            fbs.as_ptr(),
                            res.fb_id_ptr as *mut u32,
                            fbs.len(),
                        );
                    }
                }
                if res.crtc_id_ptr != 0 && res.count_crtcs >= crtcs.len() as u32 {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            crtcs.as_ptr(),
                            res.crtc_id_ptr as *mut u32,
                            crtcs.len(),
                        );
                    }
                }
                if res.connector_id_ptr != 0 && res.count_connectors >= connectors.len() as u32 {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            connectors.as_ptr(),
                            res.connector_id_ptr as *mut u32,
                            connectors.len(),
                        );
                    }
                }

                res.count_fbs = fbs.len() as u32;
                res.count_crtcs = crtcs.len() as u32;
                res.count_connectors = connectors.len() as u32;

                // Always expose the synthetic encoder when connectors exist so
                // that `drmIsKMS` (which checks count_crtcs > 0 &&
                // count_connectors > 0 && count_encoders > 0) succeeds.  The
                // synthetic encoder is valid for both the software KMS path and
                // the hardware KMS path: `possible_crtcs = 1` maps to index 0
                // of the CRTC list, which is SYNTH_CRTC_ID on the software path
                // and the hardware CRTC on the hardware path.
                if !connectors.is_empty() {
                    if res.encoder_id_ptr != 0 && res.count_encoders >= 1 {
                        unsafe {
                            *(res.encoder_id_ptr as *mut u32) = drm::SYNTH_ENCODER_ID;
                        }
                    }
                    res.count_encoders = 1;
                } else {
                    res.count_encoders = 0;
                }

                if let Some(caps) = drm::get_caps() {
                    res.max_width = caps.max_width;
                    res.max_height = caps.max_height;
                }
                Ok(0)
            }
            DRM_IOCTL_MODE_GETCONNECTOR => {
                let conn_res = unsafe { &mut *(data as *mut DrmModeGetConnector) };
                if let Some(conn) = drm::get_connector(conn_res.connector_id) {
                    conn_res.connection = if conn.connected { 1 } else { 2 };
                    // Physical dimensions, best source first:
                    //  1. the connector's own values (NVIDIA RM data);
                    //  2. the display's EDID — the preferred detailed timing
                    //     carries the image size in mm (bytes 66/67/68 of the
                    //     descriptor), and bytes 21/22 give cm as a coarser
                    //     fallback (observed: a 32" TV reported as 270x203mm by
                    //     the old 96-DPI guess vs its real 885x497mm, skewing
                    //     every DPI-aware client);
                    //  3. a ~96 DPI guess from the resolution, so wlroots never
                    //     sees "Physical size: 0x0".
                    let edid_mm = drm::get_connector_edid(conn_res.connector_id).and_then(|e| {
                        let d = &e[54..72];
                        let pixel_clock = u16::from_le_bytes([d[0], d[1]]);
                        if pixel_clock != 0 {
                            let w = d[12] as u32 | ((d[14] as u32 & 0xF0) << 4);
                            let h = d[13] as u32 | ((d[14] as u32 & 0x0F) << 8);
                            if (10..=2000).contains(&w) && (10..=2000).contains(&h) {
                                return Some((w, h));
                            }
                        }
                        // Coarse: max image size in cm (0 = unspecified).
                        let (w_cm, h_cm) = (e[21] as u32, e[22] as u32);
                        if w_cm > 0 && h_cm > 0 {
                            Some((w_cm * 10, h_cm * 10))
                        } else {
                            None
                        }
                    });
                    let (fallback_w, fallback_h) = edid_mm.unwrap_or_else(|| {
                        drm::display_mode()
                            .map(|(w, h, _)| {
                                // Assume ~96 DPI (1 in = 25.4 mm, 96 px/in).
                                ((w * 254 / 960).max(1), (h * 254 / 960).max(1))
                            })
                            .unwrap_or((1, 1))
                    });
                    conn_res.mm_width = if conn.mm_width > 0 {
                        conn.mm_width
                    } else {
                        fallback_w
                    };
                    conn_res.mm_height = if conn.mm_height > 0 {
                        conn.mm_height
                    } else {
                        fallback_h
                    };
                    // Real DRM_MODE_CONNECTOR_* from the driver (NVIDIA fills
                    // it from the RM's GET_CONNECTOR_DATA); synthetic and
                    // fallback connectors keep the historical 11.
                    conn_res.connector_type = conn.connector_type;
                    conn_res.connector_type_id = 1;
                    conn_res.encoder_id = drm::SYNTH_ENCODER_ID;
                    conn_res.subpixel = 0; // SubPixelUnknown

                    // Report exactly one encoder. wlroots calls this twice: once
                    // to learn the counts, then again with allocated arrays.
                    if conn_res.encoders_ptr != 0 && conn_res.count_encoders >= 1 {
                        unsafe {
                            *(conn_res.encoders_ptr as *mut u32) = drm::SYNTH_ENCODER_ID;
                        }
                    }
                    conn_res.count_encoders = 1;

                    // Report exactly one mode: the display's native resolution.
                    if let Some((w, h, _)) = drm::display_mode() {
                        if conn_res.modes_ptr != 0 && conn_res.count_modes >= 1 {
                            let mode = make_modeinfo(w, h);
                            unsafe {
                                core::ptr::copy_nonoverlapping(
                                    mode.as_ptr(),
                                    conn_res.modes_ptr as *mut u8,
                                    mode.len(),
                                );
                            }
                        }
                        conn_res.count_modes = 1;
                    } else {
                        conn_res.count_modes = 0;
                    }
                    // Standard connector properties (DPMS, link-status,
                    // non-desktop, EDID, and CRTC_ID for atomic clients), via
                    // the usual two-call count/fill pattern.
                    let props = connector_props(conn_res.connector_id);
                    if !props.is_empty()
                        && conn_res.props_ptr != 0
                        && conn_res.prop_values_ptr != 0
                        && conn_res.count_props >= props.len() as u32
                    {
                        for (i, (pid, val)) in props.iter().enumerate() {
                            unsafe {
                                *(conn_res.props_ptr as *mut u32).add(i) = *pid;
                                *(conn_res.prop_values_ptr as *mut u64).add(i) = *val;
                            }
                        }
                    }
                    conn_res.count_props = props.len() as u32;
                    log::debug!(
                        "[drm] GETCONNECTOR id={} connected={} modes={} mode={:?}",
                        conn_res.connector_id,
                        conn.connected,
                        conn_res.count_modes,
                        drm::display_mode()
                    );
                    Ok(0)
                } else {
                    log::debug!(
                        "[drm] GETCONNECTOR id={} -> NOT FOUND",
                        conn_res.connector_id
                    );
                    Err(FsError::InvalidParam)
                }
            }
            DRM_IOCTL_MODE_GETENCODER => {
                let enc = unsafe { &mut *(data as *mut DrmModeGetEncoder) };
                enc.encoder_id = drm::SYNTH_ENCODER_ID;
                // DRM_MODE_ENCODER_VIRTUAL=6: correct type for a software/
                // virtual encoder that drives a dumb-buffer scanout path.
                // Reporting NONE(0) causes some compositors to skip property
                // queries and misidentify the output type.
                enc.encoder_type = 6; // DRM_MODE_ENCODER_VIRTUAL
                                      // On the software KMS path the only CRTC is the synthetic one
                                      // (id=1). On the hardware KMS path, report crtc_id=0 (no
                                      // currently active CRTC) because CRTC 1 does not appear in the
                                      // resource list that hardware drivers expose; wlroots will
                                      // configure the CRTC itself via SETCRTC.
                                      // possible_crtcs=1 means bit 0 = index 0 of the CRTC list,
                                      // which is correct in both paths.
                enc.crtc_id = if drm::software_kms_active() {
                    drm::SYNTH_CRTC_ID
                } else {
                    0
                };
                enc.possible_crtcs = 1; // bitmask: CRTC index 0
                enc.possible_clones = 0;
                Ok(0)
            }
            DRM_IOCTL_MODE_GETCRTC => {
                let crtc_res = unsafe { &mut *(data as *mut DrmModeGetCrtc) };
                if let Some(crtc) = drm::get_crtc(crtc_res.crtc_id) {
                    crtc_res.fb_id = crtc.fb_id;
                    crtc_res.x = crtc.x;
                    crtc_res.y = crtc.y;
                    crtc_res.gamma_size = 0;
                    // Report the current mode: the display's native timings
                    // (the only mode the pipeline has). Linux fills this from
                    // crtc->state; compositors read it back to seed their
                    // initial output state.
                    if let Some((w, h, _)) = drm::display_mode() {
                        crtc_res.mode = make_modeinfo(w, h);
                        crtc_res.mode_valid = 1;
                    } else {
                        crtc_res.mode_valid = 0;
                    }
                    Ok(0)
                } else {
                    Err(FsError::InvalidParam)
                }
            }
            DRM_IOCTL_MODE_GETPLANERESOURCES => {
                let res = unsafe { &mut *(data as *mut DrmModeGetPlaneRes) };
                let planes = drm::get_planes();
                if res.plane_id_ptr != 0 && res.count_planes >= planes.len() as u32 {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            planes.as_ptr(),
                            res.plane_id_ptr as *mut u32,
                            planes.len(),
                        );
                    }
                }
                res.count_planes = planes.len() as u32;
                Ok(0)
            }
            DRM_IOCTL_MODE_GETPLANE => {
                let res = unsafe { &mut *(data as *mut DrmModeGetPlane) };
                if let Some(plane) = drm::get_plane(res.plane_id) {
                    res.crtc_id = plane.crtc_id;
                    res.fb_id = plane.fb_id;
                    res.possible_crtcs = plane.possible_crtcs;
                    // Advertise the formats the software scanout consumes, via
                    // the two-call pattern (count first, then fill).
                    const FORMATS: [u32; 2] = [
                        0x3432_5258, // DRM_FORMAT_XRGB8888 ("XR24")
                        0x3432_5241, // DRM_FORMAT_ARGB8888 ("AR24")
                    ];
                    if res.format_type_ptr != 0 && res.count_format_types >= FORMATS.len() as u32 {
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                FORMATS.as_ptr(),
                                res.format_type_ptr as *mut u32,
                                FORMATS.len(),
                            );
                        }
                    }
                    res.count_format_types = FORMATS.len() as u32;
                    Ok(0)
                } else {
                    Err(FsError::InvalidParam)
                }
            }
            DRM_IOCTL_MODE_OBJ_GETPROPERTIES => {
                let res = unsafe { &mut *(data as *mut DrmModeObjGetProperties) };
                // Identify the object by id (libdrm often passes obj_type=ANY).
                // Look up any registered plane — not only SYNTH_PLANE_ID — so
                // hardware planes (e.g. NVIDIA 3001, VirtIO 3000) are also
                // classified as PRIMARY/OVERLAY/CURSOR by wlroots. Atomic
                // properties (FB_ID, CRTC_ID, ACTIVE, MODE_ID, rects) only
                // appear for atomic clients, like Linux's atomic filtering;
                // legacy clients keep seeing exactly the pre-atomic set.
                let props: alloc::vec::Vec<(u32, u64)> = if let Some(p) = drm::get_plane(res.obj_id)
                {
                    plane_props(&p)
                } else if drm::get_crtc(res.obj_id).is_some() {
                    crtc_props()
                } else if drm::get_connector(res.obj_id).is_some() {
                    connector_props(res.obj_id)
                } else {
                    // Encoders exist but carry no properties; anything
                    // else is unknown. Keep the historical empty-list
                    // answer (Linux: EINVAL/ENOENT) — some clients probe
                    // every id returned by GETRESOURCES.
                    alloc::vec::Vec::new()
                };
                let n = props.len();
                // Both output arrays are written below, so both pointers must be
                // non-null: a client passing props_ptr set but prop_values_ptr=0
                // would otherwise trigger a kernel write to address 0.
                if n > 0
                    && res.props_ptr != 0
                    && res.prop_values_ptr != 0
                    && (res.count_props as usize) >= n
                {
                    for (i, (pid, val)) in props.iter().enumerate() {
                        unsafe {
                            *(res.props_ptr as *mut u32).add(i) = *pid;
                            *(res.prop_values_ptr as *mut u64).add(i) = *val;
                        }
                    }
                }
                res.count_props = n as u32;
                log::debug!(
                    "[drm] OBJ_GETPROPERTIES obj_id={} obj_type={:#x} -> {} props",
                    res.obj_id,
                    res.obj_type,
                    n
                );
                Ok(0)
            }
            DRM_IOCTL_MODE_GETPROPERTY => {
                let res = unsafe { &mut *(data as *mut DrmModeGetProperty) };
                let spec = match prop_spec(res.prop_id) {
                    Some(s) => s,
                    None => return Err(FsError::EntryNotFound),
                };
                res.flags = spec.flags;
                let mut name = [0u8; 32];
                let n = spec.name.len().min(31);
                name[..n].copy_from_slice(&spec.name.as_bytes()[..n]);
                res.name = name;
                // Two-call pattern: report counts, fill when arrays are big
                // enough. Enum properties expose both the (value, name) pairs
                // and the raw value list; range/object properties only values.
                if !spec.enums.is_empty()
                    && res.enum_blob_ptr != 0
                    && (res.count_enum_blobs as usize) >= spec.enums.len()
                {
                    for (i, (val, nm)) in spec.enums.iter().enumerate() {
                        let mut e = DrmModePropertyEnum {
                            value: *val,
                            name: [0u8; 32],
                        };
                        let ln = nm.len().min(31);
                        e.name[..ln].copy_from_slice(&nm.as_bytes()[..ln]);
                        unsafe {
                            *(res.enum_blob_ptr as *mut DrmModePropertyEnum).add(i) = e;
                        }
                    }
                }
                res.count_enum_blobs = spec.enums.len() as u32;
                if !spec.values.is_empty()
                    && res.values_ptr != 0
                    && (res.count_values as usize) >= spec.values.len()
                {
                    for (i, v) in spec.values.iter().enumerate() {
                        unsafe {
                            *(res.values_ptr as *mut u64).add(i) = *v;
                        }
                    }
                }
                res.count_values = spec.values.len() as u32;
                Ok(0)
            }
            DRM_IOCTL_MODE_GETPROPBLOB => {
                let res = unsafe { &mut *(data as *mut DrmModeGetBlob) };
                // Property-blob store first (user MODE_ID blobs, the kernel's
                // current-mode blob); EDID blobs keep their reserved
                // 20000+connector ids.
                if let Some(blob) = drm::get_blob(res.blob_id) {
                    if res.data != 0 && res.length >= blob.len() as u32 {
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                blob.as_ptr(),
                                res.data as *mut u8,
                                blob.len(),
                            );
                        }
                    }
                    res.length = blob.len() as u32;
                    return Ok(0);
                }
                let connector_id = res.blob_id.checked_sub(20000);
                if let Some(conn_id) = connector_id {
                    if let Some(edid) = drm::get_connector_edid(conn_id) {
                        if res.data != 0 && res.length >= edid.len() as u32 {
                            unsafe {
                                core::ptr::copy_nonoverlapping(
                                    edid.as_ptr(),
                                    res.data as *mut u8,
                                    edid.len(),
                                );
                            }
                        }
                        res.length = edid.len() as u32;
                        Ok(0)
                    } else {
                        Err(FsError::InvalidParam)
                    }
                } else {
                    Err(FsError::InvalidParam)
                }
            }
            DRM_IOCTL_MODE_CREATEPROPBLOB => {
                let req = unsafe { &mut *(data as *mut DrmModeCreateBlob) };
                // Linux: NULL data / zero length are EINVAL. Bound the copy —
                // real blobs (modes, gamma LUTs) are at most a few KiB.
                if req.data == 0 || req.length == 0 || req.length > 64 * 1024 {
                    return Err(FsError::InvalidParam);
                }
                let src = unsafe {
                    core::slice::from_raw_parts(req.data as *const u8, req.length as usize)
                };
                req.blob_id = drm::create_blob(src.to_vec(), true);
                log::debug!(
                    "[drm] CREATEPROPBLOB len={} -> blob={}",
                    req.length,
                    req.blob_id
                );
                Ok(0)
            }
            DRM_IOCTL_MODE_DESTROYPROPBLOB => {
                // struct drm_mode_destroy_blob { __u32 blob_id; }
                let blob_id = unsafe { *(data as *const u32) };
                match drm::destroy_blob(blob_id) {
                    drm::BlobDestroy::Destroyed => Ok(0),
                    drm::BlobDestroy::NotFound => Err(FsError::EntryNotFound),
                    // Linux: only the creator may destroy a blob -> EPERM-ish.
                    drm::BlobDestroy::KernelOwned => Err(FsError::NoPermission),
                }
            }
            DRM_IOCTL_MODE_ATOMIC => {
                let req = unsafe { &*(data as *const DrmModeAtomic) };
                // Linux contract: the client must have negotiated
                // DRM_CLIENT_CAP_ATOMIC (EINVAL otherwise), flags must be
                // known, reserved must be 0, TEST_ONLY cannot carry a flip
                // event, and async flips are refused when unsupported.
                if !drm::atomic_client() {
                    return Err(FsError::InvalidParam);
                }
                if req.flags & !DRM_MODE_ATOMIC_FLAGS != 0 || req.reserved != 0 {
                    return Err(FsError::InvalidParam);
                }
                if req.flags & DRM_MODE_PAGE_FLIP_ASYNC != 0 {
                    // DRM_CAP_ASYNC_PAGE_FLIP / ATOMIC_ASYNC_PAGE_FLIP are 0.
                    return Err(FsError::InvalidParam);
                }
                let test_only = req.flags & DRM_MODE_ATOMIC_TEST_ONLY != 0;
                let want_event = req.flags & DRM_MODE_PAGE_FLIP_EVENT != 0;
                let allow_modeset = req.flags & DRM_MODE_ATOMIC_ALLOW_MODESET != 0;
                if test_only && want_event {
                    return Err(FsError::InvalidParam);
                }
                if req.count_objs == 0 {
                    // Empty commit: no objects, no events (Linux allows it).
                    return Ok(0);
                }
                // The pipeline has 3 objects x <=16 properties; bound the
                // user-array walk well above that.
                if req.count_objs > 64 || req.objs_ptr == 0 || req.count_props_ptr == 0 {
                    return Err(FsError::InvalidParam);
                }
                let mut upd = drm::AtomicUpdate::default();
                let mut prop_idx = 0usize;
                for i in 0..req.count_objs as usize {
                    let obj_id = unsafe { *(req.objs_ptr as *const u32).add(i) };
                    let count_props = unsafe { *(req.count_props_ptr as *const u32).add(i) };
                    if count_props > 64 {
                        return Err(FsError::InvalidParam);
                    }
                    if count_props > 0 && (req.props_ptr == 0 || req.prop_values_ptr == 0) {
                        return Err(FsError::InvalidParam);
                    }
                    for _ in 0..count_props {
                        let prop_id = unsafe { *(req.props_ptr as *const u32).add(prop_idx) };
                        let value = unsafe { *(req.prop_values_ptr as *const u64).add(prop_idx) };
                        prop_idx += 1;
                        atomic_stage(&mut upd, obj_id, prop_id, value)?;
                    }
                }
                log::debug!(
                    "[drm] ATOMIC objs={} test_only={} allow_modeset={} event={} fb={:?} mode_blob={:?} active={:?}",
                    req.count_objs,
                    test_only,
                    allow_modeset,
                    want_event,
                    upd.plane_fb_id,
                    upd.mode_blob,
                    upd.active
                );
                drm::atomic_commit(&upd, test_only, allow_modeset, want_event, req.user_data)
                    .map_err(|e| match e {
                        drm::AtomicError::Invalid => FsError::InvalidParam,
                        drm::AtomicError::NotFound => FsError::EntryNotFound,
                        drm::AtomicError::Device => FsError::DeviceError,
                    })?;
                Ok(0)
            }
            DRM_IOCTL_SYNCOBJ_CREATE => {
                if !zcore_drivers::display::nouveau_uapi_enabled() {
                    return Err(FsError::OpNotSupported);
                }
                let req = unsafe { &mut *(data as *mut DrmSyncobjCreate) };
                req.handle = zcore_drivers::scheme::syncobj::create(
                    req.flags & DRM_SYNCOBJ_CREATE_SIGNALED != 0,
                );
                Ok(0)
            }

            DRM_IOCTL_SYNCOBJ_DESTROY => {
                if !zcore_drivers::display::nouveau_uapi_enabled() {
                    return Err(FsError::OpNotSupported);
                }
                let req = unsafe { &*(data as *const DrmSyncobjDestroy) };
                if zcore_drivers::scheme::syncobj::destroy(req.handle) {
                    Ok(0)
                } else {
                    Err(FsError::EntryNotFound)
                }
            }

            DRM_IOCTL_SYNCOBJ_RESET | DRM_IOCTL_SYNCOBJ_SIGNAL => {
                if !zcore_drivers::display::nouveau_uapi_enabled() {
                    return Err(FsError::OpNotSupported);
                }
                let req = unsafe { &*(data as *const DrmSyncobjArray) };
                const MAX_HANDLES: u32 = 64;
                if req.count_handles == 0 || req.count_handles > MAX_HANDLES || req.handles == 0 {
                    return Err(FsError::InvalidParam);
                }
                let apply = if cmd == DRM_IOCTL_SYNCOBJ_RESET {
                    zcore_drivers::scheme::syncobj::reset
                } else {
                    zcore_drivers::scheme::syncobj::signal
                };
                for i in 0..req.count_handles {
                    let handle = unsafe { *(req.handles as *const u32).add(i as usize) };
                    if !apply(handle) {
                        return Err(FsError::EntryNotFound);
                    }
                }
                Ok(0)
            }

            DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL => {
                if !zcore_drivers::display::nouveau_uapi_enabled() {
                    return Err(FsError::OpNotSupported);
                }
                let req = unsafe { &*(data as *const DrmSyncobjTimelineArray) };
                const MAX_HANDLES: u32 = 64;
                if req.count_handles == 0
                    || req.count_handles > MAX_HANDLES
                    || req.handles == 0
                    || req.points == 0
                {
                    return Err(FsError::InvalidParam);
                }
                for i in 0..req.count_handles as usize {
                    let handle = unsafe { *(req.handles as *const u32).add(i) };
                    let point = unsafe { *(req.points as *const u64).add(i) };
                    if !zcore_drivers::scheme::syncobj::timeline_signal(handle, point) {
                        return Err(FsError::EntryNotFound);
                    }
                }
                Ok(0)
            }

            DRM_IOCTL_SYNCOBJ_QUERY => {
                if !zcore_drivers::display::nouveau_uapi_enabled() {
                    return Err(FsError::OpNotSupported);
                }
                let req = unsafe { &*(data as *const DrmSyncobjTimelineArray) };
                const MAX_HANDLES: u32 = 64;
                if req.count_handles == 0
                    || req.count_handles > MAX_HANDLES
                    || req.handles == 0
                    || req.points == 0
                {
                    return Err(FsError::InvalidParam);
                }
                // `DRM_SYNCOBJ_QUERY_FLAGS_LAST_SUBMITTED` is a no-op here:
                // this model has no separate submitted-vs-signaled state
                // (everything is signaled synchronously), so "current point"
                // answers both.
                for i in 0..req.count_handles as usize {
                    let handle = unsafe { *(req.handles as *const u32).add(i) };
                    let Some(point) = zcore_drivers::scheme::syncobj::query(handle) else {
                        return Err(FsError::EntryNotFound);
                    };
                    unsafe { *(req.points as *mut u64).add(i) = point };
                }
                Ok(0)
            }

            DRM_IOCTL_SYNCOBJ_WAIT | DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT => {
                if !zcore_drivers::display::nouveau_uapi_enabled() {
                    return Err(FsError::OpNotSupported);
                }
                let timeline = cmd == DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT;
                // Both structs share this prefix layout, so a single path
                // can read the common fields regardless of which ioctl.
                let (handles_ptr, points_ptr, timeout_nsec, count_handles, flags) = if timeline {
                    let req = unsafe { &*(data as *const DrmSyncobjTimelineWait) };
                    (req.handles, req.points, req.timeout_nsec, req.count_handles, req.flags)
                } else {
                    let req = unsafe { &*(data as *const DrmSyncobjWait) };
                    (req.handles, 0, req.timeout_nsec, req.count_handles, req.flags)
                };
                const MAX_HANDLES: u32 = 64;
                if count_handles == 0 || count_handles > MAX_HANDLES || handles_ptr == 0 {
                    return Err(FsError::InvalidParam);
                }
                let handles: alloc::vec::Vec<u32> = (0..count_handles as usize)
                    .map(|i| unsafe { *(handles_ptr as *const u32).add(i) })
                    .collect();
                let points: Option<alloc::vec::Vec<u64>> = if timeline && points_ptr != 0 {
                    Some(
                        (0..count_handles as usize)
                            .map(|i| unsafe { *(points_ptr as *const u64).add(i) })
                            .collect(),
                    )
                } else {
                    None
                };
                // `timeout_nsec` is an ABSOLUTE CLOCK_MONOTONIC deadline (real
                // Linux semantics, confirmed against this kernel's own
                // `now_monotonic()` -> `kernel_hal::timer::timer_now()`), not a
                // relative duration -- treating it as relative would turn any
                // real userspace deadline (computed from `clock_gettime`) into
                // an effectively unbounded wait.
                let deadline_us = (timeout_nsec.max(0) as u64) / 1000;
                let wait_all = flags & DRM_SYNCOBJ_WAIT_FLAGS_WAIT_ALL != 0;
                match zcore_drivers::scheme::syncobj::wait(&handles, points.as_deref(), wait_all, deadline_us) {
                    zcore_drivers::scheme::syncobj::WaitOutcome::Signaled { first_signaled_index } => {
                        if timeline {
                            let req = unsafe { &mut *(data as *mut DrmSyncobjTimelineWait) };
                            req.first_signaled = first_signaled_index;
                        } else {
                            let req = unsafe { &mut *(data as *mut DrmSyncobjWait) };
                            req.first_signaled = first_signaled_index;
                        }
                        Ok(0)
                    }
                    // Real Linux returns -ETIME; `FsError` has no exact match,
                    // `Again` ("not ready, retry") is the closest honest fit.
                    zcore_drivers::scheme::syncobj::WaitOutcome::Timeout => Err(FsError::Again),
                    zcore_drivers::scheme::syncobj::WaitOutcome::Invalid => Err(FsError::EntryNotFound),
                }
            }

            _ => {
                // Reverse-engineering hook: log every DRM ioctl wlroots/labwc
                // issues that we do not handle. The DRM nr (`(cmd >> 8) & 0xff`)
                // maps to a DRM_IOCTL_* command, so a photo of this line tells us
                // exactly what labwc wants next. `dir` 1=W 2=R 3=RW, `size` is the
                // arg struct length.
                let nr = (cmd >> 8) & 0xff;
                let size = (cmd >> 16) & 0x3fff;
                let dir = cmd >> 30;
                log::debug!(
                    "[drm] UNHANDLED ioctl cmd={:#010x} (drm nr={:#04x} size={} dir={})",
                    cmd,
                    nr,
                    size,
                    dir
                );
                if let Some(driver) = drm::get_primary_driver() {
                    driver
                        .ioctl_owned(cmd, data, drm::current_pid())
                        .map_err(|e| match e {
                            38 => FsError::NotSupported, // ENOSYS
                            _ => FsError::DeviceError,
                        })
                } else {
                    Err(FsError::NotSupported)
                }
            }
        }
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}
