//! Fetches NVIDIA's real GSP-RM firmware image for Turing GPUs (source:
//! NVIDIA's own public `linux-firmware` distribution) into the rootfs,
//! where it's read at runtime (after rootfs mount, from `zCore`'s boot
//! sequence -- `drivers` can't reach the filesystem itself, see
//! `NvidiaGpu::set_gsp_firmware`) and handed to the vendored `kgspInitRm`
//! (nvidia-rm-sys/vendor/eclipse_rm_init.c).
//!
//! Only `gsp.bin` itself is needed here: it's the one genuinely
//! proprietary blob NVIDIA doesn't ship in the open-sourced RM core (real
//! Linux driver sources it the same way, via `request_firmware`/
//! `nv_get_firmware` in arch/nvalloc/unix/src/osinit.c). The Booter Load/
//! Unload ucodes and the GSP-RM RISC-V bootloader stub that linux-firmware
//! also ships alongside it are NOT needed -- confirmed those are already
//! compiled into the vendored RM core as `BINDATA_ARCHIVE` blobs
//! (`generated/g_bindata.c`, fetched via `kgspGetBinArchiveBooterLoadUcode_HAL`
//! / `kgspGetGspRmBootUcodeStorage_HAL` in kernel_gsp_booter.c / kernel_gsp.c),
//! not loaded from external files.
//!
//! TU102/TU104/TU106/TU116/TU117 (which includes the RTX 2060 Super's
//! TU106) all share one GSP firmware bucket -- confirmed via
//! linux-firmware's WHENCE file, which symlinks tu104/tu106 -> tu102 and
//! tu117 -> tu116 for the `gsp/` directory.
use std::{fs, path::Path};

// Must match the vendored open-gpu-kernel-modules version EXACTLY: kgspInitRm's
// _kgspFwContainerVerifyVersion (kernel_gsp.c) hard-fails with
// NV_ERR_INVALID_DATA unless the GSP firmware image's embedded .fwversion
// equals the RM's NV_VERSION_STRING. The submodule is pinned at the 570.144
// tag (the newest release for which NVIDIA publishes a matching GSP firmware
// in linux-firmware -- 610.x is not there yet), so this is 570.144 too.
const NVIDIA_FW_VERSION: &str = "570.144";
const NVIDIA_FW_URL: &str =
    "https://raw.githubusercontent.com/NVIDIA/linux-firmware/main/nvidia/tu102/gsp/gsp-570.144.bin";

/// Best-effort: a missing/failed download just means the real GPU driver
/// finds no firmware at runtime and reports that (same as upstream
/// `nvidia.ko` without `/lib/firmware/nvidia` installed) -- it must never
/// fail the whole OS image build, since GSP firmware is irrelevant to
/// every non-NVIDIA-GPU build and boot path.
pub(super) fn install(rootfs: &Path) {
    let dest_dir = rootfs.join("lib/firmware/nvidia/gsp");
    let dst = dest_dir.join("gsp.bin");
    if dst.is_file() {
        return;
    }
    if let Err(e) = fs::create_dir_all(&dest_dir) {
        eprintln!("warning: could not create {dest_dir:?}: {e}; skipping NVIDIA GSP firmware");
        return;
    }
    println!("Fetching NVIDIA GSP-RM firmware ({NVIDIA_FW_VERSION}) from linux-firmware...");
    let status = std::process::Command::new("wget")
        .arg(NVIDIA_FW_URL)
        .arg("-O")
        .arg(&dst)
        .status();
    let ok = matches!(&status, Ok(s) if s.success());
    if !ok {
        eprintln!(
            "warning: failed to fetch {NVIDIA_FW_URL} ({status:?}); NVIDIA GSP firmware will be unavailable"
        );
        let _ = fs::remove_file(&dst);
    }
}
