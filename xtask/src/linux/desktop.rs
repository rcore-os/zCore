//! Eclipse OS desktop for labwc — everything the compositor session needs to
//! look like a real desktop instead of a bare black modeset buffer:
//!
//! - a night-sky wallpaper with the Eclipse logo, rendered at build time by
//!   [`wallpaper`] (pure Rust, no image crates) into
//!   `/usr/share/backgrounds/eclipse/eclipse-night.png`;
//! - an openbox-style dark theme (`Eclipse-Dark`) under `/usr/share/themes`
//!   that labwc picks up for titlebars, menus and OSDs;
//! - labwc config in `/root/.config/labwc/`: `rc.xml` (theme + keybinds),
//!   `menu.xml` (right-click desktop menu), `environment` (cursor/GTK vars)
//!   and `autostart` (wallpaper, panel, first terminal — each guarded so a
//!   missing client never aborts the session);
//! - lunarbg (procedural wallpaper) and lunarbar (two-bar panel), Eclipse's
//!   own native Wayland clients, launched from `autostart`;
//! - dark-mode defaults for GTK 3/4 and a matching `foot` terminal palette;
//! - the bulletproof `/usr/local/bin/labwc` wrapper that pins the pixman
//!   renderer environment (login(1) strips arbitrary env vars, so a wrapper
//!   is the only delivery that survives every launch path).
//!
//! Runtime packages:
//! `apk add labwc foot xkeyboard-config font-dejavu adwaita-icon-theme`
//! (lunarbg/lunarbar are Eclipse's own binaries; swaybg/waybar are no longer
//! used — the native clients replaced them).
//!
//! `xkeyboard-config` is REQUIRED, not optional: it ships the XKB data under
//! `/usr/share/X11/xkb`. Without it labwc still starts but cannot compile a
//! keymap, so it hands clients an empty one — and foot (and any xkbcommon
//! client) then SEGFAULTs parsing it ("[XKB-822] Failed to parse input xkb
//! string", terminal exits rc=139). lunarbg/lunarbar survive only because they
//! never parse the keymap. On the musl/Alpine install this package is not
//! pulled in automatically by foot or labwc, so it must be named explicitly.

use std::{fs, path::Path};

/// Install the whole desktop into the rootfs. Called from the rootfs build;
/// everything is plain file writes, so it is safe on incremental rebuilds.
pub fn install(rootfs: &Path) {
    write_wallpaper(rootfs);
    write_theme(rootfs);
    write_labwc_rc(rootfs);
    write_labwc_menu(rootfs);
    write_labwc_environment(rootfs);
    write_labwc_autostart(rootfs);
    write_gtk_settings(rootfs);
    write_foot_config(rootfs);
    write_labwc_wrapper(rootfs);
    write_memwatch(rootfs);
    write_terminal_wrapper(rootfs);
    write_firefox_wrapper(rootfs);
    write_firefox_desktop_override(rootfs);
    write_xorg_config(rootfs);
    write_xfce_defaults(rootfs);
    write_fallback_icons(rootfs);
    write_x11_prepare(rootfs);
}

/// Eclipse's xfconf channel defaults, under `/etc/xdg` (the sysconfig layer
/// every fresh user profile inherits). The critical one: xfwm4's compositor
/// OFF — on the software ShadowFB every composited frame is a full-screen CPU
/// blit, which makes the whole desktop feel seconds behind. Also called from
/// `xorg::install` AFTER its `etc/xdg` staging merge, which would otherwise
/// clobber these with Alpine's stock defaults (compositing on).
pub(super) fn write_xfce_defaults(rootfs: &Path) {
    let chan = rootfs.join("etc/xdg/xfce4/xfconf/xfce-perchannel-xml");
    let _ = fs::create_dir_all(&chan);
    fs::write(
        chan.join("xfwm4.xml"),
        b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
          <!-- Eclipse OS defaults: no compositing on the software framebuffer. -->\n\
          <channel name=\"xfwm4\" version=\"1.0\">\n\
          \x20 <property name=\"general\" type=\"empty\">\n\
          \x20   <property name=\"use_compositing\" type=\"bool\" value=\"false\"/>\n\
          \x20   <property name=\"theme\" type=\"string\" value=\"Default\"/>\n\
          \x20 </property>\n\
          </channel>\n",
    )
    .unwrap();
}

/// `/usr/local/bin/eclipse-x11-prepare`: first-boot generation of the caches
/// the build-time `apk --no-scripts` install skipped (post-install triggers
/// never ran, and on the RAM-resident live root nothing persists anyway).
/// Without these GTK is visibly broken: no gdk-pixbuf loaders.cache means every
/// icon/image fails to decode, and missing gschemas.compiled aborts any app
/// that touches GSettings. Everything is guarded (`command -v`, only-if-stale)
/// and logged; the slow, non-critical caches (icons, mime) go to background.
/// A 48x48 opaque PNG, generated once and embedded.
///
/// NOTE: the claim this constant was added on -- "gdk-pixbuf decodes PNG with a
/// built-in loader, no module, no glycin, no sandbox" -- is FALSE for this
/// Alpine. See `write_fallback_icons` below: PNG goes through glycin here too,
/// which is why shipping these files changed nothing. They are kept only as
/// content for the icon names GTK looks up; they are not a decoding fallback.
const FALLBACK_ICON_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x30, 0x00, 0x00, 0x00, 0x30, 0x08, 0x06, 0x00, 0x00, 0x00, 0x57, 0x02, 0xf9,
    0x87, 0x00, 0x00, 0x00, 0x42, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0xed, 0xcf, 0x31, 0x09, 0x00,
    0x00, 0x08, 0x00, 0x30, 0xe3, 0x8b, 0xd8, 0x59, 0x2b, 0xf8, 0x0a, 0x3b, 0x16, 0x60, 0x91, 0xd5,
    0xf3, 0x59, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08,
    0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08,
    0x08, 0x08, 0x08, 0x08, 0x08, 0x5c, 0x2d, 0x3d, 0x1f, 0x86, 0x5a, 0xdf, 0x54, 0x1a, 0x86, 0x00,
    0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// Install PNG content for the icon names GTK and libwnck fall back to.
///
/// This does NOT fix the abort, and the reasoning it was added on was wrong.
/// The session log finally showed the assertion itself:
///
///   Wnck:ERROR:../libwnck/xutils.c:1510:default_icon_at_size:
///             assertion failed: (base)
///
/// and that line is
///
///   base = gdk_pixbuf_new_from_resource ("/org/gnome/libwnck/default_icon.png", NULL);
///   g_assert (base);
///
/// -- a PNG compiled into libwnck's own GResource. It is always present, no
/// icon theme and no file on disk is involved. The only way it decodes to NULL
/// is that gdk-pixbuf cannot decode PNG at all.
///
/// Which is exactly the case here. Alpine builds gdk-pixbuf 2.44 with
///
///   -Dpng=disabled -Djpeg=disabled -Dgif=disabled -Dtiff=disabled
///   -Dothers=disabled -Dlegacy_xpm=enabled -Dglycin=enabled
///
/// so the ONLY native loader is XPM (which is why the inventory kept reporting
/// exactly one `libpixbufloader-*.so`), and every other format is handed to an
/// out-of-process glycin loader. The image had none installed
/// (`glycin loaders: <missing>`), so nothing here could decode PNG or SVG.
///
/// These files are kept because the icon names still need content once
/// decoding works, but they were never a decoding fallback.
pub(super) fn write_fallback_icons(rootfs: &Path) {
    // The names a GTK3 lookup ends up at when nothing else matches.
    const NAMES: &[&str] = &[
        "application-x-executable",
        "image-missing",
        "gtk-missing-image",
        "application-default-icon",
    ];
    for size in ["48x48", "32x32", "24x24", "16x16"] {
        let dir = rootfs
            .join("usr/share/icons/hicolor")
            .join(size)
            .join("apps");
        if fs::create_dir_all(&dir).is_err() {
            continue;
        }
        for name in NAMES {
            let dst = dir.join(format!("{name}.png"));
            // Never shadow a real icon that the theme already provides.
            if !dst.exists() {
                let _ = fs::write(&dst, FALLBACK_ICON_PNG);
            }
        }
    }
    println!("Desktop: installed PNG content for fallback icon names under hicolor");
}

fn write_x11_prepare(rootfs: &Path) {
    let localbin = rootfs.join("usr/local/bin");
    let _ = fs::create_dir_all(&localbin);
    let prep = localbin.join("eclipse-x11-prepare");
    fs::write(
        &prep,
        b"#!/bin/sh\n\
          # Eclipse OS: regenerate the package caches `apk --no-scripts` skipped.\n\
          # Cheap when already present; see write_x11_prepare in xtask.\n\
          LOG=/var/log/eclipse-x11-prepare.log\n\
          {\n\
          echo \"[prepare] start\"\n\
          # D-Bus machine id (dbus refuses to start without one).\n\
          if [ ! -s /etc/machine-id ] && command -v dbus-uuidgen >/dev/null 2>&1; then\n\
          \x20 dbus-uuidgen > /etc/machine-id 2>/dev/null\n\
          fi\n\
          mkdir -p /var/lib/dbus /run/dbus\n\
          [ -s /var/lib/dbus/machine-id ] || cp /etc/machine-id /var/lib/dbus/machine-id 2>/dev/null\n\
          # Self-heal a missing SVG pixbuf loader.\n\
          #\n\
          # adwaita-icon-theme is SVG and gdk-pixbuf has no built-in SVG\n\
          # loader, so without librsvg every icon lookup returns NULL --\n\
          # and libwnck g_asserts on a NULL pixbuf instead of degrading,\n\
          # which kills xfce4-session and the whole X session with it.\n\
          # librsvg is in DEFAULT_PACKAGES, but three builds shipped\n\
          # without it (the image had exactly one loader,\n\
          # libpixbufloader-xpm.so), because a single unresolvable name\n\
          # aborts the whole apk transaction at image-build time.\n\
          #\n\
          # If the loader is missing and the machine has a default route,\n\
          # fetch it here rather than losing the session to an assert.\n\
          # Guarded on the route so an offline boot fails fast instead of\n\
          # making the user wait on apk retries.\n\
          # Bring the network up if nothing has. The live/QEMU boot drops to a\n\
          # shell with no networking service, so there is no address and no\n\
          # default route when the session starts -- which the SVG self-heal\n\
          # below then correctly reports as \"no network to fetch one\". A\n\
          # desktop wants the network anyway. Bounded retries so an isolated\n\
          # machine costs a few seconds, not a stall.\n\
          if ! ip route 2>/dev/null | grep -q default; then\n\
          \x20 if command -v udhcpc >/dev/null 2>&1; then\n\
          \x20   echo '[prepare] no default route; running udhcpc'\n\
          \x20   for i in $(ip -o link show 2>/dev/null | sed 's/^[0-9]*: //; s/[@:].*//' | grep -v '^lo'); do\n\
          \x20     udhcpc -i \"$i\" -n -q -t 3 -T 3 2>&1 | tail -2\n\
          \x20     ip route 2>/dev/null | grep -q default && break\n\
          \x20   done\n\
          \x20 fi\n\
          fi\n\
          # SVG support is EITHER a classic gdk-pixbuf loader module OR a\n\
          # glycin loader: Alpine moved image decoding out of\n\
          # `libpixbufloader-*.so` and into glycin (gdk-pixbuf 2.44 pulls in\n\
          # libglycin + glycin-svg, and `librsvg` becomes a library behind it\n\
          # rather than a pixbuf module). Testing only for the .so said\n\
          # \"missing\" on an image that already had librsvg installed.\n\
          if ! ls /usr/lib/gdk-pixbuf-2.0/*/loaders/*svg*.so >/dev/null 2>&1 \\\n\
          \x20  && ! ls -d /usr/libexec/glycin-loaders/*/*svg* >/dev/null 2>&1 \\\n\
          \x20  && ! ls -d /usr/lib/glycin-loaders/*/*svg* >/dev/null 2>&1; then\n\
          \x20 if command -v apk >/dev/null 2>&1 && ip route 2>/dev/null | grep -q default; then\n\
          \x20   echo '[prepare] no SVG pixbuf loader; fetching librsvg'\n\
          \x20   apk add librsvg adwaita-icon-theme shared-mime-info > /tmp/apk-librsvg.out 2>&1\n\
          \x20   rc=$?\n\
          \x20   rm -f /usr/lib/gdk-pixbuf-2.0/*/loaders.cache\n\
          \x20   tail -5 /tmp/apk-librsvg.out\n\
          \x20   # To the CONSOLE, not just the log: this verdict is the one\n\
          \x20   # thing nobody can see from a `startx` paste, and it is the\n\
          \x20   # difference between \"the package name is wrong\" and \"there\n\
          \x20   # was no network yet\".\n\
          \x20   echo \"[eclipse-x11] apk add librsvg rc=$rc: $(tail -1 /tmp/apk-librsvg.out)\" > /dev/console 2>/dev/null\n\
          \x20 else\n\
          \x20   echo '[prepare] no SVG pixbuf loader and no network to fetch one'\n\
          \x20   echo \"[eclipse-x11] SVG loader missing; apk=$(command -v apk >/dev/null 2>&1 && echo yes || echo no) default-route=$(ip route 2>/dev/null | grep -q default && echo yes || echo no)\" > /dev/console 2>/dev/null\n\
          \x20 fi\n\
          fi\n\
          # gdk-pixbuf loader cache: every GTK icon/image decode needs it.\n\
          #\n\
          # Regenerated UNCONDITIONALLY, not just when absent. A cache that\n\
          # exists is not a cache that is right: install a loader afterwards\n\
          # (`apk add librsvg`) and the stale cache still lists only what was\n\
          # there before, so the new loader is never registered and icons keep\n\
          # failing exactly as if it had not been installed. Rebuilding it is a\n\
          # scan of a handful of .so files -- milliseconds -- unlike the icon\n\
          # and mime caches below, which is why only those two are opt-in.\n\
          if command -v gdk-pixbuf-query-loaders >/dev/null 2>&1; then\n\
          \x20 echo '[prepare] gdk-pixbuf loaders.cache'\n\
          \x20 gdk-pixbuf-query-loaders --update-cache\n\
          fi\n\
          # GSettings schemas: apps abort on a missing compiled schema they use.\n\
          if command -v glib-compile-schemas >/dev/null 2>&1 \\\n\
          \x20  && [ -d /usr/share/glib-2.0/schemas ] \\\n\
          \x20  && [ ! -s /usr/share/glib-2.0/schemas/gschemas.compiled ]; then\n\
          \x20 echo '[prepare] gschemas.compiled'\n\
          \x20 glib-compile-schemas /usr/share/glib-2.0/schemas\n\
          fi\n\
          # Icon-theme and mime caches: slow but run SYNCHRONOUSLY on purpose.\n\
          # Backgrounding them ('&') leaves orphans behind when this script and\n\
          # its shell exit, and a backgrounded update-mime-database was the\n\
          # process live when the kernel panicked in Process::linux() at X\n\
          # session teardown. Until that kernel bug is understood, do not\n\
          # create orphaned background children here. Both caches are optional\n\
          # (lookups work without them, just slower), so they are OPT-IN:\n\
          # set ECLIPSE_X11_CACHES=1 to build them.\n\
          #\n\
          # They default to OFF because running them inline delays the session:\n\
          # .xinitrc calls this script BEFORE startxfce4, so update-mime-database\n\
          # over /usr/share/mime (minutes under emulation) leaves the user staring\n\
          # at a black root window with no window manager and no cursor. Making\n\
          # them synchronous was the right call -- backgrounding them orphaned\n\
          # children and a backgrounded update-mime-database was live when the\n\
          # kernel panicked -- but synchronous AND on the critical path is not.\n\
          # The two caches that actually matter (gdk-pixbuf loaders.cache and\n\
          # gschemas.compiled, without which GTK renders no images and aborts on\n\
          # GSettings) are cheap and stay unconditional above.\n\
          if [ \"${ECLIPSE_X11_CACHES:-0}\" = \"1\" ]; then\n\
          \x20 if command -v gtk-update-icon-cache >/dev/null 2>&1; then\n\
          \x20   for t in /usr/share/icons/*/; do\n\
          \x20     if [ -f \"$t/index.theme\" ] && [ ! -s \"$t/icon-theme.cache\" ]; then\n\
          \x20       echo \"[prepare] icon cache $t\"\n\
          \x20       gtk-update-icon-cache -q -f \"$t\"\n\
          \x20     fi\n\
          \x20   done\n\
          \x20 fi\n\
          \x20 if command -v update-mime-database >/dev/null 2>&1 \\\n\
          \x20    && [ -d /usr/share/mime/packages ] && [ ! -s /usr/share/mime/mime.cache ]; then\n\
          \x20   echo '[prepare] mime cache'\n\
          \x20   update-mime-database /usr/share/mime\n\
          \x20 fi\n\
          fi\n\
          echo \"[prepare] done\"\n\
          } >>\"$LOG\" 2>&1\n\
          # One line to the CONSOLE, not just the log. xfce4-session dies on a\n\
          # NULL icon (libwnck g_asserts in default_icon_at_size rather than\n\
          # degrading), and whether that is a missing SVG loader, a missing\n\
          # loaders.cache or a missing theme is decided by exactly these four\n\
          # numbers -- which were being asked for by hand, one boot at a time,\n\
          # while the console paste that everyone actually reads showed none of\n\
          # them. Cheap, and it makes every future `startx` self-diagnosing.\n\
          nload=$(ls /usr/lib/gdk-pixbuf-2.0/*/loaders/*.so 2>/dev/null | wc -l)\n\
          svg=no\n\
          ls /usr/lib/gdk-pixbuf-2.0/*/loaders/*svg*.so >/dev/null 2>&1 && svg=pixbuf\n\
          ls -d /usr/libexec/glycin-loaders/*/*svg* >/dev/null 2>&1 && svg=glycin\n\
          ls -d /usr/lib/glycin-loaders/*/*svg* >/dev/null 2>&1 && svg=glycin\n\
          # Which glycin loaders exist, and their conf.d entries: gdk-pixbuf\n\
          # 2.44 in Alpine is built -Dpng/jpeg/gif/tiff/others=disabled\n\
          # -Dglycin=enabled, so EVERY format except legacy XPM is decoded by\n\
          # an out-of-process glycin loader. No loader binary == no decoding.\n\
          gly=$(ls /usr/libexec/glycin-loaders/*/* 2>/dev/null | sed 's|.*/||' | tr '\\n' ',')\n\
          [ -n \"$gly\" ] || gly=none\n\
          glyconf=$(ls /usr/share/glycin-loaders/*/conf.d/*.conf 2>/dev/null | wc -l)\n\
          # bwrap's REAL first error line, not just pass/fail: glycin only\n\
          # falls back to running loaders unsandboxed when bwrap's stderr\n\
          # matches one of its known \"namespaces are blocked\" strings\n\
          # (\"Creating new namespace failed\", \"setting up uid map: Permission\n\
          # denied\", ...) or when bwrap dies with SIGSYS. Any OTHER failure is\n\
          # read as \"the sandbox works\", and glycin then tries to use it and\n\
          # every decode fails. So the exact wording decides the fix.\n\
          bwrap=absent\n\
          if command -v bwrap >/dev/null 2>&1; then\n\
          \x20 bwrap --unshare-all --die-with-parent --chdir / --ro-bind /usr /usr \\\n\
          \x20   --dev /dev --ro-bind /lib /lib /usr/bin/true >/dev/null 2>/tmp/bwrap.err\n\
          \x20 brc=$?\n\
          \x20 if [ \"$brc\" -eq 0 ]; then\n\
          \x20   bwrap=works\n\
          \x20 else\n\
          \x20   bwrap=\"rc=$brc:$(head -1 /tmp/bwrap.err 2>/dev/null)\"\n\
          \x20 fi\n\
          fi\n\
          # The one test that actually answers \"can this image decode a PNG?\".\n\
          # Everything else is a proxy for it, and every proxy so far has been\n\
          # wrong. libwnck aborts on exactly this: a PNG embedded in its own\n\
          # GResource that gdk_pixbuf_new_from_resource() cannot decode.\n\
          png=no-tool\n\
          PROBE_IMG=/usr/share/icons/hicolor/48x48/apps/application-x-executable.png\n\
          # The two thumbnailers take DIFFERENT arguments, and getting it wrong\n\
          # reads as a decode failure. gdk-pixbuf-thumbnailer takes two plain\n\
          # paths; glycin-thumbnailer is a GApplication that wants a URI and\n\
          # named options (-i/-o/-s), which is why passing it a path answered\n\
          # `Error: Input URI not supplied.` and told us nothing about decoding.\n\
          for t in gdk-pixbuf-thumbnailer glycin-thumbnailer; do\n\
          \x20 command -v \"$t\" >/dev/null 2>&1 || continue\n\
          \x20 rm -f /tmp/png-probe.png\n\
          \x20 if [ \"$t\" = glycin-thumbnailer ]; then\n\
          \x20   \"$t\" -i \"file://$PROBE_IMG\" -o /tmp/png-probe.png -s 48 \\\n\
          \x20     >/tmp/png-probe.err 2>&1\n\
          \x20 else\n\
          \x20   \"$t\" \"$PROBE_IMG\" /tmp/png-probe.png >/tmp/png-probe.err 2>&1\n\
          \x20 fi\n\
          \x20 if [ -s /tmp/png-probe.png ]; then\n\
          \x20   png=\"ok($t)\"\n\
          \x20 else\n\
          \x20   png=\"FAIL($t):$(head -1 /tmp/png-probe.err 2>/dev/null)\"\n\
          \x20 fi\n\
          \x20 break\n\
          done\n\
          # Which decode tools exist at all -- the first run of this probe said\n\
          # `no-tool`, and guessing which binary Alpine ships is how the last\n\
          # four rounds were lost.\n\
          pixtools=$(ls /usr/bin/*pixbuf* /usr/bin/*glycin* 2>/dev/null | sed 's|.*/||' | tr '\\n' ',')\n\
          [ -n \"$pixtools\" ] || pixtools=none\n\
          cache=missing\n\
          for c in /usr/lib/gdk-pixbuf-2.0/*/loaders.cache; do\n\
          \x20 [ -s \"$c\" ] && cache=ok\n\
          done\n\
          nicon=$(ls /usr/share/icons 2>/dev/null | tr '\\n' ',')\n\
          # Did the PNG fallback this build writes actually ARRIVE? The build\n\
          # reports the rootfs has both Adwaita and hicolor, the guest reports\n\
          # one icon theme -- so what the build stages and what the RAM image\n\
          # boots have diverged, and every packaging fix so far may have been\n\
          # landing in a tree the guest never sees. This tests one exact file.\n\
          fb=no\n\
          [ -f /usr/share/icons/hicolor/48x48/apps/application-x-executable.png ] && fb=yes\n\
          sch=missing\n\
          [ -s /usr/share/glib-2.0/schemas/gschemas.compiled ] && sch=ok\n\
          echo \"[eclipse-x11] pixbuf-loaders=$nload svg=$svg loaders.cache=$cache icon-themes=$nicon fallback-png=$fb gschemas=$sch\" > /dev/console 2>/dev/null\n\
          echo \"[eclipse-x11] png-decode=$png tools=$pixtools glycin-loaders=$gly glycin-conf=$glyconf\" > /dev/console 2>/dev/null\n\
          echo \"[eclipse-x11] bwrap=$bwrap\" > /dev/console 2>/dev/null\n\
          exit 0\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&prep, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// `/usr/local/bin/eclipse-terminal`: launch the first terminal that exists.
/// foot is preferred (pixman/shm, matches this stack); alacritty is the
/// fallback, forced onto software GL (client-side llvmpipe renders via shm,
/// which does not touch the DRM GL path that hangs this box). Keybinds, the
/// desktop menu, the panel launcher and autostart all go through this, so
/// "a terminal" keeps working no matter which one is installed.
/// `eclipse-memwatch [interval]` — one compact `/proc/memhogs` line per tick,
/// to the console.
///
/// The end-of-session dump only fires if the session exits. The runs that
/// matter do not: they exhaust physical memory and limp on, so every report so
/// far has been a wall of `frame_alloc FAILED` with no record of what climbed
/// to get there. A snapshot cannot tell a leak from a working set; the RAMP
/// can, and whichever bucket grows names the culprit -- `contig` is the DRM
/// buffer pool, `pgsrc` is file mappings, `paged` with no process behind it is
/// a plain VMO leak, and `unattr` is outside the VMO system entirely (kernel
/// heap or ramfs).
fn write_memwatch(rootfs: &Path) {
    let localbin = rootfs.join("usr/local/bin");
    let _ = fs::create_dir_all(&localbin);
    let script = localbin.join("eclipse-memwatch");
    fs::write(
        &script,
        b"#!/bin/sh\n\
          # Eclipse OS: periodic one-line memory summary on the console.\n\
          # Field numbers follow /proc/memhogs' layout; awk splits on runs of\n\
          # whitespace, so the report's column padding does not matter.\n\
          IVAL=${1:-5}\n\
          [ -r /proc/memhogs ] || exit 0\n\
          T=0\n\
          while :; do\n\
          \x20 awk -v t=\"$T\" '\n\
          \x20   /^physical:/     {used=$2; tot=$6}\n\
          \x20   /^processes:/    {np=$2; priv=$4}\n\
          \x20   /^  Paged /      {pg=$2; pgm=$4}\n\
          \x20   /^  PagedSource/ {ps=$2; psm=$4}\n\
          \x20   /^  Contiguous/  {ct=$2; ctm=$4}\n\
          \x20   /^  Child/       {ch=$2; chm=$4}\n\
          \x20   /^shared-vmo registry:/ {reg=$3}\n\
          \x20   /^unattributed:/ {un=$2}\n\
          \x20   END {printf \"[eclipse-mem] t=%ss used=%s/%s proc=%s/%sMiB paged=%s/%sMiB pgsrc=%s/%sMiB contig=%s/%sMiB child=%s/%sMiB shvmo=%s unattr=%sMiB\\n\", t,used,tot,np,priv,pg,pgm,ps,psm,ct,ctm,ch,chm,reg,un}\n\
          \x20 ' /proc/memhogs > /dev/console 2>/dev/null\n\
          \x20 T=$((T+IVAL))\n\
          \x20 sleep \"$IVAL\"\n\
          done\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn write_terminal_wrapper(rootfs: &Path) {
    let localbin = rootfs.join("usr/local/bin");
    let _ = fs::create_dir_all(&localbin);
    let wrapper = localbin.join("eclipse-terminal");
    fs::write(
        &wrapper,
        b"#!/bin/sh\n\
          # Eclipse OS: launch whichever terminal is installed.\n\
          # Every desktop path (labwc keybinds/menu, lunarbar's launcher,\n\
          # autostart) goes through this wrapper, so when \"the terminal does\n\
          # not open\" THIS is the place that must explain why: every attempt\n\
          # is appended to $HOME/.eclipse-terminal.log with the terminal's\n\
          # stderr and exit code.\n\
          export LANG=\"${LANG:-C.UTF-8}\"\n\
          # foot refuses to start without a UTF-8 locale; make sure a broken\n\
          # profile can never hand us a non-UTF-8 LANG.\n\
          case \"$LANG\" in *UTF-8|*utf8|*UTF8) ;; *) LANG=C.UTF-8 ;; esac\n\
          TLOG=\"${HOME:-/root}/.eclipse-terminal.log\"\n\
          if command -v foot >/dev/null 2>&1; then\n\
          \x20 echo \"[$(date '+%H:%M:%S')] foot $* (LANG=$LANG WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-UNSET} XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-UNSET})\" >>\"$TLOG\"\n\
          \x20 t0=$(date +%s)\n\
          \x20 foot \"$@\" 2>>\"$TLOG\"\n\
          \x20 rc=$?\n\
          \x20 t1=$(date +%s)\n\
          # foot propagates the shell's exit code: a nonzero rc after a long\n\
          # session is a normal close, not a startup failure. Only fall back\n\
          # to alacritty when foot died within 5 seconds of launch.\n\
          \x20 [ \"$rc\" -eq 0 ] && exit 0\n\
          \x20 [ $((t1 - t0)) -ge 5 ] && exit \"$rc\"\n\
          \x20 echo \"[eclipse-terminal] foot died at startup rc=$rc; trying alacritty\" >>\"$TLOG\"\n\
          fi\n\
          if command -v alacritty >/dev/null 2>&1; then\n\
          \x20 echo \"[$(date '+%H:%M:%S')] alacritty $*\" >>\"$TLOG\"\n\
          \x20 LIBGL_ALWAYS_SOFTWARE=1 alacritty \"$@\" 2>>\"$TLOG\"\n\
          \x20 rc=$?\n\
          \x20 [ \"$rc\" -eq 0 ] && exit 0\n\
          \x20 echo \"[eclipse-terminal] alacritty exited rc=$rc\" >>\"$TLOG\"\n\
          \x20 exit \"$rc\"\n\
          fi\n\
          echo 'eclipse-terminal: no terminal found (apk add foot)' >&2\n\
          echo 'eclipse-terminal: no terminal found (apk add foot)' >>\"$TLOG\"\n\
          exit 127\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// `/usr/local/bin/eclipse-firefox`: launch Firefox in the only configuration
/// that renders on this GPU-less stack — Wayland, single-process, pure software
/// WebRender (SWGL). The labwc menu, the `firefox.desktop` the panel launcher
/// scans, and any `.desktop` activation all go through this wrapper, so Firefox
/// always starts the same way regardless of launch path; every attempt is
/// logged to `$HOME/.eclipse-firefox.log` so "the browser does not open" is
/// diagnosable without a reboot.
///
/// Crucially this does NOT set `MOZ_WEBRENDER=0`: modern Firefox has no
/// non-WebRender compositor, so that would disable rendering outright (a black
/// window) and, worse, override any `gfx.webrender.software=true` profile pref.
/// `MOZ_WEBRENDER_SOFTWARE=1` keeps WebRender on but forces its software backend.
fn write_firefox_wrapper(rootfs: &Path) {
    let localbin = rootfs.join("usr/local/bin");
    let _ = fs::create_dir_all(&localbin);
    let wrapper = localbin.join("eclipse-firefox");
    fs::write(
        &wrapper,
        b"#!/bin/sh\n\
          # Eclipse OS: launch Firefox in a GPU-less, single-process, software\n\
          # WebRender configuration. See write_firefox_wrapper in\n\
          # xtask/src/linux/desktop.rs for the rationale.\n\
          export HOME=\"${HOME:-/root}\"\n\
          export LANG=\"${LANG:-C.UTF-8}\"\n\
          case \"$LANG\" in *UTF-8|*utf8|*UTF8) ;; *) LANG=C.UTF-8 ;; esac\n\
          export MOZ_ENABLE_WAYLAND=1\n\
          # Single process: no e10s content children, hence no seccomp/namespace\n\
          # sandbox and no cross-process IPC to exercise on this kernel.\n\
          export MOZ_FORCE_DISABLE_E10S=1\n\
          export MOZ_DISABLE_CONTENT_SANDBOX=1\n\
          export MOZ_DISABLE_GMP_SANDBOX=1\n\
          export MOZ_DISABLE_RDD_SANDBOX=1\n\
          export MOZ_DISABLE_GPU_SANDBOX=1\n\
          export MOZ_DISABLE_SOCKET_PROCESS_SANDBOX=1\n\
          # Software rendering. NOT MOZ_WEBRENDER=0 (that disables rendering).\n\
          export LIBGL_ALWAYS_SOFTWARE=1\n\
          export MOZ_WEBRENDER_SOFTWARE=1\n\
          export MOZ_ACCELERATED=0\n\
          export MOZ_CRASHREPORTER_DISABLE=1\n\
          FLOG=\"${HOME:-/root}/.eclipse-firefox.log\"\n\
          if ! command -v firefox >/dev/null 2>&1; then\n\
          \x20 echo 'eclipse-firefox: firefox not found (apk add firefox-esr)' >&2\n\
          \x20 echo 'eclipse-firefox: firefox not found' >>\"$FLOG\"\n\
          \x20 exit 127\n\
          fi\n\
          echo \"[$(date '+%H:%M:%S')] firefox $* (WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-UNSET} XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-UNSET})\" >>\"$FLOG\"\n\
          exec firefox \"$@\" 2>>\"$FLOG\"\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// User-level `firefox.desktop` override so lunarbar's launcher menu (and any
/// XDG activation) runs Firefox through `eclipse-firefox` instead of the stock
/// package's `Exec=firefox`, which tries hardware GL/WebRender and paints black
/// on this GPU-less stack. lunarbar scans `$XDG_DATA_HOME/applications` before
/// `/usr/share/applications` and keeps the first entry per desktop-file id, so
/// this shadows the packaged `firefox.desktop` without touching it.
fn write_firefox_desktop_override(rootfs: &Path) {
    let dir = rootfs.join("root/.local/share/applications");
    let _ = fs::create_dir_all(&dir);
    fs::write(
        dir.join("firefox.desktop"),
        b"[Desktop Entry]\n\
          Version=1.0\n\
          Type=Application\n\
          Name=Firefox\n\
          GenericName=Web Browser\n\
          Comment=Browse the Web (Eclipse OS software-rendering wrapper)\n\
          Exec=/usr/local/bin/eclipse-firefox %u\n\
          TryExec=/usr/local/bin/eclipse-firefox\n\
          Terminal=false\n\
          Icon=firefox\n\
          Categories=Network;WebBrowser;\n\
          MimeType=text/html;x-scheme-handler/http;x-scheme-handler/https;\n\
          StartupNotify=false\n",
    )
    .unwrap();
}

/// Ship an Xorg config + `.xinitrc` so `startx` works out of the box.
///
/// Eclipse pins the **fbdev** driver on `/dev/fb0` rather than modesetting on
/// DRM. The scanout here is software either way — there is no real GPU
/// acceleration — so the DRM path only adds a dumb-buffer allocation and a KMS
/// atomic commit per frame on top of the same memcpy-to-framebuffer. Talking to
/// the linear framebuffer directly (the kernel's FbDev exposes the fbdev
/// ioctls + an mmap'able framebuffer VMO — linux-object/src/fs/devfs/fbdev.rs)
/// skips that round-trip, which is why Eclipse's X uses fbdev for speed. This
/// requires `xf86-video-fbdev` (installed from xorg.rs) and a `/dev/fb0` node,
/// which the kernel creates whenever a display driver is present.
///
/// The one thing autoconfig gets wrong is input. Xorg's udev backend DOES
/// enumerate every `/dev/input/event*` (it logs "No input driver specified,
/// ignoring this device" for each), so the devices are discovered — what's
/// missing is a driver + a rule that assigns it. The proven-good input path on
/// this kernel is **libinput**: labwc drives these exact event nodes through
/// libinput without trouble. So keep `AutoAddDevices` ON and ship a libinput
/// catch-all `InputClass` matching `/dev/input/event*` (the same shape distros
/// ship as `40-libinput.conf`). libinput then probes each device's real
/// capabilities via `EVIOCGBIT` and classifies keyboard vs pointer itself — no
/// need to hard-code which event node is the keyboard, which was the fragile
/// part of the old static evdev config. Requires `xf86-input-libinput`.
fn write_xorg_config(rootfs: &Path) {
    let confd = rootfs.join("etc/X11/xorg.conf.d");
    let _ = fs::create_dir_all(&confd);
    fs::write(
        confd.join("10-eclipse.conf"),
        b"# Eclipse OS: Xorg on the kernel framebuffer (/dev/fb0) via the fbdev\n\
          # driver -- NOT DRM/modesetting -- for speed: the scanout is software\n\
          # either way, so going straight to the linear framebuffer skips the\n\
          # DRM dumb-buffer + KMS commit per frame. Input auto-added, libinput.\n\
          Section \"ServerFlags\"\n\
          \x20   Option \"AutoAddDevices\" \"true\"\n\
          \x20   Option \"DontZap\"        \"false\"\n\
          # Do NOT touch DRM. Even with an explicit fbdev Device, modern Xorg\n\
          # platform-probes /dev/dri/card0 and tries to bind it as a GPU screen;\n\
          # on this kernel that probe hangs (Xorg.0.log stalls at 'Platform probe\n\
          # for /sys/class/drm/card0' and never brings the screen up, so startx\n\
          # times out and init respawns X every ~90s). Turning off GPU auto-add\n\
          # and auto-bind keeps X entirely on the fbdev/framebuffer path.\n\
          \x20   Option \"AutoAddGPU\"     \"false\"\n\
          \x20   Option \"AutoBindGPU\"    \"false\"\n\
          EndSection\n\
          \n\
          Section \"Device\"\n\
          \x20   Identifier \"fb\"\n\
          \x20   Driver     \"fbdev\"\n\
          \x20   Option     \"fbdev\"    \"/dev/fb0\"\n\
          \x20   Option     \"ShadowFB\" \"true\"\n\
          EndSection\n\
          \n\
          Section \"Screen\"\n\
          \x20   Identifier \"screen\"\n\
          \x20   Device     \"fb\"\n\
          EndSection\n\
          \n\
          # Assign libinput to every enumerated evdev node. No MatchIsKeyboard/\n\
          # MatchIsPointer here: those depend on udev ID_INPUT_* tags this kernel\n\
          # does not emit, so they would match nothing. Match purely on the device\n\
          # path and let libinput classify the device from its own capabilities.\n\
          Section \"InputClass\"\n\
          \x20   Identifier   \"eclipse-libinput\"\n\
          \x20   MatchDevicePath \"/dev/input/event*\"\n\
          \x20   Driver       \"libinput\"\n\
          EndSection\n\
          \n\
          Section \"ServerLayout\"\n\
          \x20   Identifier  \"layout\"\n\
          \x20   Screen      \"screen\"\n\
          EndSection\n",
    )
    .unwrap();

    // `.xinitrc`: a WM if one is installed, then a terminal, so `startx` yields
    // a usable screen instead of a bare X root. Everything is `command -v`
    // guarded and logged, and the server is kept alive at the end so a missing
    // terminal does not tear the session straight back down.
    let xinitrc = rootfs.join("root/.xinitrc");
    fs::write(
        &xinitrc,
        b"#!/bin/sh\n\
          # Eclipse OS default X session (fbdev on /dev/fb0 + software ShadowFB).\n\
          export LANG=\"${LANG:-C.UTF-8}\"\n\
          export LIBGL_ALWAYS_SOFTWARE=1\n\
          LOG=\"$HOME/.xinitrc.log\"; exec >\"$LOG\" 2>&1\n\
          echo \"[xinit] session start $(date 2>/dev/null || echo boot)\"\n\
          # Runtime dir: dbus/xfce bits expect one.\n\
          export XDG_RUNTIME_DIR=\"${XDG_RUNTIME_DIR:-/run/user/$(id -u 2>/dev/null || echo 0)}\"\n\
          mkdir -p \"$XDG_RUNTIME_DIR\" && chmod 700 \"$XDG_RUNTIME_DIR\"\n\
          [ -r \"$HOME/.Xresources\" ] && xrdb -merge \"$HOME/.Xresources\" 2>/dev/null\n\
          # Full XFCE4 session, if baked in (see xtask xorg.rs). Regenerate the\n\
          # GTK caches apk --no-scripts skipped, then hand the session to\n\
          # startxfce4 under a D-Bus session bus (xfce4-session aborts without\n\
          # one). exec: xfce IS the session; its exit ends X.\n\
          if command -v startxfce4 >/dev/null 2>&1; then\n\
          \x20 command -v eclipse-x11-prepare >/dev/null 2>&1 && eclipse-x11-prepare\n\
          \x20 echo \"[xinit] session=xfce4\"\n\
          \x20 # Session bus WITHOUT dbus-launch. dbus-launch asks dbus-daemon to\n\
          \x20 # report the bus address back through an inherited pipe fd\n\
          \x20 # (--print-address <fd>), and on this kernel that fd is unusable\n\
          \x20 # after the fork+exec: the daemon dies with \"Writing to pipe: Bad\n\
          \x20 # file descriptor\" and the session never starts. Running the\n\
          \x20 # daemon directly works (verified), so pick the socket path\n\
          \x20 # ourselves with --address and skip the address hand-back\n\
          \x20 # entirely -- no pipe, no exec, nothing to lose.\n\
          \x20 if [ -z \"$DBUS_SESSION_BUS_ADDRESS\" ] && command -v dbus-daemon >/dev/null 2>&1; then\n\
          \x20   BUS=\"$XDG_RUNTIME_DIR/bus\"\n\
          \x20   rm -f \"$BUS\"\n\
          \x20   if dbus-daemon --session --address=\"unix:path=$BUS\" --fork; then\n\
          \x20     DBUS_SESSION_BUS_ADDRESS=\"unix:path=$BUS\"; export DBUS_SESSION_BUS_ADDRESS\n\
          \x20     echo \"[xinit] dbus session bus: $DBUS_SESSION_BUS_ADDRESS\"\n\
          \x20   else\n\
          \x20     echo \"[xinit] dbus-daemon could not bind $BUS; falling back to dbus-launch\"\n\
          \x20     command -v dbus-launch >/dev/null 2>&1 \\\n\
          \x20       && exec dbus-launch --exit-with-session startxfce4\n\
          \x20   fi\n\
          \x20 fi\n\
          \x20 # NOT `exec`: when the session dies we want to say WHY. The\n\
          \x20 # reason xfce4-session aborts is printed on its stderr, which\n\
          \x20 # lands in $LOG -- and every report of this so far has been a\n\
          \x20 # console paste that does not include that file, so the one line\n\
          \x20 # that identifies the abort has never been visible. Run it,\n\
          \x20 # then copy the tail of the log to the console.\n\
          \x20 # Sample memory WHILE the session runs. The end-of-session dump\n\
          \x20 # below only fires if the session actually exits, and the runs\n\
          \x20 # that matter do not: they exhaust RAM and limp on, so every\n\
          \x20 # report so far has been a wall of `frame_alloc FAILED` with no\n\
          \x20 # idea of what climbed to get there. One compact line every 5s\n\
          \x20 # gives the RAMP, and the ramp is what names the culprit --\n\
          \x20 # whichever of paged / pgsrc / contig / child / unattr is the one\n\
          \x20 # that grows.\n\
          \x20 MEMWATCH=\n\
          \x20 if command -v eclipse-memwatch >/dev/null 2>&1; then\n\
          \x20   eclipse-memwatch 5 & MEMWATCH=$!\n\
          \x20 fi\n\
          \x20 startxfce4\n\
          \x20 rc=$?\n\
          \x20 [ -n \"$MEMWATCH\" ] && kill \"$MEMWATCH\" 2>/dev/null\n\
          \x20 echo \"[xinit] startxfce4 exited rc=$rc\"\n\
          \x20 {\n\
          \x20   echo \"[eclipse-x11] session ended rc=$rc; last 40 lines of $LOG:\"\n\
          \x20   tail -40 \"$LOG\"\n\
          \x20   echo \"[eclipse-x11] ---- end of session log ----\"\n\
          \x20   # Where the RAM went. The session has been running long enough\n\
          \x20   # to exhaust physical memory (\"frame_alloc FAILED: 2561 MiB\n\
          \x20   # used / 2561 MiB managed\"), and the split between per-process\n\
          \x20   # totals and the unattributed remainder says whether the\n\
          \x20   # desktop simply does not fit or the kernel is holding pages\n\
          \x20   # nobody owns. Those need opposite fixes, and nothing in a\n\
          \x20   # console paste has ever distinguished them.\n\
          \x20   echo \"[eclipse-x11] ---- /proc/memhogs ----\"\n\
          \x20   head -16 /proc/memhogs 2>/dev/null || echo \"(no /proc/memhogs: kernel predates it)\"\n\
          \x20   echo \"[eclipse-x11] ---- end of memhogs ----\"\n\
          \x20 } > /dev/console 2>/dev/null\n\
          \x20 exit $rc\n\
          fi\n\
          # A window manager, if one is installed (bare X still works without).\n\
          for wm in openbox twm jwm icewm; do\n\
          \x20 if command -v \"$wm\" >/dev/null 2>&1; then echo \"[xinit] wm=$wm\"; \"$wm\" & break; fi\n\
          done\n\
          # A terminal to interact with; the last one exec'd keeps the session.\n\
          for t in xterm st urxvt rxvt; do\n\
          \x20 if command -v \"$t\" >/dev/null 2>&1; then echo \"[xinit] term=$t\"; exec \"$t\"; fi\n\
          done\n\
          echo '[xinit] no X terminal found (apk add xterm); holding the server open'\n\
          exec sleep 2147483647\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&xinitrc, fs::Permissions::from_mode(0o755)).unwrap();
    }

    // `.xserverrc`: launch the X server with access control OFF (`-ac`). busybox
    // ships no `mcookie`, so `startx` cannot mint an MIT-MAGIC-COOKIE; it then
    // hands the server an empty/absent `-auth` file and the server rejects every
    // client ("Authorization required, but no authorization protocol specified"),
    // leaving startx spinning forever on "waiting for X server to begin accepting
    // connections" (and the churn eventually trips an SMP teardown race in the
    // kernel). `-ac` disables the auth check entirely -- fine for this
    // single-user VM -- so xterm/twm connect on the first try. startx appends the
    // display, its `-auth <file>` and the vt arg as "$@"; `-ac` overrides them.
    let xserverrc = rootfs.join("root/.xserverrc");
    fs::write(
        &xserverrc,
        b"#!/bin/sh\n\
          # Eclipse OS: X with access control disabled (see write_xorg_config).\n\
          for x in X Xorg; do\n\
          \x20 command -v \"$x\" >/dev/null 2>&1 && exec \"$x\" -ac -nolisten tcp \"$@\"\n\
          done\n\
          exec X -ac -nolisten tcp \"$@\"\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&xserverrc, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn write_wallpaper(rootfs: &Path) {
    let dir = rootfs.join("usr/share/backgrounds/eclipse");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("eclipse-night.png");
    // The render is deterministic, so skip the (CPU-heavy) redraw when the
    // file already exists — incremental rootfs rebuilds stay fast.
    if !path.exists() {
        let png = wallpaper::render_png(1600, 900);
        fs::write(&path, png).unwrap();
    }
}

/// Openbox-3 `themerc` consumed by labwc for window decorations, menus and
/// OSDs. Palette: deep purple night (matches the wallpaper), lavender text,
/// violet accents.
fn write_theme(rootfs: &Path) {
    let dir = rootfs.join("usr/share/themes/Eclipse-Dark/openbox-3");
    let _ = fs::create_dir_all(&dir);
    fs::write(
        dir.join("themerc"),
        b"# Eclipse OS - dark labwc/openbox theme.\n\
          border.width: 1\n\
          padding.width: 8\n\
          padding.height: 5\n\
          \n\
          window.active.border.color: #6b5aa8\n\
          window.inactive.border.color: #29233f\n\
          window.active.title.bg.color: #191430\n\
          window.inactive.title.bg.color: #131022\n\
          window.active.label.text.color: #e8e4f8\n\
          window.inactive.label.text.color: #837da0\n\
          window.active.button.unpressed.image.color: #e8e4f8\n\
          window.active.button.pressed.image.color: #9b8ae0\n\
          window.active.button.hover.image.color: #ffffff\n\
          window.inactive.button.unpressed.image.color: #837da0\n\
          \n\
          menu.title.bg.color: #191430\n\
          menu.title.text.color: #9b8ae0\n\
          menu.items.bg.color: #14101f\n\
          menu.items.text.color: #e8e4f8\n\
          menu.items.disabled.text.color: #5a5478\n\
          menu.items.active.bg.color: #6b5aa8\n\
          menu.items.active.text.color: #ffffff\n\
          menu.separator.color: #3a3357\n\
          menu.separator.padding.height: 4\n\
          \n\
          osd.bg.color: #191430\n\
          osd.border.color: #6b5aa8\n\
          osd.label.text.color: #e8e4f8\n",
    )
    .unwrap();
}

/// labwc main config: theme, fonts, four workspaces and enough keybinds to
/// drive the desktop from the keyboard (terminal, menu, tiling, workspaces).
fn write_labwc_rc(rootfs: &Path) {
    let cfg = rootfs.join("root/.config/labwc");
    let _ = fs::create_dir_all(&cfg);
    fs::write(
        cfg.join("rc.xml"),
        br#"<?xml version="1.0"?>
<labwc_config>
  <!-- xwaylandPersistence (labwc >= 0.7.3): start Xwayland WITH the
       compositor and keep it alive, instead of the default lazy on-demand
       spawn. On this stack the lazy spawn never finishes coming up while the
       first X11 client sits parked on labwc's socket forever (the vulkaninfo/
       glxgears infinite hang); eager startup either brings X11 up for real or
       fails visibly in /tmp/labwc.log at session start -- never by hanging a
       client. DISPLAY is not auto-pinned (see write_labwc_environment); X11
       apps need DISPLAY=:0 explicitly until Xwayland is verified healthy. -->
  <core><gap>0</gap><xwaylandPersistence>yes</xwaylandPersistence></core>
  <theme>
    <name>Eclipse-Dark</name>
    <cornerRadius>8</cornerRadius>
    <font place="ActiveWindow"><name>DejaVu Sans</name><size>10</size><weight>bold</weight></font>
    <font place="InactiveWindow"><name>DejaVu Sans</name><size>10</size></font>
    <font place="MenuItem"><name>DejaVu Sans</name><size>10</size></font>
    <font place="OnScreenDisplay"><name>DejaVu Sans</name><size>11</size></font>
  </theme>
  <desktops number="4"/>
  <keyboard>
    <!-- Terminals -->
    <keybind key="W-Return"><action name="Execute"><command>/usr/local/bin/eclipse-terminal</command></action></keybind>
    <keybind key="A-Return"><action name="Execute"><command>/usr/local/bin/eclipse-terminal</command></action></keybind>
    <!-- Desktop menu also on a key, in case the mouse is missing -->
    <keybind key="W-space"><action name="ShowMenu"><menu>root-menu</menu></action></keybind>
    <!-- Window management -->
    <keybind key="A-F4"><action name="Close"/></keybind>
    <keybind key="A-Tab"><action name="NextWindow"/></keybind>
    <keybind key="W-Up"><action name="ToggleMaximize"/></keybind>
    <keybind key="W-Left"><action name="SnapToEdge"><direction>left</direction></action></keybind>
    <keybind key="W-Right"><action name="SnapToEdge"><direction>right</direction></action></keybind>
    <!-- Workspaces -->
    <keybind key="W-1"><action name="GoToDesktop"><to>1</to></action></keybind>
    <keybind key="W-2"><action name="GoToDesktop"><to>2</to></action></keybind>
    <keybind key="W-3"><action name="GoToDesktop"><to>3</to></action></keybind>
    <keybind key="W-4"><action name="GoToDesktop"><to>4</to></action></keybind>
    <keybind key="W-S-1"><action name="SendToDesktop"><to>1</to></action></keybind>
    <keybind key="W-S-2"><action name="SendToDesktop"><to>2</to></action></keybind>
    <keybind key="W-S-3"><action name="SendToDesktop"><to>3</to></action></keybind>
    <keybind key="W-S-4"><action name="SendToDesktop"><to>4</to></action></keybind>
  </keyboard>
  <mouse>
    <!-- Restore labwc's built-in mouse bindings FIRST: move (drag the
         titlebar, or Alt+drag anywhere), resize (Alt+right-drag or drag an
         edge/corner), focus+raise on click, and the titlebar Close / Iconify /
         Maximize buttons. Without <default/> a custom <mouse> section REPLACES
         all of them, which is why windows could not be moved and the titlebar
         buttons did nothing. Our own bindings below are then added on top. -->
    <default />
    <context name="Root">
      <mousebind button="Right" action="Press"><action name="ShowMenu"><menu>root-menu</menu></action></mousebind>
    </context>
    <context name="TitleBar">
      <mousebind button="Left" action="DoubleClick"><action name="ToggleMaximize"/></mousebind>
    </context>
  </mouse>
</labwc_config>
"#,
    )
    .unwrap();
}

/// Right-click desktop menu (openbox menu format). Entries whose binary is
/// not installed simply do nothing, so no guards are needed here.
fn write_labwc_menu(rootfs: &Path) {
    let cfg = rootfs.join("root/.config/labwc");
    let _ = fs::create_dir_all(&cfg);
    fs::write(
        cfg.join("menu.xml"),
        br#"<?xml version="1.0" encoding="UTF-8"?>
<openbox_menu>
  <menu id="root-menu" label="Eclipse OS">
    <item label="Terminal"><action name="Execute"><command>/usr/local/bin/eclipse-terminal</command></action></item>
    <item label="Firefox"><action name="Execute"><command>/usr/local/bin/eclipse-firefox</command></action></item>
    <item label="Editor (nano)"><action name="Execute"><command>/usr/local/bin/eclipse-terminal nano</command></action></item>
    <item label="Monitor (top)"><action name="Execute"><command>/usr/local/bin/eclipse-terminal top</command></action></item>
    <separator/>
    <item label="Recargar labwc"><action name="Reconfigure"/></item>
    <item label="Salir de la sesion"><action name="Exit"/></item>
  </menu>
</openbox_menu>
"#,
    )
    .unwrap();
}

/// labwc sources `~/.config/labwc/environment` itself, so these survive even
/// when the compositor is launched from a shell that skipped /etc/profile.
fn write_labwc_environment(rootfs: &Path) {
    let cfg = rootfs.join("root/.config/labwc");
    let _ = fs::create_dir_all(&cfg);
    fs::write(
        cfg.join("environment"),
        b"# Eclipse OS - env for the labwc session (sourced by labwc itself).\n\
          # PATH first: the per-VT console shells do NOT source /etc/profile,\n\
          # so a labwc launched from one inherits a PATH without\n\
          # /usr/local/bin - bypassing the labwc wrapper and making keybinds\n\
          # and autostart unable to find eclipse-terminal (seen as the\n\
          # terminal retry loop failing with rc=127 while foot was installed).\n\
          PATH=/usr/local/bin:/bin:/sbin:/usr/bin:/usr/sbin\n\
          # Xwayland display: deliberately NOT pinned. It used to be DISPLAY=:0\n\
          # on the theory that labwc lazy-spawns a rootless Xwayland on the\n\
          # first X11 connect -- but on the RTX the spawn never completes\n\
          # (dmesg shows Xwayland never even reaches CHANNEL_ALLOC), and an\n\
          # X11 connect to labwc's socket then blocks FOREVER waiting for the\n\
          # server. With DISPLAY pinned, that hang swallowed every app that\n\
          # merely PROBES X11: the full `vulkaninfo` report (it queries Xlib/\n\
          # xcb surface support after enumerating -- seen frozen right after\n\
          # listing the RTX 2060 SUPER with the kernel completely idle),\n\
          # glxgears, any toolkit probing X11 first. Unset, X11 apps fail\n\
          # FAST and LOUD (\"cannot open display\") and Wayland-native apps\n\
          # (vulkaninfo, vkcube, eglgears_wayland, foot) are unaffected.\n\
          # Getting Xwayland actually working again is its own follow-up; when\n\
          # it does, export DISPLAY=:0 here and in eclipse-init's CHILD_ENV.\n\
          XCURSOR_THEME=Adwaita\n\
          # lunarbg draws its logo round by pre-squeezing for the panel aspect.\n\
          # It auto-detects the panel aspect from wl_output.geometry (the EDID\n\
          # physical size), so this is only a FALLBACK for drivers/panels that\n\
          # report no physical size (then circles would draw round-for-the-mode,\n\
          # i.e. elliptical on a 16:9 panel driven at a 4:3 mode). Auto-detect\n\
          # takes priority when available; set it to your panel if needed.\n\
          LUNARBG_ASPECT=16:9\n\
          XCURSOR_SIZE=24\n\
          GTK_THEME=Adwaita:dark\n\
          # UTF-8 locale: foot refuses box-drawing/unicode and prints\n\
          # \"error: 'C' is not a UTF-8 locale\" without it. musl accepts any\n\
          # UTF-8 locale name; glibc userspaces ship C.UTF-8 as a builtin.\n\
          LANG=C.UTF-8\n\
          # Private gdk-pixbuf loader registry, regenerated by autostart on\n\
          # every session start (the apk trigger that creates the system one\n\
          # may never have run). Harmless if the file does not exist yet.\n\
          GDK_PIXBUF_MODULE_FILE=/root/.cache/pixbuf-loaders.cache\n\
          # No D-Bus session bus on Eclipse OS: keep GTK's settings out of\n\
          # dconf so apps never try to autolaunch one.\n\
          GSETTINGS_BACKEND=memory\n",
    )
    .unwrap();
}

/// Session clients used to launch from labwc's `autostart` via
/// `spawn_async_no_shell("sh …/autostart")`. On Eclipse that path SIGSEGVs
/// inside busybox ash (musl mallocng walks a NULL context pointer at
/// `pc=0x552425`) — labwc is heavily multi-threaded and the fork/exec of `sh`
/// is the trigger; the same script is fine on Linux and when started from
/// single-threaded eclipse-init. So: **do not create an autostart file**
/// (labwc skips missing scripts) and launch lunarbg/lunarbar as init
/// services instead (see `write_boot_services` in mod.rs).
fn write_labwc_autostart(rootfs: &Path) {
    let cfg = rootfs.join("root/.config/labwc");
    let _ = fs::create_dir_all(&cfg);
    let path = cfg.join("autostart");
    let _ = fs::remove_file(&path);
    // Leave a breadcrumb so operators looking for the old script know where
    // the clients went.
    fs::write(
        cfg.join("autostart.README"),
        b"Eclipse OS: labwc autostart is intentionally absent.\n\
          labwc runs `sh ~/.config/labwc/autostart` via a double-fork; that\n\
          ash crashes on this kernel (SIGSEGV in musl mallocng). Wallpaper\n\
          and panel are started by eclipse-init instead:\n\
            /etc/eclipse/services/lunarbg.service\n\
            /etc/eclipse/services/lunarbar.service\n\
          Wrappers: /usr/local/bin/eclipse-lunarbg, eclipse-lunarbar.\n",
    )
    .unwrap();
}

/// Prefer dark GTK everywhere (file managers, editors, dialogs).
fn write_gtk_settings(rootfs: &Path) {
    for ver in ["gtk-3.0", "gtk-4.0"] {
        let dir = rootfs.join("root/.config").join(ver);
        let _ = fs::create_dir_all(&dir);
        fs::write(
            dir.join("settings.ini"),
            b"[Settings]\n\
              gtk-application-prefer-dark-theme=1\n\
              gtk-theme-name=Adwaita-dark\n\
              gtk-icon-theme-name=Adwaita\n\
              gtk-cursor-theme-name=Adwaita\n\
              gtk-font-name=DejaVu Sans 10\n",
        )
        .unwrap();
    }
}

/// foot terminal palette matching the desktop (deep violet background,
/// lavender foreground).
fn write_foot_config(rootfs: &Path) {
    let dir = rootfs.join("root/.config/foot");
    let _ = fs::create_dir_all(&dir);
    fs::write(
        dir.join("foot.ini"),
        b"# Eclipse OS - foot terminal theme.\n\
          # Name a real monospace family first: the bare 'monospace' alias\n\
          # can resolve to a non-mono font (DejaVuMathTeXGyre) on a minimal\n\
          # fontconfig, which foot warns about on every start.\n\
          font=DejaVu Sans Mono:size=10,monospace:size=10\n\
          pad=6x6\n\
          # Single render worker. foot defaults to one render thread PER CPU\n\
          # (logs \"using N rendering threads\"); on a 20-core box that is 20\n\
          # threads hammering the render mutex/semaphores at once, which\n\
          # SEGFAULTs foot (terminal exits rc=139) on Eclipse's young SMP;\n\
          # the crash never reproduces in a 2-CPU VM. One worker trades a\n\
          # little redraw speed for a terminal that actually starts.\n\
          workers=1\n\
          \n\
          [colors-dark]\n\
          background=120f1c\n\
          foreground=e0dcf4\n\
          regular0=1d1930\n\
          regular1=e07a7a\n\
          regular2=8fd18a\n\
          regular3=e0c07a\n\
          regular4=8a9fe0\n\
          regular5=b98ae0\n\
          regular6=7ac9d1\n\
          regular7=c9c4e4\n\
          bright0=3a3357\n\
          bright1=f09a9a\n\
          bright2=aef0a8\n\
          bright3=f0d89a\n\
          bright4=a8bef0\n\
          bright5=d1a8f0\n\
          bright6=9ae0e8\n\
          bright7=f0eefc\n",
    )
    .unwrap();
}

/// Bulletproof `labwc` launcher. wlroots picks its renderer from
/// WLR_RENDERER *at exec time*, and it does NOT auto-fall-back from gles2 to
/// pixman. On this box the nvidia DRM node is a stub with no usable
/// GLES2/GBM: the gles2/GBM path hangs the whole OS at GL FBO creation.
///
/// We set the vars in the kernel init env and /etc/profile, but login(1)
/// rebuilds the environment and strips arbitrary vars, so a compositor
/// started from a post-login shell can lose them. A wrapper is the only
/// delivery that cannot be stripped: it re-exports what is missing in
/// labwc's own process and execs the real binary. Placed in /usr/local/bin,
/// which is prepended to PATH (kernel init env + /etc/profile) so it wins.
fn write_labwc_wrapper(rootfs: &Path) {
    let localbin = rootfs.join("usr/local/bin");
    let _ = fs::create_dir_all(&localbin);
    let wrapper = localbin.join("labwc");
    fs::write(
        &wrapper,
        b"#!/bin/sh\n\
          # Eclipse OS: labwc launcher. See xtask/src/linux/desktop.rs.\n\
          # A Wayland compositor needs XDG_RUNTIME_DIR for its socket; set it\n\
          # here too in case labwc was started from a non-login shell that\n\
          # never sourced /etc/profile (otherwise clients can't connect and\n\
          # the desktop stays black with no autostart).\n\
          : \"${XDG_RUNTIME_DIR:=/run/user/0}\"\n\
          export XDG_RUNTIME_DIR\n\
          [ -d \"$XDG_RUNTIME_DIR\" ] || { mkdir -p \"$XDG_RUNTIME_DIR\" && chmod 0700 \"$XDG_RUNTIME_DIR\"; }\n\
          # Software cursor needs an XCURSOR theme on disk; wlroots draws NO\n\
          # pointer without one (apk add adwaita-icon-theme).\n\
          : \"${XCURSOR_THEME:=Adwaita}\"; export XCURSOR_THEME\n\
          : \"${XCURSOR_SIZE:=24}\"; export XCURSOR_SIZE\n\
          # Do NOT set WLR_NO_HARDWARE_CURSORS: the kernel DRM scheme composites\n\
          # the legacy MODE_CURSOR bitmap over every scanout frame, so wlroots'\n\
          # hardware-cursor path works and avoids re-rendering the whole pixman\n\
          # scene on every pointer move. Forcing software cursors used to paper\n\
          # over a missing cursor ioctl and burned a core idle; leave the var\n\
          # unset unless a caller overrides it for debugging.\n\
          # Software renderer. This kernel's /dev/dri/card0 is the software-KMS\n\
          # path (pixman scanout, no GBM/EGL): wlroots' default GLES2 renderer\n\
          # would try to eglCreateContext on a node with no GL and abort before\n\
          # the first frame. pixman is the renderer the whole desktop was\n\
          # designed around (README-drm.md).\n\
          : \"${WLR_RENDERER:=pixman}\"; export WLR_RENDERER\n\
          # Backends: DRM for output + libinput for evdev. Naming them keeps\n\
          # wlroots off the headless/X11 autodetect fallbacks when no parent\n\
          # display exists.\n\
          : \"${WLR_BACKENDS:=drm,libinput}\"; export WLR_BACKENDS\n\
          # Name the DRM node explicitly. eclipse-init does NOT source\n\
          # /etc/profile, so a supervised labwc otherwise reaches wlroots'\n\
          # udev GPU enumeration -- which returns nothing without a running\n\
          # udevd and makes wlroots abort with 'Found 0 GPUs, cannot create\n\
          # backend'. With WLR_DRM_DEVICES set, wlroots skips enumeration and\n\
          # opens this node directly (via libseat), which is exactly the KMS\n\
          # device Eclipse exposes. Colon-separated list; we have one card.\n\
          : \"${WLR_DRM_DEVICES:=/dev/dri/card0}\"; export WLR_DRM_DEVICES\n\
          # wlroots' libinput backend aborts the compositor when it enumerates\n\
          # ZERO input devices ('libinput initialization failed, no input\n\
          # devices' -> 'Failed to initialize backend'). Without udevd, libinput\n\
          # may find none even though /dev/input/event* exist. This flag lets\n\
          # the compositor start regardless; devices that ARE discovered still\n\
          # work. (Same setting as /etc/profile, which init-launched sessions\n\
          # never source -- that difference alone kept the boot session dead\n\
          # while a console-launched labwc worked.)\n\
          : \"${WLR_LIBINPUT_NO_DEVICES:=1}\"; export WLR_LIBINPUT_NO_DEVICES\n\
          # Seat/session: wlroots opens DRM and input devices through libseat.\n\
          # eclipse-init runs a seatd daemon (seatd.service); libseat auto-\n\
          # detects it via the socket, so LIBSEAT_BACKEND stays UNSET here.\n\
          # Do NOT set LIBSEAT_BACKEND=builtin: Alpine's libseat is built\n\
          # without the daemonless builtin backend, so that only yields\n\
          # 'No backend matched name builtin' and labwc aborts. A caller-set\n\
          # LIBSEAT_BACKEND still wins if someone knows better.\n\
          # No wait loop here: eclipse-init already parks natively on\n\
          # wait_socket=/run/seatd.sock before EVERY start of this service\n\
          # (boot and crash-restarts alike). The old `sleep 0.1`-per-poll\n\
          # shell loop forked a busybox per iteration for nothing; a manual\n\
          # launch from a shell has seatd long since up. One-shot diagnose:\n\
          seatd_sock=\"${SEATD_SOCK:-/run/seatd.sock}\"\n\
          if [ -z \"${LIBSEAT_BACKEND:-}\" ] && [ ! -S \"$seatd_sock\" ]; then\n\
          \x20 echo \"labwc: seatd socket $seatd_sock not up; libseat may fail\" >&2\n\
          fi\n\
          # XDG basedir a few clients read; harmless if already set.\n\
          : \"${XDG_CONFIG_HOME:=$HOME/.config}\"; export XDG_CONFIG_HOME\n\
          # UTF-8 locale: foot refuses to render with a plain 'C' locale\n\
          # (\"error: 'C' is not a UTF-8 locale\"). init-launched sessions never\n\
          # source /etc/profile, so set it here like the rest of the session env.\n\
          : \"${LANG:=C.UTF-8}\"; export LANG\n\
          # GL clients INSIDE this session (foot -> eglgears/glxgears, Xwayland\n\
          # apps) inherit labwc's environment, so pin them to zink+NVK here on\n\
          # NVIDIA. Our nouveau uAPI implements zink/NVK's VM_BIND/EXEC\n\
          # submission, not classic nvc0 GEM_PUSHBUF -- an unpinned client\n\
          # silently falls back to llvmpipe and crashes on dma-buf import.\n\
          # eclipse-init pins these in its child env, but login(1) strips\n\
          # arbitrary vars, so a session started through any login chain lost\n\
          # them; the wrapper is the one delivery that survives every chain.\n\
          # `:=` keeps a caller's explicit override. NVIDIA-gated (same check\n\
          # as eclipse-init) so QEMU's virtio/virgl stays untouched.\n\
          if [ \"$(cat /sys/class/drm/card0/device/vendor 2>/dev/null)\" = \"0x10de\" ]; then\n\
          \x20 : \"${GALLIUM_DRIVER:=zink}\"; export GALLIUM_DRIVER\n\
          \x20 : \"${MESA_LOADER_DRIVER_OVERRIDE:=zink}\"; export MESA_LOADER_DRIVER_OVERRIDE\n\
          fi\n\
          # Capture labwc's own stdout/stderr (wlroots backend/output/input\n\
          # discovery, errors) to a file. init wires a service's stdio to\n\
          # /dev/null, so without this redirect a black-screen bring-up leaves\n\
          # NO diagnostics anywhere. `cat /tmp/labwc.log` after a failed start\n\
          # then says whether wlroots found a DRM output, set a mode, and which\n\
          # libinput devices it opened.\n\
          LOG=/tmp/labwc.log\n\
          : > \"$LOG\" 2>/dev/null || true\n\
          for d in /usr/bin /bin /usr/sbin /sbin; do\n\
          \x20 if [ -x \"$d/labwc\" ]; then exec \"$d/labwc\" \"$@\" >>\"$LOG\" 2>&1; fi\n\
          done\n\
          # NOT INSTALLED -- see the matching note in eclipse-seatd. `>&2` alone\n\
          # goes to /dev/null under init and leaves an empty /tmp/labwc.log, so\n\
          # write the reason into the log and onto the console as well.\n\
          MSG='labwc: the real labwc binary is NOT INSTALLED -- the Wayland\n\
          session cannot start, so lunarbg/lunarbar have nothing to connect to.\n\
          Fix: apk add labwc seatd  (needs network at image-build time).'\n\
          echo \"$MSG\" >>\"$LOG\" 2>/dev/null || true\n\
          echo \"$MSG\" > /dev/console 2>/dev/null || true\n\
          echo \"$MSG\" >&2\n\
          # Back off instead of hot-looping on a missing package (see eclipse-seatd).\n\
          sleep 60\n\
          exit 127\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Autostart file is intentionally absent (clients come from init).
    #[test]
    fn autostart_is_absent() {
        let dir = std::env::temp_dir().join(format!("eclipse-desktop-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        write_labwc_autostart(&dir);
        assert!(
            !dir.join("root/.config/labwc/autostart").exists(),
            "autostart must not exist — labwc would `sh` it and ash SIGSEGVs"
        );
        assert!(
            dir.join("root/.config/labwc/autostart.README").is_file(),
            "breadcrumb README should explain the move to init services"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
/// Build-time wallpaper renderer. Draws the Eclipse OS night scene (gradient
/// sky, stars, crescent moon, mountain silhouettes, the Eclipse disc with
/// its three white stripes and the "Eclipse OS" wordmark) into an RGB
/// buffer, then encodes a valid PNG with no external crates: stored-deflate
/// zlib blocks + hand-rolled CRC32/Adler32. Fully deterministic.
mod wallpaper {
    /// Render the scene and return the encoded PNG bytes.
    pub fn render_png(w: usize, h: usize) -> Vec<u8> {
        let img = render(w, h);
        encode_png(w as u32, h as u32, &img)
    }

    // ---------------------------------------------------------------- scene

    fn render(w: usize, h: usize) -> Vec<u8> {
        let mut buf = vec![0f32; w * h * 3]; // linear-ish RGB accumulation

        let fw = w as f32;
        let fh = h as f32;
        // Logo geometry, shared by several passes (stars avoid it).
        let cx = fw * 0.5;
        let cy = fh * 0.43;
        let radius = fh * 0.19;

        // Sky: vertical gradient, deep navy to violet with a warm glow band
        // near the horizon (behind the mountains).
        for y in 0..h {
            let t = y as f32 / fh;
            let (r, g, b) = sky_color(t);
            // Horizon glow: gaussian band centred just above the ridge line.
            let glow = (-((y as f32 - fh * 0.78) / (fh * 0.10)).powi(2)).exp() * 0.30;
            for x in 0..w {
                let i = (y * w + x) * 3;
                buf[i] = r + glow * 0.75;
                buf[i + 1] = g + glow * 0.35;
                buf[i + 2] = b + glow * 0.60;
            }
        }

        draw_stars(&mut buf, w, h, cx, cy, radius);
        draw_crescent_moon(&mut buf, w, h, fw * 0.83, fh * 0.16, fh * 0.05);
        draw_mountains(&mut buf, w, h);
        draw_logo(&mut buf, w, h, cx, cy, radius);
        draw_wordmark(&mut buf, w, h, cx, cy + radius + fh * 0.075);

        // Quantise with a hair of deterministic noise so the smooth gradients
        // do not band at 8 bits.
        let mut out = vec![0u8; w * h * 3];
        for (i, v) in buf.iter().enumerate() {
            let n = (hash2(i as u32, 0x9e3779b9) as f32 / u32::MAX as f32 - 0.5) * 1.5;
            out[i] = (v.clamp(0.0, 1.0) * 255.0 + n).round().clamp(0.0, 255.0) as u8;
        }
        out
    }

    fn sky_color(t: f32) -> (f32, f32, f32) {
        // Stops: deep navy -> indigo -> violet.
        let a = (0.051, 0.043, 0.118); // #0d0b1e
        let b = (0.102, 0.078, 0.251); // #1a1440
        let c = (0.239, 0.165, 0.388); // #3d2a63
        if t < 0.55 {
            lerp3(a, b, t / 0.55)
        } else {
            lerp3(b, c, (t - 0.55) / 0.45)
        }
    }

    fn lerp3(a: (f32, f32, f32), b: (f32, f32, f32), t: f32) -> (f32, f32, f32) {
        let t = t.clamp(0.0, 1.0);
        (
            a.0 + (b.0 - a.0) * t,
            a.1 + (b.1 - a.1) * t,
            a.2 + (b.2 - a.2) * t,
        )
    }

    fn draw_stars(buf: &mut [f32], w: usize, h: usize, cx: f32, cy: f32, radius: f32) {
        for i in 0..240u32 {
            let x = (hash2(i, 1) % w as u32) as f32;
            let y = (hash2(i, 2) % (h as u32 * 6 / 10)) as f32;
            // Keep the area around the logo clean.
            let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            if d < radius + 70.0 {
                continue;
            }
            let bright = 0.25 + (hash2(i, 3) % 1000) as f32 / 1000.0 * 0.75;
            add_px(
                buf,
                w,
                h,
                x as i32,
                y as i32,
                (bright, bright, bright * 0.95),
            );
            // A cross twinkle on the brightest few.
            if bright > 0.85 {
                let half = bright * 0.35;
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    add_px(buf, w, h, x as i32 + dx, y as i32 + dy, (half, half, half));
                }
            }
        }
    }

    fn draw_crescent_moon(buf: &mut [f32], w: usize, h: usize, mx: f32, my: f32, r: f32) {
        // Crescent = disc minus an offset disc; soft-edged via coverage.
        let bite_x = mx + r * 0.45;
        let bite_y = my - r * 0.18;
        let (x0, x1) = span(mx, r + 2.0, w);
        let (y0, y1) = span(my, r + 2.0, h);
        for y in y0..y1 {
            for x in x0..x1 {
                let d = dist(x as f32, y as f32, mx, my);
                let db = dist(x as f32, y as f32, bite_x, bite_y);
                let cover = coverage(r, d) * (1.0 - coverage(r * 0.92, db));
                if cover > 0.0 {
                    blend_px(buf, w, x, y, (0.85, 0.83, 0.94), cover * 0.9);
                }
            }
        }
    }

    /// One mountain silhouette layer: baseline height, the six sine-wave
    /// parameters that shape the ridge, and its RGB color.
    type MountainLayer = (f32, [f32; 6], (f32, f32, f32));

    fn draw_mountains(buf: &mut [f32], w: usize, h: usize) {
        let fh = h as f32;
        // Two silhouette layers; the front one is darker.
        let layers: [MountainLayer; 2] = [
            (
                fh * 0.780,
                [0.0040, 1.7, 0.011, 0.4, 0.027, 2.2],
                (0.133, 0.102, 0.220), // #221a38
            ),
            (
                fh * 0.845,
                [0.0060, 4.1, 0.015, 1.1, 0.033, 0.0],
                (0.090, 0.067, 0.161), // #171129
            ),
        ];
        for (base, p, color) in layers {
            for x in 0..w {
                let fx = x as f32;
                let ridge = base
                    + (fx * p[0] + p[1]).sin() * fh * 0.055
                    + (fx * p[2] + p[3]).sin() * fh * 0.028
                    + (fx * p[4] + p[5]).sin() * fh * 0.012;
                let start = ridge.max(0.0) as usize;
                for y in start..h {
                    // Soft top edge on the first row.
                    let cover = if y == start {
                        1.0 - (ridge - ridge.floor())
                    } else {
                        1.0
                    };
                    blend_px(buf, w, x, y, color, cover);
                }
            }
        }
    }

    fn draw_logo(buf: &mut [f32], w: usize, h: usize, cx: f32, cy: f32, r: f32) {
        // Outer glow halo.
        let halo = r * 0.55;
        let (x0, x1) = span(cx, r + halo, w);
        let (y0, y1) = span(cy, r + halo, h);
        for y in y0..y1 {
            for x in x0..x1 {
                let d = dist(x as f32, y as f32, cx, cy);
                if d > r && d < r + halo {
                    let t = 1.0 - (d - r) / halo;
                    let a = t * t * 0.45;
                    add_px(
                        buf,
                        w,
                        h,
                        x as i32,
                        y as i32,
                        (0.55 * a, 0.47 * a, 0.86 * a),
                    );
                }
            }
        }
        // Dark disc with a subtle vertical shade, then a lavender rim.
        for y in y0..y1 {
            for x in x0..x1 {
                let d = dist(x as f32, y as f32, cx, cy);
                let cover = coverage(r, d);
                if cover > 0.0 {
                    let shade = 0.5 + (y as f32 - cy) / (2.0 * r); // 0 top .. 1 bottom
                    let base = lerp3((0.110, 0.086, 0.208), (0.055, 0.043, 0.118), shade);
                    blend_px(buf, w, x, y, base, cover);
                }
                let rim = (1.0 - ((d - r).abs() / 3.0)).clamp(0.0, 1.0);
                if rim > 0.0 {
                    blend_px(buf, w, x, y, (0.66, 0.60, 0.91), rim * 0.9);
                }
            }
        }
        // The three white stripes (rounded bars across the disc).
        for (off_frac, bar_frac) in [(-0.36f32, 0.085f32), (-0.02, 0.085), (0.32, 0.085)] {
            let yb = cy + r * off_frac;
            let bar_r = r * bar_frac;
            let dy = yb - cy;
            let chord = (r * r - dy * dy).max(0.0).sqrt();
            let half = chord - r * 0.16;
            if half <= 0.0 {
                continue;
            }
            let (bx0, bx1) = span(cx, half + bar_r + 2.0, w);
            let (by0, by1) = span(yb, bar_r + 2.0, h);
            for y in by0..by1 {
                for x in bx0..bx1 {
                    let d = capsule_dist(x as f32, y as f32, cx - half, yb, cx + half, yb);
                    let cover = coverage(bar_r, d);
                    if cover > 0.0 {
                        blend_px(buf, w, x, y, (0.95, 0.94, 0.98), cover);
                    }
                }
            }
        }
    }

    /// "Eclipse OS" in a 5x7 bitmap font, scaled up with soft edges.
    fn draw_wordmark(buf: &mut [f32], w: usize, h: usize, cx: f32, cy: f32) {
        let text = "Eclipse OS";
        let scale = (h as f32 * 0.008).round().max(3.0) as usize; // ~7px at 900
        let advance = (6 * scale) as i32;
        let total = text.len() as i32 * advance - scale as i32;
        // Signed coords: on small renders the text may start off-canvas.
        let left = cx as i32 - total / 2;
        let top = cy as i32;
        for (ci, ch) in text.chars().enumerate() {
            let glyph = glyph5x7(ch);
            let gx = left + ci as i32 * advance;
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..5 {
                    if bits & (0b10000 >> col) == 0 {
                        continue;
                    }
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let x = gx + (col * scale + sx) as i32;
                            let y = top + (row * scale + sy) as i32;
                            if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                                blend_px(buf, w, x as usize, y as usize, (0.85, 0.82, 0.94), 0.95);
                            }
                        }
                    }
                }
            }
        }
    }

    fn glyph5x7(c: char) -> [u8; 7] {
        match c {
            'E' => [
                0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
            ],
            'c' => [
                0b00000, 0b00000, 0b01111, 0b10000, 0b10000, 0b10000, 0b01111,
            ],
            'l' => [
                0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
            ],
            'i' => [
                0b00100, 0b00000, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110,
            ],
            'p' => [
                0b00000, 0b00000, 0b11110, 0b10001, 0b10001, 0b11110, 0b10000,
            ],
            's' => [
                0b00000, 0b00000, 0b01111, 0b10000, 0b01110, 0b00001, 0b11110,
            ],
            'e' => [
                0b00000, 0b00000, 0b01110, 0b10001, 0b11111, 0b10000, 0b01111,
            ],
            'O' => [
                0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
            ],
            'S' => [
                0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
            ],
            _ => [0; 7],
        }
    }

    // ------------------------------------------------------- draw utilities

    fn dist(x: f32, y: f32, cx: f32, cy: f32) -> f32 {
        ((x - cx).powi(2) + (y - cy).powi(2)).sqrt()
    }

    /// Anti-aliased coverage of a disc of radius `r` at distance `d`.
    fn coverage(r: f32, d: f32) -> f32 {
        (r - d + 0.5).clamp(0.0, 1.0)
    }

    /// Distance from a point to the segment (ax,ay)-(bx,by); a capsule is
    /// all points within `bar_r` of the segment.
    fn capsule_dist(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 {
            (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        dist(px, py, ax + dx * t, ay + dy * t)
    }

    /// Pixel range [c-r, c+r] clamped to [0, limit).
    fn span(c: f32, r: f32, limit: usize) -> (usize, usize) {
        let lo = (c - r).floor().max(0.0) as usize;
        let hi = ((c + r).ceil() as usize + 1).min(limit);
        (lo, hi)
    }

    fn add_px(buf: &mut [f32], w: usize, h: usize, x: i32, y: i32, c: (f32, f32, f32)) {
        if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
            return;
        }
        let i = (y as usize * w + x as usize) * 3;
        buf[i] += c.0;
        buf[i + 1] += c.1;
        buf[i + 2] += c.2;
    }

    fn blend_px(buf: &mut [f32], w: usize, x: usize, y: usize, c: (f32, f32, f32), a: f32) {
        let i = (y * w + x) * 3;
        buf[i] = buf[i] * (1.0 - a) + c.0 * a;
        buf[i + 1] = buf[i + 1] * (1.0 - a) + c.1 * a;
        buf[i + 2] = buf[i + 2] * (1.0 - a) + c.2 * a;
    }

    /// Deterministic 2-input integer hash (xorshift-style avalanche).
    fn hash2(a: u32, b: u32) -> u32 {
        let mut x = a.wrapping_mul(0x85eb_ca6b) ^ b.wrapping_mul(0xc2b2_ae35);
        x ^= x >> 16;
        x = x.wrapping_mul(0x7feb_352d);
        x ^= x >> 15;
        x = x.wrapping_mul(0x846c_a68b);
        x ^ (x >> 16)
    }

    // ---------------------------------------------------------- PNG encoder

    /// Minimal PNG encoder: 8-bit RGB, zlib stream made of *stored* (i.e.
    /// uncompressed) deflate blocks. Larger on disk than real compression
    /// but standards-compliant, dependency-free and trivially correct.
    fn encode_png(w: u32, h: u32, rgb: &[u8]) -> Vec<u8> {
        assert_eq!(rgb.len(), (w * h * 3) as usize);
        let mut png = Vec::with_capacity(rgb.len() + rgb.len() / 32 + 1024);
        png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);

        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, RGB, deflate, none, no interlace
        write_chunk(&mut png, b"IHDR", &ihdr);

        // Raw scanlines: filter byte 0 (None) + RGB row.
        let stride = (w * 3) as usize;
        let mut raw = Vec::with_capacity((stride + 1) * h as usize);
        for row in rgb.chunks_exact(stride) {
            raw.push(0);
            raw.extend_from_slice(row);
        }

        // zlib wrapper with stored deflate blocks (max 65535 bytes each).
        let mut z = Vec::with_capacity(raw.len() + raw.len() / 65535 * 5 + 16);
        z.extend_from_slice(&[0x78, 0x01]); // CMF/FLG, 32K window, check ok
        let mut chunks = raw.chunks(65535).peekable();
        while let Some(block) = chunks.next() {
            let last = chunks.peek().is_none();
            z.push(last as u8); // BFINAL, BTYPE=00 (stored)
            let len = block.len() as u16;
            z.extend_from_slice(&len.to_le_bytes());
            z.extend_from_slice(&(!len).to_le_bytes());
            z.extend_from_slice(block);
        }
        z.extend_from_slice(&adler32(&raw).to_be_bytes());
        write_chunk(&mut png, b"IDAT", &z);
        write_chunk(&mut png, b"IEND", &[]);
        png
    }

    fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc = Crc32::new();
        crc.update(kind);
        crc.update(data);
        out.extend_from_slice(&crc.finish().to_be_bytes());
    }

    struct Crc32(u32);

    impl Crc32 {
        fn new() -> Self {
            Crc32(0xffff_ffff)
        }
        fn update(&mut self, data: &[u8]) {
            // Canonical bit-at-a-time reflected CRC32 (poly 0xEDB88320).
            for &b in data {
                self.0 ^= b as u32;
                for _ in 0..8 {
                    self.0 = if self.0 & 1 != 0 {
                        0xedb8_8320 ^ (self.0 >> 1)
                    } else {
                        self.0 >> 1
                    };
                }
            }
        }
        fn finish(self) -> u32 {
            self.0 ^ 0xffff_ffff
        }
    }

    fn adler32(data: &[u8]) -> u32 {
        const MOD: u32 = 65521;
        let (mut a, mut b) = (1u32, 0u32);
        for chunk in data.chunks(5552) {
            for &byte in chunk {
                a += byte as u32;
                b += a;
            }
            a %= MOD;
            b %= MOD;
        }
        (b << 16) | a
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn crc32_matches_reference() {
            // CRC32 of "123456789" is the classic check value 0xCBF43926.
            let mut crc = Crc32::new();
            crc.update(b"123456789");
            assert_eq!(crc.finish(), 0xcbf4_3926);
        }

        #[test]
        fn adler32_matches_reference() {
            // Adler32 of "Wikipedia" per the algorithm's documentation.
            assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        }

        /// Manual aid: render the real wallpaper to $ECLIPSE_WALLPAPER_OUT
        /// (or the temp dir) so it can be eyeballed without a full build.
        /// Run with: `cargo test -p xtask dump_wallpaper -- --ignored`.
        #[test]
        #[ignore]
        fn dump_wallpaper() {
            let out = std::env::var_os("ECLIPSE_WALLPAPER_OUT")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::env::temp_dir().join("eclipse-night.png"));
            std::fs::write(&out, render_png(1600, 900)).unwrap();
            eprintln!("wallpaper written to {}", out.display());
        }

        #[test]
        fn png_structure_is_valid() {
            let png = render_png(64, 36);
            assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
            assert_eq!(&png[12..16], b"IHDR");
            assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        }
    }
}
