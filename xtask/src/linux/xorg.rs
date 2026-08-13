//! Build-time population of the X.Org stack into the rootfs.
//!
//! Xorg used to be a *runtime* `apk add` chore (the on-screen hints in
//! `desktop.rs` all say "apk add …"): a fresh Eclipse install booted to a bare
//! shell and the user had to fetch xorg-server, an input driver and fonts by
//! hand before `startx` did anything — and even then a real-hardware run showed
//! the input driver (`xf86-input-libinput`) simply missing, so X came up with
//! no keyboard or mouse.
//!
//! This module bakes the whole stack into the image at build time. It installs
//! the packages with `apk add --root` into a THROWAWAY staging root (Alpine's
//! offline root-install pulls the full dependency closure, incl. musl/base,
//! which must NOT clobber Eclipse's hand-staged base), then copies only the
//! X-owned trees (`usr/*`, X fonts/config) into the real rootfs. A built image
//! then has a working X server, the libinput driver, a software-GL renderer,
//! keyboard-map data and the base fonts already present, with the base system
//! untouched. `startx` works out of the box in QEMU (the live initramfs — see
//! `image.rs`, which copies these paths in uncapped) and on real hardware (the
//! installed btrfs root).
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
    // The server itself, plus GLX. It also ships the built-in `modesetting`
    // driver (DRM/`/dev/dri/card0`), but Eclipse deliberately drives X through
    // the framebuffer instead (see `xf86-video-fbdev` below and
    // `write_xorg_config` in desktop.rs).
    "xorg-server",
    // The framebuffer video driver. Eclipse's X runs on `/dev/fb0` via this
    // driver, NOT DRM/modesetting: the kernel exposes a plain linear
    // framebuffer (FbDev, linux-object/src/fs/devfs/fbdev.rs) with the fbdev
    // ioctls + mmap the driver needs, and going straight to the framebuffer
    // avoids the DRM dumb-buffer + KMS commit round-trip on every frame, which
    // is pure overhead on a software scanout. So install fbdev explicitly --
    // its absence used to force the modesetting fallback.
    "xf86-video-fbdev",
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
    // ── Hardware GL on NVIDIA via Zink + NVK (Vulkan) ───────────────────────
    // Since Mesa 25.1 the DEFAULT OpenGL path for NVIDIA GPUs is NO LONGER the
    // classic nvc0 Gallium driver (GEM_PUSHBUF) -- it is Zink (GL-on-Vulkan)
    // running on NVK (the open Vulkan driver for Turing+). Zink itself ships in
    // `mesa-dri-gallium` (`zink_dri.so`), but it needs a Vulkan driver + the
    // loader underneath, and neither was installed -- so on the RTX every GL
    // renderer attempt failed with "DRI2: failed to load driver" / vulkan
    // "ERROR_INCOMPATIBLE_DRIVER" and NOT A SINGLE nouveau ioctl was issued
    // (Mesa never reached the kernel: it was trying the Vulkan path with no
    // Vulkan present). This is also the path Eclipse's kernel already targets:
    // NVK speaks the new nouveau uAPI (VM_INIT/VM_BIND/EXEC), which is exactly
    // what `drivers/src/display/nouveau_uapi.rs` implements.
    //   - vulkan-loader:        libvulkan.so.1, the ICD loader
    //   - mesa-vulkan-nouveau:  NVK, the Vulkan driver (+ its ICD manifest in
    //                           usr/share/vulkan/icd.d/, which LIVE_TREES must
    //                           also carry -- see there)
    //   - vulkan-tools:         `vulkaninfo`/`vkcube`, so NVK bring-up can be
    //                           checked from a shell without labwc in the way
    "vulkan-loader",
    "mesa-vulkan-nouveau",
    "vulkan-tools",
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
    // A lightweight window manager for the plain Xorg session (desktop=xorg).
    // Without a WM the `.xinitrc` falls through to a bare xterm with no way to
    // move/resize windows; openbox is what its WM loop tries first. Small, no
    // GTK/desktop dependencies.
    "openbox",
    // Handy CLI knobs many desktops/scripts call (RandR + DPMS/screensaver).
    "xrandr",
    "xset",
    // ── XFCE4 desktop ───────────────────────────────────────────────────────
    // Explicit components rather than the `xfce4` metapackage: the meta drags
    // in xfce4-pulseaudio-plugin and with it the whole audio stack, useless on
    // this kernel. `startxfce4` ships in xfce4-session; the `.xinitrc` prefers
    // it over the bare-WM fallbacks when present.
    "xfce4-session",
    "xfwm4",
    "xfce4-panel",
    "xfdesktop",
    "xfce4-settings",
    "xfconf",
    "thunar",
    "garcon",
    "xfce4-terminal",
    "xfce4-appfinder",
    // xfce4-session aborts without a D-Bus session bus; dbus-x11 provides the
    // `dbus-launch` the `.xinitrc` wraps startxfce4 in.
    "dbus",
    "dbus-x11",
    // GTK's fallback icon theme: without it every unthemed icon in the panel,
    // Thunar and the settings dialogs renders as "missing image".
    "adwaita-icon-theme",
    // THE missing piece behind the session-killing abort.
    //
    // Alpine builds gdk-pixbuf 2.44 with every native loader turned OFF except
    // legacy XPM:
    //
    //   -Dpng=disabled -Djpeg=disabled -Dgif=disabled -Dtiff=disabled
    //   -Dothers=disabled -Dlegacy_xpm=enabled -Dglycin=enabled
    //
    // Decoding is delegated to glycin, which runs a separate loader BINARY per
    // format out of /usr/libexec/glycin-loaders/2+/. Those binaries live in
    // their own subpackages, and this image had none of them -- so gdk-pixbuf
    // could not decode PNG, JPEG or SVG by any path. That is not cosmetic:
    //
    //   Gtk-WARNING: Could not load a pixbuf from icon theme.
    //   Wnck:ERROR:../libwnck/xutils.c:1510:default_icon_at_size:
    //             assertion failed: (base)
    //
    // and libwnck's `base` there is a PNG compiled into libwnck's OWN
    // GResource -- nothing on disk, nothing theme-related. It can only be NULL
    // if PNG decoding is unavailable. -> xfce4-session SIGABRT, session lost.
    //
    // image-rs covers PNG/JPEG/WebP/BMP/GIF; svg covers Adwaita's icons.
    "glycin-image-rs",
    "glycin-svg",
    // `glycin-thumbnailer`, so the session-start probe can actually DECODE a
    // PNG instead of inferring it from which files exist. Every file-presence
    // proxy used so far has been wrong, and this image ships no
    // `gdk-pixbuf-thumbnailer` (the guest reports `png-decode=no-tool`,
    // `tools=gdk-pixbuf-query-loaders`). It earns its place at runtime too:
    // it is what generates Thunar's thumbnails.
    "glycin-thumbnailer",
    // Still wanted: glycin-svg decodes SVG *through* librsvg. It is a library
    // behind glycin here, NOT a `libpixbufloader-svg.so` -- testing for that
    // .so reported "missing" on images where librsvg was installed all along.
    "librsvg",
    // Provides `gdk-pixbuf-query-loaders`, which eclipse-x11-prepare needs to
    // write `loaders.cache`. That step is already unconditional, but it is
    // guarded by `command -v` -- so without this package it silently does
    // nothing and the SVG loader above is never registered.
    "gdk-pixbuf",
    // The mime database GTK names in the same warning; also what GIO needs to
    // return a valid GFileInfo (xfdesktop logs
    // `xfdesktop_regular_file_icon_new: assertion 'G_IS_FILE_INFO(file_info)'
    // failed` without it).
    "shared-mime-info",
    // The base theme every other icon theme inherits from; Adwaita pulls it in
    // as a dependency, but naming it keeps icon lookup working if the theme
    // set is ever trimmed.
    "hicolor-icon-theme",
    // ── labwc Wayland session ───────────────────────────────────────────────
    // Eclipse's own desktop is a labwc/wlroots session (see desktop.rs and
    // README-desktop.md): all its config, wrapper and autostart are generated
    // at build time, but the binaries were never installed -- so `labwc` on
    // the console failed with "real binary not found". These are that stack.
    //
    // labwc pulls its own runtime closure: wlroots (the software-KMS + libinput
    // backend this kernel's /dev/dri/card0 drives via the pixman renderer),
    // wayland-libs, libxkbcommon and pixman. Naming labwc is enough for those.
    "labwc",
    // seatd: the seat manager wlroots opens DRM and input devices through. With
    // no logind/elogind here, libseat otherwise has nothing to talk to. The
    // labwc wrapper prefers libseat's daemonless `builtin` backend (works as
    // root, no service), but installing seatd provides `libseat.so` itself --
    // without the package that backend is not even present -- and leaves the
    // daemon path available. `seatd-launch` also ships here.
    "seatd",
    // foot: the native Wayland terminal the autostart and every desktop path
    // launches. It was referenced everywhere and installed nowhere, so the
    // session came up with no usable terminal. Brings its own terminfo.
    "foot",
    // Wayland client/server libs and the protocol data files. labwc/foot pull
    // wayland-libs, but the protocol XML lives in `wayland-protocols`, which a
    // few clients read at runtime; name it so it is never the missing piece.
    "wayland-protocols",
];

/// Whether the build is running as root (euid 0), via `id -u` — no extra crate
/// dependency. If `id` can't be run we assume NON-root: that is the common
/// developer-build case, and it makes apk take the `--usermode` path (which a
/// genuine root build would then reject loudly rather than silently mis-owning
/// files).
fn running_as_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim() == "0")
        .unwrap_or(false)
}

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

/// Build one `apk add` invocation against the staging root. Factored out so the
/// bulk install and the per-package retry below cannot drift apart in their
/// flags — a retry that differed by one argument would "fail" for reasons that
/// have nothing to do with the package being tested.
#[allow(clippy::too_many_arguments)]
fn mk_apk_add(
    apk_bin: &Path,
    stage: &Path,
    arch: &str,
    repos: &Path,
    cache: &Path,
    keys: &Path,
    initdb: bool,
) -> Command {
    let mut cmd = Command::new(apk_bin);
    cmd.arg("add").arg("--root").arg(stage);
    // apk-tools 3.x (Chimera static build) needs --initdb to create its
    // database in the (empty) staging root — but only the first time; a later
    // add into the now-populated root must not re-init it.
    if initdb {
        cmd.arg("--initdb");
    }
    cmd.arg("--arch")
        .arg(arch)
        .arg("--repositories-file")
        .arg(repos)
        // Absolute, persistent cache. apk fetches a missing repository index
        // automatically and reuses a cached one, so NOT forcing --update-cache
        // lets an OFFLINE rebuild succeed off the .apk/index a prior online
        // build cached here (a forced refresh would hard-fail with no network).
        .arg("--cache-dir")
        .arg(cache)
        // Post-install scripts would need to chroot into the target; skip them
        // (font caches regenerate on first use).
        .arg("--no-scripts");
    // apk 3.x refuses to create a database as a non-root user without
    // --usermode, and refuses --usermode AS root ("--usermode not allowed as
    // root"). The build normally runs as an unprivileged user (`make` on the
    // developer's box); CI/sudo runs as root. Pass the flag only when non-root.
    if !running_as_root() {
        cmd.arg("--usermode");
    }
    if keys.is_dir() {
        cmd.arg("--keys-dir").arg(keys);
    }
    cmd
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
        "Xorg stack: installing {} package(s) via apk into a staging root ...",
        packages.len(),
    );

    // CRITICAL: install into a THROWAWAY staging root, NOT the real rootfs.
    // apk --initdb starts from an empty database, so to satisfy xorg-server it
    // pulls the ENTIRE dependency closure — including musl/libc and other base
    // packages — and writes them over whatever is at the target. Aimed at the
    // real rootfs that clobbered the hand-staged busybox/musl/ld-musl base and
    // made every shell SIGSEGV at boot (jump to a garbage PC). Install into a
    // scratch dir, then copy ONLY the X-owned trees (usr/*, X fonts, X config)
    // into the real rootfs — never /bin, /lib, /sbin or base /etc — so the base
    // system is untouched and X is purely additive.
    let stage = PROJECT_DIR.join("ignored").join("xorg-stage");
    let _ = std::fs::remove_dir_all(&stage);
    let _ = std::fs::create_dir_all(&stage);

    let mut cmd = mk_apk_add(apk_bin, &stage, arch, &repos, &cache, &keys, true);
    for p in &packages {
        cmd.arg(p);
    }

    let mut outcome = cmd.status();
    // `apk add` is ONE transaction: a single unresolvable name aborts all of
    // it, and the failure branch below only warns and skips the merge — so the
    // rootfs silently keeps whatever a PREVIOUS build left there. That reads as
    // success (X still starts, from the old files) while every package added
    // since is quietly absent. It is how `librsvg` was added to the list,
    // shipped in three builds, and never appeared in the image: the guest had
    // exactly one pixbuf loader, libpixbufloader-xpm.so.
    //
    // So on failure, retry package-by-package: the resolvable ones still land,
    // and the ones that do not get NAMED instead of taking the rest down with
    // them.
    let mut unresolved: Vec<String> = Vec::new();
    if !matches!(&outcome, Ok(s) if s.success()) {
        eprintln!(
            "warning: bulk `apk add` failed; retrying package-by-package so one \
             unresolvable name cannot void the whole X stack"
        );
        let _ = std::fs::remove_dir_all(&stage);
        let _ = std::fs::create_dir_all(&stage);
        let mut first = true;
        let mut any_ok = false;
        for p in &packages {
            let mut c = mk_apk_add(apk_bin, &stage, arch, &repos, &cache, &keys, first);
            c.arg(p);
            match c.status() {
                Ok(s) if s.success() => {
                    any_ok = true;
                    first = false;
                }
                _ => unresolved.push(p.clone()),
            }
        }
        if !unresolved.is_empty() {
            eprintln!(
                "warning: these packages could NOT be installed: {}",
                unresolved.join(" ")
            );
        }
        if any_ok {
            // Something installed, so the merge below is worth doing. Synthesise
            // a success status rather than restructuring the match: this is a
            // host-only (unix) build tool.
            use std::os::unix::process::ExitStatusExt;
            outcome = Ok(std::process::ExitStatus::from_raw(0));
        }
    }
    // Audit the staging root's apk database against what was ASKED for, every
    // build, success or not. `apk add` reports failure for the transaction as a
    // whole; it does not tell you which name it could not resolve, and the
    // package-by-package retry above only runs when the bulk call fails. So a
    // requested package could be quietly absent with nothing in the output
    // naming it -- which is exactly how `glycin-image-rs` (the loader that
    // decodes PNG, and therefore the difference between a working desktop and
    // an aborting one) went missing across several builds while the console
    // showed no error at all.
    //
    // Asking apk itself rather than parsing `lib/apk/db/installed`: this build
    // uses apk-tools 3.x, whose on-disk database format is not the 2.x text
    // file, and an audit that silently reads nothing would be worse than none
    // at all.
    let installed: Vec<String> = Command::new(apk_bin)
        .arg("info")
        .arg("--root")
        .arg(&stage)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if installed.is_empty() {
        eprintln!("warning: Xorg stack: `apk info` returned nothing; cannot audit what installed");
    } else {
        let missing: Vec<&String> = packages
            .iter()
            .filter(|p| !installed.iter().any(|i| i == *p))
            .collect();
        if missing.is_empty() {
            println!(
                "Xorg stack: all {} requested packages are installed ({} in the closure)",
                packages.len(),
                installed.len()
            );
        } else {
            eprintln!(
                "warning: Xorg stack: requested but NOT installed: {}",
                missing
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
    }
    match &outcome {
        Ok(s) if s.success() => {
            // Merge ONLY the X-owned trees from the staging root into the real
            // rootfs. Never copy bin/ lib/ sbin/ or base /etc: those are the
            // hand-staged base the closure would otherwise clobber. usr/lib
            // holds the X + mesa libs (musl lives in /lib, which we skip), so
            // the base loader/libc is preserved.
            const X_TREES: &[&str] = &[
                "usr/bin",
                "usr/lib",
                "usr/libexec",
                "usr/share",
                "etc/fonts",
                "etc/X11",
                // XFCE/GTK: session defaults (xfce4/xfconf per-channel XML,
                // autostart, menus) and the D-Bus policy files.
                "etc/xdg",
                "etc/dbus-1",
            ];
            let skip_nothing = stage.join("\0does-not-exist");
            for rel in X_TREES {
                copy_uncapped(&stage.join(rel), &rootfs.join(rel), &skip_nothing);
            }
            // Alpine's X binaries (Xorg, mcookie, xterm, …) are dynamically
            // linked against Alpine's musl. The hand-staged base ships Eclipse's
            // own (musl-cross) `ld-musl-x86_64.so.1`, and an Alpine binary run
            // against it jumps to a bogus low PC (observed: `mcookie` SIGSEGV at
            // pc=0x1bd0, "Couldn't create cookie"). musl keeps a stable,
            // BACKWARD-compatible ABI, so installing Alpine's (newer) loader as
            // the one `/lib/ld-musl-x86_64.so.1` makes BOTH sets work: Eclipse's
            // older-musl base binaries keep running on the newer loader, and the
            // Alpine X binaries get the musl they were built against. Copy ONLY
            // this single file from the closure (not the whole base), with an
            // explicit executable mode. ECLIPSE_XORG_MUSL=0 keeps Eclipse's
            // loader (X binaries then need a matching runtime musl instead).
            let use_alpine_musl = !matches!(
                std::env::var("ECLIPSE_XORG_MUSL")
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase()
                    .as_str(),
                "0" | "off" | "no" | "false"
            );
            let stage_ld = stage.join("lib/ld-musl-x86_64.so.1");
            let ld = rootfs.join("lib/ld-musl-x86_64.so.1");
            let libc_alias = rootfs.join("lib/libc.musl-x86_64.so.1");
            if use_alpine_musl && stage_ld.is_file() {
                if let Some(parent) = ld.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::remove_file(&ld);
                if std::fs::copy(&stage_ld, &ld).is_ok() {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&ld, std::fs::Permissions::from_mode(0o755));
                    println!(
                        "Xorg stack: installed Alpine musl as /lib/ld-musl-x86_64.so.1 \
                         (one coherent loader for base + X; ECLIPSE_XORG_MUSL=0 to keep Eclipse's)"
                    );
                }
            }
            // `libc.musl-x86_64.so.1` is the soname the binaries NEED; it is an
            // alias of the loader now in place. Create it additively.
            if ld.is_file() && !libc_alias.exists() {
                let _ = std::os::unix::fs::symlink("ld-musl-x86_64.so.1", &libc_alias);
            }
            // Xorg opens its logfile (`/var/log/Xorg.0.log`) very early — before
            // probing any device — and dies with a fatal "Cannot open log file"
            // if the directory is absent. The staged base has no `/var/log`, so
            // create it. `/tmp/.X11-unix` (the X socket dir) is created by the
            // server at runtime, and `/tmp` already exists, so we leave that be.
            let _ = std::fs::create_dir_all(rootfs.join("var/log"));
            // dbus writes its machine-id under /var/lib/dbus (seeded at first
            // boot by eclipse-x11-prepare from /etc/machine-id).
            let _ = std::fs::create_dir_all(rootfs.join("var/lib/dbus"));
            // Fontconfig scan scope — the single biggest labwc startup cost.
            //
            // The stock Alpine fonts.conf scans `/usr/share/fonts` RECURSIVELY,
            // which drags in the ~400+ X11 bitmap fonts (font-misc-misc, cursor,
            // 75dpi/100dpi, encodings, cyrillic) that Xorg reaches through its
            // OWN FontPath and that no fontconfig/pango client ever wants. A boot
            // trace of labwc (/proc/bootprofile) showed the first fontconfig user
            // opening + gunzipping + parsing every one of those files COLD — the
            // scan alone was ~110s of a ~229s startup, per-file amplified by the
            // slow cold-metadata path AND thrashing the dcache for everything
            // that ran after it.
            //
            // A prior `<rejectfont>` glob did NOT fix this and the comment that
            // claimed it did was wrong: reject filters at MATCH time, not SCAN
            // time — fontconfig still opens and parses every rejected file to
            // build its cache. The only thing that stops the scan is not listing
            // those directories. So ship a fonts.conf that scans ONLY the
            // scalable dirs the desktop actually uses (DejaVu for pango/foot,
            // Adwaita for GTK); the bitmap packages stay on disk for Xorg. The
            // conf.d rendering defaults are still included.
            let fc_confd = rootfs.join("etc/fonts/conf.d");
            let _ = std::fs::create_dir_all(&fc_confd);
            let _ = std::fs::write(
                rootfs.join("etc/fonts/fonts.conf"),
                b"<?xml version=\"1.0\"?>\n\
                  <!DOCTYPE fontconfig SYSTEM \"fonts.dtd\">\n\
                  <fontconfig>\n\
                  \x20 <description>Eclipse: scan only the scalable fonts the desktop uses; X11 bitmap fonts stay on disk for Xorg's FontPath but out of the fontconfig scan (see xtask/src/linux/xorg.rs)</description>\n\
                  \x20 <dir>/usr/share/fonts/dejavu</dir>\n\
                  \x20 <dir>/usr/share/fonts/Adwaita</dir>\n\
                  \x20 <dir>/usr/local/share/fonts</dir>\n\
                  \x20 <dir prefix=\"xdg\">fonts</dir>\n\
                  \x20 <dir>~/.fonts</dir>\n\
                  \x20 <cachedir>/var/cache/fontconfig</cachedir>\n\
                  \x20 <cachedir prefix=\"xdg\">fontconfig</cachedir>\n\
                  \x20 <include ignore_missing=\"yes\">/etc/fonts/conf.d</include>\n\
                  </fontconfig>\n",
            );
            // Where fontconfig persists its per-directory caches at first use.
            // On the installed btrfs root this makes the first-boot scan a
            // one-time cost instead of silently failing the cache write.
            let _ = std::fs::create_dir_all(rootfs.join("var/cache/fontconfig"));
            // Bulletproof the narrow scan: physically delete the X11 bitmap font
            // directories. The narrow fonts.conf above stops fontconfig from
            // listing them, but a boot trace showed /usr/share/fonts STILL being
            // scanned recursively — some conf.d fragment (or a package default)
            // re-adds the parent dir, dragging the ~400+ bitmap fonts back into
            // the scan (~110s of cold open+gunzip+parse on the critical path to
            // labwc's first frame). Removing the directories makes that scan find
            // nothing regardless of which config re-adds the parent. The desktop
            // uses only the scalable DejaVu + Adwaita fonts (kept); Xorg's `fixed`
            // core font goes with them, which is acceptable for the Wayland
            // desktop. Best-effort: a missing dir is fine.
            for bitmap_dir in [
                "usr/share/fonts/misc",
                "usr/share/fonts/75dpi",
                "usr/share/fonts/100dpi",
                "usr/share/fonts/cyrillic",
                "usr/share/fonts/encodings",
                "usr/share/fonts/Type1",
                "usr/share/fonts/util",
            ] {
                let _ = std::fs::remove_dir_all(rootfs.join(bitmap_dir));
            }
            // The etc/xdg merge above may have overwritten Eclipse's xfconf
            // defaults with Alpine's stock ones (desktop::install runs BEFORE
            // this function) — most importantly the xfwm4 channel that turns
            // the compositor OFF for the software framebuffer. Re-assert them.
            super::desktop::write_xfce_defaults(rootfs);
            // The usr/share merge above lands Alpine's icon themes on top of
            // ours, so re-assert the PNG fallbacks afterwards (they are only
            // written where no real icon exists).
            super::desktop::write_fallback_icons(rootfs);
            let _ = std::fs::remove_dir_all(&stage);
        }
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
            // The X server starting is NOT the same as the desktop starting.
            // adwaita-icon-theme is SVG, gdk-pixbuf has no built-in SVG loader,
            // and libwnck's default_icon_at_size g_asserts on a NULL pixbuf
            // rather than degrading — so a missing librsvg does not degrade the
            // icons, it kills xfce4-session and the whole session with it. The
            // check above would happily report OK for that image, and did.
            let loaders = std::fs::read_dir(rootfs.join("usr/lib/gdk-pixbuf-2.0"))
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path().join("loaders"))
                .find(|p| p.is_dir());
            let names: Vec<String> = loaders
                .iter()
                .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            // SVG support is EITHER a pixbuf loader module OR a glycin
            // loader. Alpine moved image decoding out of
            // `libpixbufloader-*.so` into glycin (gdk-pixbuf 2.44 pulls in
            // libglycin + glycin-svg, and librsvg becomes a library behind it
            // rather than a pixbuf module), so testing only for the .so
            // reported "NO SVG pixbuf loader" on an image that DID install
            // librsvg -- which is what this banner said for four builds.
            let glycin_svg = ["usr/lib/glycin-loaders", "usr/libexec/glycin"]
                .iter()
                .filter_map(|d| std::fs::read_dir(rootfs.join(d)).ok())
                .flatten()
                .flatten()
                .any(|e| {
                    let p = e.path();
                    p.file_name()
                        .map(|n| n.to_string_lossy().contains("svg"))
                        .unwrap_or(false)
                        || std::fs::read_dir(&p)
                            .into_iter()
                            .flatten()
                            .flatten()
                            .any(|c| c.file_name().to_string_lossy().contains("svg"))
                });
            let has_svg = names.iter().any(|n| n.contains("svg")) || glycin_svg;

            // Inventory what ACTUALLY landed in the rootfs, every build.
            //
            // Five rounds went into guessing why the desktop had no usable
            // icon: a missing package, a wrong package name, a stale loader
            // cache, glycin instead of pixbuf modules. Each guess cost a full
            // rebuild and a boot to disprove. All of it is answerable from the
            // staged tree at build time, so print it and stop guessing:
            // whether apk installed the thing is visible in whether its files
            // are here.
            let list = |rel: &str, limit: usize| -> String {
                match std::fs::read_dir(rootfs.join(rel)) {
                    Ok(rd) => {
                        let mut v: Vec<String> = rd
                            .flatten()
                            .map(|e| e.file_name().to_string_lossy().into_owned())
                            .collect();
                        v.sort();
                        let n = v.len();
                        v.truncate(limit);
                        if n > limit {
                            format!("{} (+{} more)", v.join(" "), n - limit)
                        } else if v.is_empty() {
                            "<empty>".to_string()
                        } else {
                            v.join(" ")
                        }
                    }
                    Err(_) => "<missing>".to_string(),
                }
            };
            println!("Xorg stack: icon themes: {}", list("usr/share/icons", 12));
            // The loaders live under a VERSIONED directory --
            // usr/libexec/glycin-loaders/2+/glycin-image-rs -- so listing
            // `usr/libexec/glycin` (as this did) always answered "<missing>",
            // including on builds where glycin-svg was installed the whole
            // time. Walk one level down instead.
            let glycin_loaders: Vec<String> = ["usr/libexec/glycin-loaders", "usr/lib/glycin-loaders"]
                .iter()
                .flat_map(|base| std::fs::read_dir(rootfs.join(base)).into_iter().flatten())
                .flatten()
                .flat_map(|ver| std::fs::read_dir(ver.path()).into_iter().flatten())
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            println!(
                "Xorg stack: glycin loaders: {}",
                if glycin_loaders.is_empty() {
                    "<none>".to_string()
                } else {
                    glycin_loaders.join(" ")
                }
            );
            // Did librsvg's own files arrive at all? If not, apk never
            // installed it despite it being in DEFAULT_PACKAGES; if yes, the
            // question is only which decode path uses it.
            let rsvg: Vec<String> = std::fs::read_dir(rootfs.join("usr/lib"))
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.contains("rsvg"))
                .collect();
            println!(
                "Xorg stack: librsvg files in usr/lib: {}",
                if rsvg.is_empty() {
                    "<none -- apk did NOT install librsvg>".to_string()
                } else {
                    rsvg.join(" ")
                }
            );
            if has_svg {
                println!(
                    "Xorg stack: SVG decode present ({} pixbuf loader(s){}).",
                    names.len(),
                    if glycin_svg { ", via glycin" } else { "" }
                );
            } else {
                eprintln!(
                    "======================================================================\n\
                     Xorg stack: NO SVG pixbuf loader ({} loader(s): {}).\n\
                     adwaita-icon-theme is SVG, so every icon lookup returns NULL and\n\
                     libwnck aborts xfce4-session on the first one — `startx` will bring\n\
                     up X and then lose the session to SIGABRT.\n\
                     `librsvg` is in DEFAULT_PACKAGES; if it is not here, apk did not\n\
                     install it (see any per-package warning above).\n\
                     ======================================================================",
                    names.len(),
                    if names.is_empty() {
                        "none".to_string()
                    } else {
                        names.join(" ")
                    },
                );
            }
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
    // seatd lives in /usr/sbin on some providers (Ubuntu); harmless when empty.
    "usr/sbin",
    // glibc runtime paths. The Alpine/musl stack never populates these, but a
    // glibc-built desktop stack (e.g. Ubuntu-packaged labwc/Xorg staged into
    // the rootfs) puts its loader in /lib64 and its libraries in
    // /lib/x86_64-linux-gnu; without copying them into the live root those
    // binaries are unrunnable in QEMU. Empty on pure-musl builds, so this
    // costs nothing there.
    "lib64",
    "lib/x86_64-linux-gnu",
    "usr/lib",         // libX11/xcb/pixman/drm/input/xkbcommon + usr/lib/xorg modules (minus dri) + libvulkan*/NVK
    // Vulkan ICD manifests (usr/share/vulkan/icd.d/nouveau_icd.*.json). The
    // loader (`libvulkan.so.1`, from usr/lib above) finds NVK ONLY through this
    // JSON; without it in the live root, NVK is invisible on the ISO even though
    // its .so is present -- and Zink (GL-on-Vulkan) then has no Vulkan to run on.
    "usr/share/vulkan",
    "usr/libexec",     // Xorg.wrap on some layouts
    "usr/share/X11",   // xkb data, xorg.conf.d defaults, rgb.txt
    "usr/share/fonts", // base bitmap fonts X refuses to start without
    "usr/share/fontconfig",
    // libinput's device-quirks database. Without it X logs "failed to find
    // data files" and libinput falls back to degraded device behavior.
    "usr/share/libinput",
    "etc/fonts",
    "etc/libinput", // local-overrides.quirks (if present)
    // ── XFCE4 in QEMU ───────────────────────────────────────────────────────
    // The XFCE/GTK data the session reads at runtime. usr/bin and usr/lib
    // above already carry the binaries and libraries (gdk-pixbuf loaders, GTK
    // modules); these are the /usr/share + /etc trees that make them work.
    "usr/share/xfce4",
    "usr/share/xfwm4",
    "usr/share/themes",
    "usr/share/icons",
    "usr/share/glib-2.0", // GSettings schemas (compiled at first boot)
    // glycin's conf.d: one .conf per loader, mapping a mime type to the
    // loader binary under usr/libexec (which `usr/libexec` above already
    // brings). Without these glycin has NO loader registry, so a present
    // loader binary is never invoked and every decode fails exactly as if it
    // were absent. The booted image showed precisely that: a glycin-svg
    // binary in usr/libexec and `glycin-conf=0`.
    "usr/share/glycin-loaders",
    "usr/share/thumbnailers",
    "usr/share/dbus-1",
    "usr/share/mime",
    "usr/share/applications",
    "usr/share/desktop-directories",
    "etc/xdg",
    "etc/dbus-1",
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
    // `live_enabled()` (ECLIPSE_XORG_LIVE) is the master switch for staging the
    // desktop into the RAM live root; the Xorg-specific `enabled()` only gates
    // the Xorg half below, so a labwc-only build (ECLIPSE_XORG=0) still gets its
    // compositor copied in.
    if !live_enabled() {
        return;
    }
    // Copy the desktop stack into the live root when EITHER the Xorg server OR
    // the labwc/Wayland compositor is installed in the full rootfs. Both are
    // desktop binaries under usr/bin + usr/lib (+ the glibc loader trees) that
    // LIVE_KEEP deliberately omits, so the QEMU live boot cannot run the desktop
    // without them.
    //
    // labwc previously reached the live initramfs ONLY as a side effect of this
    // Xorg copy sweeping all of usr/bin+usr/lib -- so a labwc-only build (or one
    // where the Xorg apk failed) booted with the compositor binary and
    // libwlroots ABSENT: the /usr/local/bin/labwc wrapper is kept (usr/local/bin
    // is in LIVE_KEEP) but finds no /usr/bin/labwc, prints "real binary not
    // found (apk add labwc)" and exits 127, and eclipse-init just respawns it
    // forever. That is a black screen with the compositor never actually there.
    let have_xorg = enabled()
        && ["usr/bin/Xorg", "usr/bin/X", "usr/lib/xorg"]
            .iter()
            .any(|p| full.join(p).exists());
    let have_labwc = full.join("usr/bin/labwc").exists();
    if !have_xorg && !have_labwc {
        return;
    }

    // INCLUDE mesa's DRI drivers now (previously excluded). The old rationale
    // ("too heavy for the RAM initramfs") was wrong: the heavy libraries
    // (libgallium-*.so, libLLVM-*.so) live in `usr/lib` and are ALREADY copied
    // uncapped by the `usr/lib` tree above — the `usr/lib/dri` entries are just
    // symlinks into them (Mesa 26.x megadriver). Excluding them saved nothing
    // and only broke GL: labwc logged "virtio_gpu: driver missing" / "DRI2:
    // failed to create screen" and fell back to a non-working kms_swrast, so
    // the desktop could not render on the GL path at all. Keeping them lets
    // Mesa load virtio_gpu_dri.so (QEMU + virgl) and nouveau_dri.so (real
    // hardware). `skip` is pointed at a sentinel that matches no real entry.
    let skip: PathBuf = full.join("usr/lib/__eclipse_include_all__");

    println!(
        "Desktop stack: copying into live/QEMU initramfs (xorg={have_xorg} labwc={have_labwc}, including DRI drivers for GL) ..."
    );
    for rel in LIVE_TREES {
        copy_uncapped(&full.join(rel), &live.join(rel), &skip);
    }
    let mib = tree_size(&live.join("usr")) / (1024 * 1024);
    println!("Xorg stack: live root usr/ is now ~{mib} MiB");

    // Inventory the LIVE root, not just the rootfs. The two have diverged:
    // the rootfs reports `icon themes: Adwaita hicolor` while the booted
    // guest reports one theme, which means packaging fixes have been landing
    // in a tree the RAM image never carries. Printing both sides says which
    // of the two is wrong without another boot.
    let themes = match std::fs::read_dir(live.join("usr/share/icons")) {
        Ok(rd) => {
            let mut v: Vec<String> = rd
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            if v.is_empty() {
                "<empty>".to_string()
            } else {
                v.join(" ")
            }
        }
        Err(_) => "<missing>".to_string(),
    };
    let fallback = live
        .join("usr/share/icons/hicolor/48x48/apps/application-x-executable.png")
        .is_file();
    println!("Xorg stack: LIVE root icon themes: {themes}");
    println!(
        "Xorg stack: LIVE root fallback PNG: {}",
        if fallback { "present" } else { "MISSING" }
    );
}
