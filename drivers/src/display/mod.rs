//! Only UEFI Display currently.

// The NVIDIA GPU driver is built on the x86_64-only `nvidia-rm-sys` shim
// (which compiles C objects and links the RM blob for that target), so the
// whole stack is gated to x86_64. Other arches get UEFI display only.
#[cfg(target_arch = "x86_64")]
mod nvidia;
#[cfg(target_arch = "x86_64")]
mod nvidia_hooks;
#[cfg(target_arch = "x86_64")]
mod nouveau_uapi;
mod uefi;

#[cfg(target_arch = "x86_64")]
pub use nvidia::{boot_edid, set_boot_edid, set_boot_fb_info, NvidiaGpu, NvidiaGpuDriverPci};
#[cfg(target_arch = "x86_64")]
pub use nouveau_uapi::set_enabled as set_nouveau_uapi_enabled;
pub use uefi::UefiDisplay;

/// The UEFI-captured EDID is only wired up on x86_64 (via the NVIDIA/UEFI
/// boot path). On other arches there is no boot EDID; readers (procfs
/// `gpuedid`, the DRM synthetic connector) get `None` and fall back to their
/// mode-derived estimates.
#[cfg(not(target_arch = "x86_64"))]
pub fn boot_edid() -> Option<([u8; 128], u32)> {
    None
}
