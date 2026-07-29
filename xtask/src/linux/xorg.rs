//! Build-time population of the X.Org stack into the rootfs.
//!
//! Xorg used to be a *runtime* `apk add` chore (the on-screen hints in
//! `desktop.rs` all say "apk add …"): a fresh Eclipse install booted to a bare
//! shell and the user had to fetch xorg-server, an input driver and fonts by
//! hand before `startx` did anything — and even then a real-hardware run showed
//! the input driver (`xf86-input-libinput`) simply missing, so X came up with
//! no keyboard or mouse.
//!
//! This module bakes the whole stack into the image at build time with
//! `apk add --root <rootfs>` (Alpine's supported offline root-install path — no
//! chroot, no running target), so a built image has a working X server, the
//! libinput driver, a software-GL renderer, keyboard-map data and the base
//! fonts already present. `startx` then works out of the box in QEMU (the live
//! initramfs — see `image.rs`, which copies these paths in uncapped) and on
//! real hardware (the installed btrfs root).
//!
//! It is **best-effort**: a missing network, an unreachable mirror or an
//! unavailable package prints a warning and leaves the image buildable (exactly
//! like `nvidia_firmware`). The downloaded `.apk`s are cached under
//! `ignored/apk-cache` so a second build — or an offline one — reuses them.
//!
//! Knobs:
//!   * `ECLIPSE_XORG=0|off|no|false` — skip entirely (lean/minimal images).
//!   * `ECLIPSE_XORG_PACKAGES="pkg1 pkg2 …"` — replace the default package set
//!     (e.g. to match a non-Alpine repository whose names differ).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::PROJECT_DIR;

/// Default top-level package set. `apk` resolves the dependency closure
/// (libX11, pixman, libdrm, libinput, …) itself, so this lists only the
/// user-facing pieces. Names track the Alpine repositories written into
/// `/etc/apk/repositories` by `mod.rs`; override with `ECLIPSE_XORG_PACKAGES`
/// for a different provider.
const DEFAULT_PACKAGES: &[&str] = &[
    // The server itself. Brings the built-in `modesetting` driver, which is
    // what this kernel's DRM scheme (/dev/dri/card0) drives, plus GLX.
    "xorg-server",
    // THE piece missing on the real-hardware run: without an input driver X
    // starts with no keyboard or mouse. libinput is what labwc/this kernel's
    // evdev nodes are known to work with.
    "xf86-input-libinput",
    // `startx` / `xinit`.
    "xinit",
    // Software GL (`swrast_dri.so` / llvmpipe): the AIGLX log line
    // "dlopen of /usr/lib/dri/swrast_dri.so failed" came from this being
    // absent, leaving GLX with "no usable GL providers". Needed by GL clients
    // (Firefox). Heavy (~tens of MiB) but the one that makes GL work headless.
    "mesa-dri-gallium",
    "mesa-gl",
    // Keyboard: the layout database plus the tools X needs at runtime to
    // compile a keymap and let the user set one.
    "xkeyboard-config",
    "setxkbmap",
    "xkbcomp",
    // Fonts: X refuses to start without its base bitmap fonts (`fixed`) and the
    // cursor font; `encodings` is their companion. DejaVu covers scalable text.
    "font-misc-misc",
    "font-cursor-misc",
    "encodings",
    "font-dejavu",
    // A minimal in-X terminal so `startx` yields a usable session even with no
    // Wayland compositor installed (the `.xinitrc` falls back to `xterm`).
    "xterm",
    // Handy CLI knobs many desktops/scripts call (RandR + DPMS/screensaver).
    "xrandr",
    "xset",
];

/// Returns `true` unless `ECLIPSE_XORG` is explicitly set to a falsey value.
fn enabled() -> bool {
    match std::env::var("ECLIPSE_XORG") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "off" | "no" | "false" | ""
        ),
        Err(_) => true,
    }
}

/// Populate `rootfs` with the X.Org stack. `apk_bin` is the (host-runnable)
/// apk binary already staged into the rootfs by `mod.rs`, `arch` the target
/// arch name (e.g. "x86_64"). Best-effort: never panics, never fails the build.
pub(super) fn install(rootfs: &Path, apk_bin: &Path, arch: &str) {
    if !enabled() {
        println!("Xorg stack: skipped (ECLIPSE_XORG is off)");
        return;
    }

    // The apk binary is a static target-arch build; running it on the host to
    // populate the root only works when target arch == host arch. The desktop
    // is x86_64-only anyway, so restrict to that and skip cross-builds cleanly.
    if arch != "x86_64" {
        println!("Xorg stack: skipped (only wired for x86_64, target is {arch})");
        return;
    }

    if !apk_bin.is_file() {
        eprintln!(
            "warning: apk binary {apk_bin:?} not found; skipping Xorg install \
             (startx will need a runtime `apk add`)"
        );
        return;
    }

    let repos = rootfs.join("etc/apk/repositories");
    let keys = rootfs.join("etc/apk/keys");
    if !repos.is_file() {
        eprintln!(
            "warning: {repos:?} missing; skipping Xorg install \
             (apk repositories not set up)"
        );
        return;
    }

    // Persistent, gitignored cache so a re-build or an OFFLINE build reuses the
    // .apk files fetched by an earlier online build instead of hitting the
    // mirror again.
    let cache = PROJECT_DIR.join("ignored").join("apk-cache");
    let _ = std::fs::create_dir_all(&cache);

    let packages: Vec<String> = match std::env::var("ECLIPSE_XORG_PACKAGES") {
        Ok(list) if !list.trim().is_empty() => {
            list.split_whitespace().map(str::to_string).collect()
        }
        _ => DEFAULT_PACKAGES.iter().map(|s| s.to_string()).collect(),
    };

    println!(
        "Xorg stack: installing {} package(s) into {} via apk --root ...",
        packages.len(),
        rootfs.display()
    );

    let mut cmd = Command::new(apk_bin);
    cmd.arg("add")
        .arg("--root")
        .arg(rootfs)
        // This apk is apk-tools 3.x (Chimera static build), whose on-disk
        // database differs from the empty v2-style `lib/apk/db/installed` that
        // mod.rs lays down; without --initdb apk 3.x aborts with "Failed to
        // open apk database". --initdb has it create its own database in the
        // target root — the same format the identical runtime apk expects, and
        // safe because `make` clears the rootfs every build.
        .arg("--initdb")
        .arg("--arch")
        .arg(arch)
        .arg("--repositories-file")
        .arg(&repos)
        // Absolute, persistent cache. apk fetches a missing repository index
        // automatically and reuses a cached one, so NOT forcing --update-cache
        // lets an OFFLINE rebuild succeed off the .apk/index a prior online
        // build cached here (a forced refresh would hard-fail with no network).
        .arg("--cache-dir")
        .arg(&cache)
        // Post-install scripts (fontconfig cache, etc.) would need to run in the
        // target root via chroot; on a cross-root host install that is not
        // available. Skip them — the base font dirs are declared in the shipped
        // xorg.conf.d and caches regenerate on first use.
        .arg("--no-scripts");
    if keys.is_dir() {
        cmd.arg("--keys-dir").arg(&keys);
    }
    for p in &packages {
        cmd.arg(p);
    }

    let outcome = cmd.status();
    match &outcome {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!(
                "warning: `apk add` for the Xorg stack exited {s:?} (mirror \
                 unreachable, or a package name not in the configured repos?). \
                 The image is still usable; startx will need a runtime \
                 `apk add`, or set ECLIPSE_XORG_PACKAGES to match your repos."
            );
        }
        Err(e) => {
            eprintln!(
                "warning: could not run apk ({e}); skipping Xorg install. \
                 The image is still usable; startx will need a runtime `apk add`."
            );
        }
    }

    // Verify and report LOUDLY either way: the whole point is that `startx`
    // works, so a build that silently shipped without the server (a warned-past
    // apk failure) must be unmistakable, not a surprise at boot.
    let xserver = ["usr/bin/Xorg", "usr/bin/X"]
        .iter()
        .find(|p| rootfs.join(p).is_file());
    let startx = rootfs.join("usr/bin/startx").is_file();
    let libinput = rootfs
        .join("usr/lib/xorg/modules/input/libinput_drv.so")
        .is_file()
        || rootfs
            .join("usr/lib/xorg/modules/input/libinput_drv.la")
            .is_file();
    match (xserver, startx) {
        (Some(_), true) => {
            println!(
                "Xorg stack: OK — X server + startx present, input driver {}.",
                if libinput {
                    "present"
                } else {
                    "MISSING (no libinput_drv.so — X will have no input!)"
                }
            );
        }
        _ => {
            eprintln!(
                "======================================================================\n\
                 Xorg stack: NOT installed — the built image will say \
                 `sh: startx: not found`.\n\
                 apk could not fetch the packages (result: {outcome:?}).\n\
                 Most likely: no network to the mirror in /etc/apk/repositories, \
                 or the package\n names differ from your repo. To fix on a machine \
                 WITH internet:\n\
                 \x20 * run the build again (this step is best-effort and skips \
                 when offline), or\n\
                 \x20 * set ECLIPSE_XORG_PACKAGES=\"...\" to match your repo's names, or\n\
                 \x20 * `apk add` the stack once at runtime (it is cached).\n\
                 ======================================================================"
            );
        }
    }
}

// ─── Live/QEMU initramfs inclusion ──────────────────────────────────────────
//
// The installed system (real hardware) runs the FULL btrfs rootfs, so the
// `apk add --root` above is all it needs. QEMU, however, boots the *minimal
// live initramfs* (see image.rs `build_live_rootfs`), whose `LIVE_KEEP`
// deliberately omits `usr/bin` / `usr/lib` and whose per-file cap drops big
// files — so without help X would be present on disk but absent in QEMU.
//
// `copy_into_live` copies the X-owned trees into the live root UNCAPPED so
// `startx` works in QEMU too. It deliberately EXCLUDES `usr/lib/dri` (mesa's
// llvmpipe/`swrast_dri.so`, tens of MiB): the live root is RAM-resident, and X
// starts fine without GL — GLX is simply unavailable in QEMU, while the
// installed system keeps full software GL. Turn the whole thing off with
// `ECLIPSE_XORG_LIVE=0` to keep the installer initramfs lean.

/// X-owned trees copied verbatim (uncapped) from the full rootfs into the live
/// root. Missing entries are silently skipped, so this is safe whether or not
/// the `apk` install above actually ran.
const LIVE_TREES: &[&str] = &[
    "usr/bin",         // X, Xorg, startx, xinit, xterm, xkbcomp, setxkbmap, xrandr, xset
    "usr/lib",         // libX11/xcb/pixman/drm/input/xkbcommon + usr/lib/xorg modules (minus dri)
    "usr/libexec",     // Xorg.wrap on some layouts
    "usr/share/X11",   // xkb data, xorg.conf.d defaults, rgb.txt
    "usr/share/fonts", // base bitmap fonts X refuses to start without
    "usr/share/fontconfig",
    "etc/fonts",
];

fn live_enabled() -> bool {
    match std::env::var("ECLIPSE_XORG_LIVE") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "off" | "no" | "false" | ""
        ),
        Err(_) => true,
    }
}

/// Recursively copy `src` into `dst`, uncapped, preserving symlinks and
/// permissions, skipping any path under `skip`. Missing `src` is a no-op.
fn copy_uncapped(src: &Path, dst: &Path, skip: &Path) {
    if src == skip {
        return;
    }
    let md = match std::fs::symlink_metadata(src) {
        Ok(m) => m,
        Err(_) => return,
    };
    if md.file_type().is_symlink() {
        if let Ok(target) = std::fs::read_link(src) {
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::remove_file(dst);
            let _ = std::os::unix::fs::symlink(target, dst);
        }
        return;
    }
    if md.is_dir() {
        let _ = std::fs::create_dir_all(dst);
        if let Ok(rd) = std::fs::read_dir(src) {
            for entry in rd.flatten() {
                copy_uncapped(&entry.path(), &dst.join(entry.file_name()), skip);
            }
        }
        return;
    }
    if let Some(parent) = dst.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::copy(src, dst);
}

/// Total size in bytes of a directory tree (for the size notice). Best-effort.
fn tree_size(p: &Path) -> u64 {
    let md = match std::fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    if md.file_type().is_symlink() {
        return 0;
    }
    if md.is_dir() {
        std::fs::read_dir(p)
            .map(|rd| rd.flatten().map(|e| tree_size(&e.path())).sum())
            .unwrap_or(0)
    } else {
        md.len()
    }
}

/// Copy the X.Org stack from the `full` rootfs into the `live` (QEMU/initramfs)
/// root so `startx` works in QEMU too. Best-effort; no-op when disabled, when
/// Xorg was not installed, or per-tree when a source path is absent.
pub(super) fn copy_into_live(full: &Path, live: &Path) {
    if !enabled() || !live_enabled() {
        return;
    }
    // Only bother if the server actually got installed into the full rootfs.
    let have_xorg = ["usr/bin/Xorg", "usr/bin/X", "usr/lib/xorg"]
        .iter()
        .any(|p| full.join(p).exists());
    if !have_xorg {
        return;
    }

    // Exclude mesa's DRI drivers (llvmpipe/swrast): too heavy for the RAM
    // initramfs, and X runs without GL.
    let skip: PathBuf = full.join("usr/lib/dri");

    println!("Xorg stack: copying into live/QEMU initramfs (excluding usr/lib/dri) ...");
    for rel in LIVE_TREES {
        copy_uncapped(&full.join(rel), &live.join(rel), &skip);
    }
    let mib = tree_size(&live.join("usr")) / (1024 * 1024);
    println!("Xorg stack: live root usr/ is now ~{mib} MiB");
}
