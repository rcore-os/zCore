mod btrfs_image;
mod desktop;
mod image;
mod nvidia_firmware;
mod opencv;
mod test;
mod xorg;

use crate::{commands::fetch_online, Arch, PROJECT_DIR, REPOS};
use os_xtask_utils::{dir, CommandExt, Ext, Git, Make};
use std::{
    env,
    ffi::OsString,
    fs,
    os::unix,
    path::{Path, PathBuf},
};

pub(crate) struct LinuxRootfs(Arch);

impl LinuxRootfs {
    /// 生成指定架构的 linux rootfs 操作对象。
    #[inline]
    pub const fn new(arch: Arch) -> Self {
        Self(arch)
    }

    /// 构造启动内存文件系统 rootfs。
    /// 对于 x86_64，这个文件系统可用于 libos 启动。
    /// 若设置 `clear`，将清除已存在的目录。
    pub fn make(&self, clear: bool) {
        // 若已存在且不需要清空，可以直接退出
        let dir = self.path();
        if dir.is_dir() && !clear {
            Self::install_ca_certs(&dir);
            let musl = self.0.linux_musl_cross();
            let bin = dir.join("bin");
            // Ensure busybox applet symlinks are present even on incremental builds.
            // Without this, a rootfs built before symlink support was added (or from a
            // partial build) would produce a rootfs image where `ls`, `cat`, etc. are
            // missing, making the installed system unusable.
            Self::ensure_busybox_applets(&bin);
            let nl_dump = self.nl_dump(&musl);
            if nl_dump.is_file() {
                let _ = fs::copy(&nl_dump, bin.join("nl_dump"));
            }
            let edhcpc = self.edhcpc(&musl);
            if edhcpc.is_file() {
                let _ = fs::copy(&edhcpc, bin.join("edhcpc"));
            }
            let install_eclipse = self.install_eclipse(&musl);
            if install_eclipse.is_file() {
                let _ = fs::copy(&install_eclipse, bin.join("install-eclipse"));
            }
            let eclipse_useradd = self.eclipse_useradd(&musl);
            if eclipse_useradd.is_file() {
                let _ = fs::copy(&eclipse_useradd, bin.join("eclipse-useradd"));
            }
            let eclipse_bench = self.eclipse_bench(&musl);
            if eclipse_bench.is_file() {
                let _ = fs::copy(&eclipse_bench, bin.join("eclipse-bench"));
            }
            self.install_thread_tests(&dir);
            // INIT (PID 1): the Eclipse-native Rust init by default, with busybox
            // init as a resilient fallback. `install_busybox_init` runs first so
            // `/sbin/init` always resolves to *some* PID 1; `install_eclipse_init`
            // then repoints it at `eclipse-init` when its (best-effort) build is
            // available.
            Self::install_base_accounts(&dir);
            self.install_busybox_init(&dir);
            self.install_eclipse_init(&dir, &musl);
            // Incremental builds (`make image` -> make(false)) take this early
            // path when the rootfs already exists — the common case, since a
            // checked-in rootfs/<arch> is present. It previously RETURNED before
            // desktop/Xorg install, so `startx` was never baked in on any but a
            // from-scratch (`clear`) build (observed: a freshly-built image
            // booting to "sh: startx: not found"). Refresh the desktop config
            // and (best-effort, idempotent) the Xorg package set here too, so an
            // ordinary rebuild picks them up. apk-add of already-present
            // packages is a no-op, so this is cheap on repeat builds.
            // /etc/profile carries the shell environment AND the serial
            // terminal-size probe; refresh it here too so a plain `make image`
            // (this incremental path) picks up changes, not only a from-scratch
            // build.
            Self::write_profile(&dir.join("etc"));
            desktop::install(&dir);
            xorg::install(&dir, &bin.join("apk"), self.0.name());
            return;
        }
        // 准备最小系统需要的资源
        let musl = self.0.linux_musl_cross();
        let busybox = self.busybox(&musl);
        // 拷贝 apk
        let bin = dir.join("bin");
        let lib = dir.join("lib");
        dir::clear(&dir).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&lib).unwrap();

        let apk = self.apk(&musl);
        if apk.is_file() {
            fs::copy(&apk, bin.join("apk")).unwrap();
            let etc = dir.join("etc");
            let etc_apk = etc.join("apk");
            fs::create_dir_all(&etc_apk).unwrap();
            fs::write(
                etc_apk.join("repositories"),
                "http://dl-cdn.alpinelinux.org/alpine/v3.24/main\nhttp://dl-cdn.alpinelinux.org/alpine/v3.24/community\n",
            )
            .unwrap();
            fs::write(etc_apk.join("world"), "").unwrap();

            // Alpine repo signatures: without /etc/apk/keys/*.rsa.pub apk reports
            // "UNTRUSTED signature" and leaves 0 packages after a slow index download.
            let keys_dst = etc_apk.join("keys");
            fs::create_dir_all(&keys_dst).unwrap();
            let keys_src = PROJECT_DIR.join("prebuilt").join("alpine-apk-keys");
            if keys_src.is_dir() {
                for entry in fs::read_dir(&keys_src).unwrap().flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("pub") {
                        fs::copy(&path, keys_dst.join(entry.file_name())).unwrap();
                    }
                }
            } else {
                eprintln!(
                    "warning: missing prebuilt/alpine-apk-keys — apk update will show UNTRUSTED signature"
                );
            }

            Self::write_resolv_conf(&etc);
            Self::write_hosts(&etc);
            let lib_apk = dir.join("lib").join("apk");
            fs::create_dir_all(&lib_apk).unwrap();
            let lib_apk_db = lib_apk.join("db");
            fs::create_dir_all(&lib_apk_db).unwrap();
            fs::write(lib_apk_db.join("installed"), "").unwrap();

            let var_lib = dir.join("var").join("lib");
            fs::create_dir_all(&var_lib).unwrap();
            #[cfg(unix)]
            let _ = unix::fs::symlink("../../lib/apk", var_lib.join("apk"));

            let var_cache_apk = dir.join("var").join("cache").join("apk");
            fs::create_dir_all(&var_cache_apk).unwrap();
        }

        // 拷贝 busybox
        fs::copy(busybox, bin.join("busybox")).unwrap();

        let etc = dir.join("etc");
        fs::create_dir_all(&etc).unwrap();
        if !etc.join("resolv.conf").exists() {
            Self::write_resolv_conf(&etc);
        }
        if !etc.join("hosts").exists() {
            Self::write_hosts(&etc);
        }
        Self::write_profile(&etc);
        Self::write_passwd(&etc, &dir);
        Self::write_console_configs(&etc, &dir);
        desktop::install(&dir);
        // Bake the whole X.Org stack (server + libinput input driver + software
        // GL + xkb data + base fonts + xterm) into the rootfs so `startx` works
        // out of the box, instead of leaving it as a runtime `apk add` chore
        // that a fresh install / a fresh QEMU boot does not have. Best-effort:
        // an offline build just warns and ships without it. Uses the apk binary
        // and repositories already staged above.
        xorg::install(&dir, &bin.join("apk"), self.0.name());
        Self::install_ca_certs(&dir);

        // /etc/machine-id — prevents dhcp_vendor "No such file or directory".
        // MUST be 32 lowercase hex digits: dbus validates it and refuses to
        // autolaunch ("Invalid machine ID in /etc/machine-id") on anything
        // else, which breaks GTK apps' session-bus fallback. "ec1195e0" is
        // hex-alphabet leetspeak for "eclipse0". Written only when absent, so
        // an existing (installer-generated) id is preserved — an installed
        // system with the old non-hex id needs it rewritten once by hand.
        let machine_id = etc.join("machine-id");
        if !machine_id.exists() {
            fs::write(&machine_id, b"ec1195e0ec1195e0ec1195e0ec1195e0\n").unwrap();
        }

        // /etc/hostname
        fs::write(etc.join("hostname"), b"Eclipse\n").unwrap();

        // /etc/fstab — placeholders sustituidos por install-eclipse (sin mount syscall)
        fs::write(
            etc.join("fstab"),
            b"# /etc/fstab - generado por install-eclipse\n\
# <dispositivo>      <punto de montaje>  <tipo>  <opciones>       <dump>  <pass>\n\
__ECLIPSE_ROOT_DEV__  /                  btrfs   defaults          0  1\n\
__ECLIPSE_EFI_DEV___  /boot/efi          vfat    defaults,noatime  0  0\n\
__ECLIPSE_HOME_DEV__  /home              btrfs   defaults          0  0\n\
__ECLIPSE_SWAP_DEV__  none               swap    sw                0  0\n",
        )
        .unwrap();

        // 拷贝 libc.so
        let from = musl
            .join(format!("{}-linux-musl", self.0.name()))
            .join("lib")
            .join("libc.so");
        let to = lib.join(format!("ld-musl-{arch}.so.1", arch = self.0.name()));
        fs::copy(from, &to).unwrap();
        Ext::new(self.strip(&musl)).arg("-s").arg(to).invoke();
        // 为 busybox 支持的所有 applets 建立符号链接
        Self::ensure_busybox_applets(&bin);
        // Create standard pseudo-filesystem mount points
        let _ = fs::create_dir_all(dir.join("run"));
        let _ = fs::create_dir_all(dir.join("proc"));
        let _ = fs::create_dir_all(dir.join("sys"));
        let _ = fs::create_dir_all(dir.join("tmp"));
        let _ = fs::create_dir_all(dir.join("dev"));
        // Mount points referenced by /etc/fstab (EFI system partition, /home).
        // They must exist so `mount` (and the boot-time fstab processing) can
        // attach the filesystems there.
        let _ = fs::create_dir_all(dir.join("boot/efi"));
        let _ = fs::create_dir_all(dir.join("home"));

        // udhcpc / udhcpc6 scripts — apply leases via `ip` (no ifconfig/route)
        let udhcpc_dir = dir.join("usr/share/udhcpc");
        fs::create_dir_all(&udhcpc_dir).unwrap();
        let udhcpc_script = udhcpc_dir.join("default.script");
        fs::write(
            &udhcpc_script,
            b"#!/bin/sh\n\
              # udhcpc (DHCPv4) script for Eclipse OS\n\
              RESOLV_CONF=/etc/resolv.conf\n\
              mask_to_prefix() {\n\
                case \"$1\" in\n\
                  255.255.255.255) echo 32 ;;\n\
                  255.255.255.254) echo 31 ;;\n\
                  255.255.255.252) echo 30 ;;\n\
                  255.255.255.248) echo 29 ;;\n\
                  255.255.255.240) echo 28 ;;\n\
                  255.255.255.224) echo 27 ;;\n\
                  255.255.255.192) echo 26 ;;\n\
                  255.255.255.128) echo 25 ;;\n\
                  255.255.255.0)   echo 24 ;;\n\
                  255.255.0.0)     echo 16 ;;\n\
                  255.0.0.0)       echo 8 ;;\n\
                  *)               echo 24 ;;\n\
                esac\n\
              }\n\
              case \"$1\" in\n\
                deconfig)\n\
                  ip link set dev \"$interface\" up 2>/dev/null\n\
                  ip -4 addr flush dev \"$interface\" 2>/dev/null\n\
                  ip -4 route del default dev \"$interface\" 2>/dev/null\n\
                  ;;\n\
                bound|renew)\n\
                  ip link set dev \"$interface\" up 2>/dev/null\n\
                  prefix=$(mask_to_prefix \"${subnet:-255.255.255.0}\")\n\
                  ip -4 addr flush dev \"$interface\" 2>/dev/null\n\
                  ip -4 addr add \"$ip/$prefix\" dev \"$interface\" 2>/dev/null\n\
                  if [ -n \"$router\" ]; then\n\
                    for r in $router; do\n\
                      ip -4 route del default 2>/dev/null\n\
                      ip -4 route add default via \"$r\" dev \"$interface\" 2>/dev/null\n\
                      break\n\
                    done\n\
                  fi\n\
                  if [ -n \"$dns\" ]; then\n\
                    : > \"$RESOLV_CONF\"\n\
                    for d in $dns; do\n\
                      echo \"nameserver $d\" >> \"$RESOLV_CONF\"\n\
                    done\n\
                  fi\n\
                  ;;\n\
                leasefail|nak)\n\
                  ;;\n\
              esac\n\
              exit 0\n",
        )
        .unwrap();
        let udhcpc6_script = udhcpc_dir.join("default6.script");
        fs::write(
            &udhcpc6_script,
            b"#!/bin/sh\n\
              # udhcpc6 (DHCPv6) script for Eclipse OS\n\
              RESOLV_CONF=/etc/resolv.conf\n\
              case \"$1\" in\n\
                deconfig)\n\
                  ip link set dev \"$interface\" up 2>/dev/null\n\
                  ;;\n\
                bound|renew)\n\
                  ip link set dev \"$interface\" up 2>/dev/null\n\
                  if [ -n \"$ipv6\" ]; then\n\
                    ip -6 addr del \"$ipv6/128\" dev \"$interface\" 2>/dev/null\n\
                    ip -6 addr add \"$ipv6/128\" dev \"$interface\" 2>/dev/null\n\
                  fi\n\
                  if [ -n \"$ipv6prefix\" ]; then\n\
                    ip -6 addr add \"$ipv6prefix\" dev \"$interface\" 2>/dev/null\n\
                  fi\n\
                  # Append IPv6 nameservers without truncating -> a full rewrite\n\
                  # would wipe DHCPv4 nameservers already written by udhcpc.\n\
                  if [ -n \"$dns\" ]; then\n\
                    touch \"$RESOLV_CONF\"\n\
                    for d in $dns; do\n\
                      grep -q \"^nameserver $d\\$\" \"$RESOLV_CONF\" 2>/dev/null \\\n\
                        || echo \"nameserver $d\" >> \"$RESOLV_CONF\"\n\
                    done\n\
                  fi\n\
                  ;;\n\
                leasefail|nak)\n\
                  ;;\n\
              esac\n\
              exit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&udhcpc_script, fs::Permissions::from_mode(0o755)).unwrap();
            fs::set_permissions(&udhcpc6_script, fs::Permissions::from_mode(0o755)).unwrap();
        }

        // ALSO install them under /etc/udhcpc.
        //
        // `/usr/share/udhcpc/default.script` is where Alpine's busybox looks.
        // The busybox actually staged into this image is Ubuntu's, and its
        // compiled-in path is `/etc/udhcpc/default.script` — confirmed with
        // `strings /bin/busybox` in the running guest. So `udhcpc` found no
        // script at all and applied nothing:
        //
        //   udhcpc: lease of 10.0.2.15 obtained from 10.0.2.2   <- DHCP fine
        //   ip addr show eth0 -> inet 0.0.0.0/0                 <- never applied
        //
        // The interface stayed at 0.0.0.0, so every outbound connection failed
        // and `apk` had no network — while the link itself was up and the
        // e1000e driver was transmitting and receiving correctly. Passing
        // `-s /usr/share/udhcpc/default.script` explicitly configured the
        // address immediately, which is what isolated it.
        //
        // Install to BOTH paths rather than picking one: which busybox ends up
        // in the image is a staging decision that has changed before, and a
        // duplicated 1 KiB script is far cheaper than a silently networkless
        // system. `/etc` is already in LIVE_KEEP, so this ships in the QEMU
        // initramfs too.
        let etc_udhcpc = dir.join("etc/udhcpc");
        fs::create_dir_all(&etc_udhcpc).unwrap();
        for name in ["default.script", "default6.script"] {
            let dst = etc_udhcpc.join(name);
            fs::copy(udhcpc_dir.join(name), &dst).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&dst, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }

        // openssl wrapper to busybox ssl_client
        let usr_sbin = dir.join("usr/sbin");
        fs::create_dir_all(&usr_sbin).unwrap();
        let openssl_script = usr_sbin.join("openssl");
        fs::write(
            &openssl_script,
            b"#!/bin/sh\n\
              if [ \"$1\" != \"s_client\" ]; then\n\
                echo \"openssl wrapper: command '$1' not supported\" >&2\n\
                exit 1\n\
              fi\n\
              shift\n\
              CONNECT=\"\"\n\
              SERVERNAME=\"\"\n\
              while [ $# -gt 0 ]; do\n\
                case \"$1\" in\n\
                  -connect)\n\
                    CONNECT=\"$2\"\n\
                    shift 2\n\
                    ;;\n\
                  -servername)\n\
                    SERVERNAME=\"$2\"\n\
                    shift 2\n\
                    ;;\n\
                  -quiet)\n\
                    shift 1\n\
                    ;;\n\
                  *)\n\
                    shift 1\n\
                    ;;\n\
                esac\n\
              done\n\
              if [ -z \"$CONNECT\" ]; then\n\
                echo \"openssl wrapper: missing -connect\" >&2\n\
                exit 1\n\
              fi\n\
              if [ -n \"$SERVERNAME\" ]; then\n\
                exec ssl_client -n \"$SERVERNAME\" \"$CONNECT\"\n\
              else\n\
                exec ssl_client \"$CONNECT\"\n\
              fi\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&openssl_script, fs::Permissions::from_mode(0o755)).unwrap();
        }

        // 拷贝 nl_dump (netlink dump helper).
        // Do this AFTER symlink creation to ensure it's a real binary, not a BusyBox link.
        let nl_dump = self.nl_dump(&musl);
        if nl_dump.is_file() {
            let dst = bin.join("nl_dump");
            let _ = dir::rm(&dst);
            fs::copy(&nl_dump, &dst).unwrap();
        }

        // 拷贝 edhcpc (Eclipse DHCPv4 client).
        // This is a static, minimal DHCPv4 client that uses rtnetlink to apply IP/gw.
        let edhcpc = self.edhcpc(&musl);
        if edhcpc.is_file() {
            let dst = bin.join("edhcpc");
            let _ = dir::rm(&dst);
            fs::copy(&edhcpc, &dst).unwrap();
        }

        // DNS/hosts resolver shim (dynamic) + CLI helper.
        let eclipse_resolv = self.eclipse_resolv(&musl);
        if eclipse_resolv.is_file() {
            let _ = dir::rm(bin.join("eclipse-resolv"));
            fs::copy(&eclipse_resolv, bin.join("eclipse-resolv")).unwrap();
        }
        let libeclipse_dns = self.libeclipse_dns(&musl);
        if libeclipse_dns.is_file() {
            let _ = dir::rm(lib.join("libeclipse_dns.so"));
            fs::copy(&libeclipse_dns, lib.join("libeclipse_dns.so")).unwrap();
        }

        // labwc spawn race guard (delayed g_strfreev + NULL-safe execvp).
        // See tools/eclipse-spawnfix/spawnfix.c and the labwc wrapper.
        let libeclipse_spawnfix = self.libeclipse_spawnfix(&musl);
        if libeclipse_spawnfix.is_file() {
            let _ = dir::rm(lib.join("libeclipse_spawnfix.so"));
            fs::copy(&libeclipse_spawnfix, lib.join("libeclipse_spawnfix.so")).unwrap();
        }

        // lunarbg: the native wallpaper client (Rust, static musl). Renders
        // the Eclipse night scene procedurally over wlr-layer-shell, replacing
        // swaybg and its gdk-pixbuf image-decoding dependency entirely.
        let lunarbg = self.lunarbg();
        if lunarbg.is_file() {
            let _ = dir::rm(bin.join("lunarbg"));
            fs::copy(&lunarbg, bin.join("lunarbg")).unwrap();
        } else {
            eprintln!("warning: lunarbg not built; autostart will fall back to swaybg");
        }

        // lunarbar: the native status/task bar (Rust, static musl). A two-bar
        // panel over wlr-layer-shell + wlr-foreign-toplevel-management, replacing
        // waybar and its GTK/D-Bus/gdk-pixbuf/fontconfig dependency chain.
        let lunarbar = self.lunarbar();
        if lunarbar.is_file() {
            let _ = dir::rm(bin.join("lunarbar"));
            fs::copy(&lunarbar, bin.join("lunarbar")).unwrap();
        } else {
            eprintln!("warning: lunarbar not built; autostart will fall back to waybar");
        }

        // 拷贝 install-eclipse
        let install_eclipse = self.install_eclipse(&musl);
        if install_eclipse.is_file() {
            let dst = bin.join("install-eclipse");
            let _ = dir::rm(&dst);
            fs::copy(&install_eclipse, &dst).unwrap();
        }

        // 拷贝 eclipse-useradd
        let eclipse_useradd = self.eclipse_useradd(&musl);
        if eclipse_useradd.is_file() {
            let dst = bin.join("eclipse-useradd");
            let _ = dir::rm(&dst);
            fs::copy(&eclipse_useradd, &dst).unwrap();
        }

        // 拷贝 eclipse-bench (CPU/mem/disk/process benchmark)
        let eclipse_bench = self.eclipse_bench(&musl);
        if eclipse_bench.is_file() {
            let dst = bin.join("eclipse-bench");
            let _ = dir::rm(&dst);
            fs::copy(&eclipse_bench, &dst).unwrap();
        }

        self.install_thread_tests(&dir);
        // INIT (PID 1): the Eclipse-native Rust init by default; busybox init
        // kept as a resilient fallback. busybox lays down `/sbin/init` -> busybox
        // (+ inittab + rcS) first, then `install_eclipse_init` repoints
        // `/sbin/init` -> `eclipse-init` when its (best-effort) build succeeds.
        Self::install_base_accounts(&dir);
        self.install_busybox_init(&dir);
        self.install_eclipse_init(&dir, &musl);
    }

    /// Instala tests freestanding de multihilo (thr3: repro de la barrier de sysbench).
    fn install_thread_tests(&self, rootfs: &Path) {
        if let Arch::X86_64 = self.0 {
            let thr3 = self.thread_test_thr3();
            if thr3.is_file() {
                let dst = rootfs.join("thr3");
                let _ = dir::rm(&dst);
                fs::copy(&thr3, &dst).unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&dst, fs::Permissions::from_mode(0o755)).unwrap();
                }
            }
        }
    }

    /// Lay down the base `/etc/passwd` and `/etc/group` so name lookups resolve.
    ///
    /// Writes a minimal but standard set of system accounts. `/etc/passwd` is
    /// only created when absent (so it never clobbers accounts added later);
    /// `/etc/group` is created when absent, and otherwise just gets a `uucp`
    /// line appended if it lacks one — idempotent on incremental rebuilds.
    fn install_base_accounts(rootfs: &Path) {
        let etc = rootfs.join("etc");
        let _ = fs::create_dir_all(&etc);

        let passwd = etc.join("passwd");
        if !passwd.exists() {
            fs::write(
                &passwd,
                "root:x:0:0:root:/root:/bin/sh\n\
                 nobody:x:65534:65534:nobody:/:/bin/false\n",
            )
            .unwrap();
        }

        let group = etc.join("group");
        let base = "root:x:0:root\n\
                    bin:x:1:\n\
                    daemon:x:2:\n\
                    sys:x:3:\n\
                    adm:x:4:\n\
                    tty:x:5:\n\
                    disk:x:6:\n\
                    lp:x:7:\n\
                    wheel:x:10:root\n\
                    uucp:x:14:root\n\
                    nogroup:x:65533:\n\
                    nobody:x:65534:\n";
        match fs::read_to_string(&group) {
            Ok(existing) => {
                if !existing.lines().any(|l| l.starts_with("uucp:")) {
                    let mut updated = existing;
                    if !updated.ends_with('\n') {
                        updated.push('\n');
                    }
                    updated.push_str("uucp:x:14:root\n");
                    fs::write(&group, updated).unwrap();
                }
            }
            Err(_) => fs::write(&group, base).unwrap(),
        }
    }

    /// Wire up busybox `init` as the PID 1 base program.
    ///
    /// busybox already ships in the rootfs (with an `init` applet), so no
    /// package install / network is needed: this only lays down `/sbin/init`
    /// (-> `/bin/busybox`), a minimal `/etc/inittab`, and the `/etc/init.d/rcS`
    /// sysinit hook. The Eclipse kernel owns the virtual terminals (it spawns
    /// the per-VT shells itself), so the inittab has NO `getty`/`askfirst`
    /// lines — `init` runs the sysinit hook once and then reaps orphaned
    /// children as PID 1.
    fn install_busybox_init(&self, rootfs: &Path) {
        let etc = rootfs.join("etc");
        let _ = fs::create_dir_all(&etc);

        // /sbin/init -> /bin/busybox. busybox selects its applet from
        // basename(argv[0]), so exec'ing /sbin/init runs the `init` applet
        // regardless of the symlink target. (The kernel boots INIT=/sbin/init.)
        let sbin = rootfs.join("sbin");
        let _ = fs::create_dir_all(&sbin);
        let init_link = sbin.join("init");
        let _ = fs::remove_file(&init_link);
        #[cfg(unix)]
        {
            let _ = unix::fs::symlink("/bin/busybox", &init_link);
        }

        // Minimal inittab. NO getty/askfirst: the kernel already provides the
        // per-VT shells. busybox init runs the sysinit hook, then idles handling
        // Ctrl-Alt-Del / shutdown / restart while reaping orphaned children.
        fs::write(
            etc.join("inittab"),
            b"# Eclipse OS - busybox init. The kernel owns the virtual terminals\n\
              # (it spawns the per-VT shells), so there are NO getty lines here;\n\
              # init runs the sysinit hook once and then reaps orphaned children.\n\
              ::sysinit:/etc/init.d/rcS\n\
              ::ctrlaltdel:/bin/busybox reboot\n\
              ::shutdown:/bin/busybox swapoff -a\n\
              ::restart:/bin/busybox init\n",
        )
        .unwrap();

        // /etc/init.d/rcS — the sysinit hook. The kernel already mounts the root
        // fs, brings up the network and spawns the shells, so this is just a
        // place to start optional background services. Safe by default (no-op);
        // a commented example shows how to launch the seatd seat manager.
        let initd = etc.join("init.d");
        let _ = fs::create_dir_all(&initd);
        let rcs = initd.join("rcS");
        fs::write(
            &rcs,
            b"#!/bin/sh\n\
              # Eclipse OS sysinit hook (busybox init). Add boot-time services\n\
              # here; the kernel already handles root mount, networking and the\n\
              # per-TTY shells. Example - start the Wayland/X seat manager:\n\
              #   [ -x /usr/bin/seatd ] && /usr/bin/seatd >/dev/null 2>&1 &\n\
              exit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&rcs, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// Compila thr3 con gcc del host (bare metal, sin libc).
    fn thread_test_thr3(&self) -> PathBuf {
        let dir = PROJECT_DIR.join("tools").join("thread-tests");
        let source = dir.join("thr3.c");
        let executable = dir.join("thr3-metal");
        if executable.is_file() && source.is_file() {
            if let (Ok(bin_meta), Ok(src_meta)) = (fs::metadata(&executable), fs::metadata(&source))
            {
                if let (Ok(bin_mtime), Ok(src_mtime)) = (bin_meta.modified(), src_meta.modified()) {
                    if bin_mtime >= src_mtime {
                        return executable;
                    }
                }
            }
        }

        println!("Compiling thr3 (sysbench barrier regression test)...");
        fs::create_dir_all(&dir).unwrap();
        let status = Ext::new("gcc")
            .current_dir(&dir)
            .arg("-static")
            .arg("-no-pie")
            .arg("-nostdlib")
            .arg("-fno-stack-protector")
            .arg("-fno-builtin")
            .arg("-O1")
            .arg("-DQUICK_TEST")
            .arg("-o")
            .arg(&executable)
            .arg(&source)
            .status();
        if !status.success() {
            eprintln!("warning: failed to compile thr3");
        }
        executable
    }

    fn write_resolv_conf(etc: &Path) {
        fs::write(
            etc.join("resolv.conf"),
            "nameserver 8.8.8.8\nnameserver 1.1.1.1\n",
        )
        .unwrap();
    }

    fn write_hosts(etc: &Path) {
        fs::write(
            etc.join("hosts"),
            b"127.0.0.1\tlocalhost\n\
::1\t\tlocalhost ip6-localhost ip6-loopback\n\
127.0.1.1\tEclipse\n",
        )
        .unwrap();
    }

    fn write_profile(etc: &Path) {
        fs::write(
            etc.join("profile"),
            b"export PATH=/usr/local/bin:/bin:/sbin:/usr/bin:/usr/sbin\n\
              export SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt\n\
              export SSL_CERT_DIR=/etc/ssl/certs\n\
              export CURL_CA_BUNDLE=/etc/ssl/certs/ca-certificates.crt\n\
              # NOTE: /lib/libeclipse_dns.so is deliberately NOT in LD_PRELOAD.\n\
              # Until the kernel published AT_SECURE/AT_UID..AT_EGID in the\n\
              # auxv, musl ran every process in secure mode and silently\n\
              # DROPPED this preload -- so the whole system (apk, TLS, DNS)\n\
              # has always run without the shim actually loaded. The moment\n\
              # the auxv was fixed and the shim really injected everywhere,\n\
              # labwc froze within seconds of starting. Keep the system on\n\
              # its proven configuration; opt in per command if needed:\n\
              #   LD_PRELOAD=/lib/libeclipse_dns.so some-command\n\
              export HOME=/root\n\
              export TERM=xterm-256color\n\
              # No GPU here: wlroots/labwc must use the software (pixman) renderer.\n\
              # Otherwise wlroots tries GLES2/EGL then Vulkan. With no GPU that\n\
              # path can fail outright, or start on Mesa llvmpipe and become\n\
              # extremely slow because every frame is rendered and copied on CPU.\n\
              # It does not auto-fall back to pixman. See docs/README-drm.md.\n\
              export WLR_RENDERER=pixman\n\
              export WLR_RENDERER_ALLOW_SOFTWARE=1\n\
              # wlroots' libinput backend aborts the whole compositor if it\n\
              # enumerates zero input devices ('libinput initialization failed,\n\
              # no input devices'). Without a running udevd to tag devices,\n\
              # libinput may find none even when /sys/class/input is populated.\n\
              # This flag lets the compositor start regardless; devices that ARE\n\
              # discovered still work, so it is safe to leave on permanently.\n\
              export WLR_LIBINPUT_NO_DEVICES=1\n\
              # Hardware cursor is now composited by the kernel: wlroots' legacy\n\
              # DRM backend calls drmModeSetCursor/MoveCursor, which the DRM\n\
              # scheme accepts and draws over each scanned-out frame. So do NOT\n\
              # set WLR_NO_HARDWARE_CURSORS -- letting wlroots use the hardware\n\
              # cursor path avoids re-rendering the whole scene on every pointer\n\
              # move (the whole point of a HW cursor).\n\
              # Pin wlroots to the console GPU's DRM node (card0 =\n\
              # nvidia-gpu-23:0.0). This box has TWO nvidia DRM cards (card0 =\n\
              # console GPU driving the physical monitor via the UEFI GOP\n\
              # framebuffer; card1 = the compute GPU we run kernels on). Without\n\
              # this, wlroots may enumerate both and bind a phantom connector on\n\
              # the compute GPU. Pinning card0 gives labwc a SINGLE logical\n\
              # output and keeps it off the compute GPU. Physical pixels always\n\
              # land on the GOP framebuffer via the kernel's software-KMS\n\
              # scanout regardless, so this is really about presenting one\n\
              # output, not about which port lights up.\n\
              export WLR_DRM_DEVICES=/dev/dri/card0\n\
              # Software GL via Mesa (no usable HW 3D). The DRM node reports a\n\
              # real NVIDIA PCI id, so Mesa would try the hardware nouveau driver\n\
              # and fail; force the KMS software rasteriser (kms_swrast/llvmpipe)\n\
              # which renders into dumb buffers. Only used when a GL renderer is\n\
              # selected (WLR_RENDERER=gles2); that path is intentionally avoided\n\
              # by default because it is much slower than pixman here.\n\
              export GALLIUM_DRIVER=llvmpipe\n\
              export MESA_LOADER_DRIVER_OVERRIDE=kms_swrast\n\
              # Runtime dir for the Wayland socket (created on demand, mode 0700).\n\
              export XDG_RUNTIME_DIR=/run/user/0\n\
              [ -d \"$XDG_RUNTIME_DIR\" ] || { mkdir -p \"$XDG_RUNTIME_DIR\" && chmod 0700 \"$XDG_RUNTIME_DIR\"; }\n\
              # --- serial terminal size detection --------------------------------\n\
              # The kernel console reports the FRAMEBUFFER size (e.g. 227x113 at a\n\
              # 2048x2048 mode). That is right for the on-screen graphic console\n\
              # but wrong for a serial viewer: full-screen apps (nano, less, top)\n\
              # then overflow. Ask the host terminal (QEMU -serial) for its size\n\
              # via cursor-position report and apply it with TIOCSWINSZ. Reject\n\
              # tiny answers (<2x2) and absurd sizes (>512): a bogus \\e[1;1R or\n\
              # an unanswered \\e[999;999H echo used to leave nano unusable.\n\
              # Only mark ECLIPSE_TTY_SIZED after stty succeeds so a failed probe\n\
              # can retry. With no serial reply this times out in ~0.3s (VTIME)\n\
              # and keeps the framebuffer size.\n\
              if [ -z \"${ECLIPSE_TTY_SIZED:-}\" ] && [ -t 0 ] && [ -t 1 ]; then\n\
              \x20 __sz_old=$(stty -g 2>/dev/null)\n\
              \x20 stty raw -echo min 0 time 3 2>/dev/null\n\
              \x20 printf '\\033[999;999H\\033[6n'\n\
              \x20 __sz=$(dd bs=32 count=1 2>/dev/null)\n\
              \x20 [ -n \"$__sz_old\" ] && stty \"$__sz_old\" 2>/dev/null\n\
              \x20 __sz=${__sz#*[}\n\
              \x20 __rows=${__sz%%;*}\n\
              \x20 __cols=${__sz#*;}; __cols=${__cols%%R*}\n\
              \x20 case \"$__rows$__cols\" in\n\
              \x20   ''|*[!0-9]*) : ;;\n\
              \x20   *) [ \"$__rows\" -gt 1 ] && [ \"$__cols\" -gt 1 ] \\\n\
              \x20        && [ \"$__rows\" -le 512 ] && [ \"$__cols\" -le 512 ] \\\n\
              \x20        && stty rows \"$__rows\" cols \"$__cols\" 2>/dev/null \\\n\
              \x20        && export ECLIPSE_TTY_SIZED=1 ;;\n\
              \x20 esac\n\
              \x20 unset __sz __sz_old __rows __cols\n\
              fi\n",
        )
        .unwrap();
    }

    /// Ensure the root user's home (`/root`) exists and that `/etc/passwd` and
    /// `/etc/group` carry a usable `root` entry. bash resolves `~`/the home
    /// directory via `getpwuid(geteuid())`, i.e. /etc/passwd — without a valid
    /// entry (and an existing home) it greets "I can't find my home directory!".
    /// Only writes the files when absent, so a package-provided passwd/group is
    /// left untouched.
    fn write_passwd(etc: &Path, rootfs: &Path) {
        // Root's home directory must exist for `cd ~` / login to succeed.
        let _ = fs::create_dir_all(rootfs.join("root"));

        let passwd = etc.join("passwd");
        if !passwd.exists() {
            fs::write(
                &passwd,
                b"root:x:0:0:root:/root:/bin/sh\n\
                  nobody:x:65534:65534:nobody:/:/sbin/nologin\n",
            )
            .unwrap();
        }
        let group = etc.join("group");
        if !group.exists() {
            fs::write(
                &group,
                b"root:x:0:\n\
                  nogroup:x:65534:\n\
                  tty:x:5:\n\
                  video:x:28:\n",
            )
            .unwrap();
        }
    }

    /// Lay down configuration that console programs need to behave well:
    ///
    /// - `/root/.bashrc`: bash, unlike POSIX sh, does NOT read `/etc/profile`
    ///   for non-login interactive shells, so source it here to inherit the
    ///   system PATH, the DNS resolver shim (`LD_PRELOAD`) and the SSL cert
    ///   locations. Also sets a readable prompt.
    /// - `/etc/nanorc`: a minimal, option-only nano config (no `include` of the
    ///   syntax files, whose directives trip "Mistakes in '/etc/nanorc'" on some
    ///   nano builds). Written unconditionally so the OS default wins over a
    ///   package file; users can re-add `include` lines for syntax highlighting.
    fn write_console_configs(etc: &Path, rootfs: &Path) {
        let bashrc = rootfs.join("root").join(".bashrc");
        if let Some(parent) = bashrc.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(
            &bashrc,
            b"# Eclipse OS - bash config for the root account.\n\
              # bash ignores /etc/profile for non-login interactive shells, so\n\
              # source it here to inherit PATH, the DNS resolver shim and the\n\
              # SSL certificate locations.\n\
              [ -r /etc/profile ] && . /etc/profile\n\
              export PS1='\\[\\e[1;32m\\]Eclipse\\[\\e[0m\\]:\\[\\e[1;34m\\]\\w\\[\\e[0m\\]\\$ '\n\
              alias ll='ls -la'\n",
        )
        .unwrap();

        fs::write(
            etc.join("nanorc"),
            b"# Eclipse OS - minimal nano configuration.\n\
              # Option-only (no syntax `include`s) so it loads cleanly across\n\
              # nano versions. Add `include \"/usr/share/nano/*.nanorc\"` yourself\n\
              # for syntax highlighting.\n\
              set tabsize 4\n\
              set tabstospaces\n\
              set autoindent\n",
        )
        .unwrap();
    }

    /// Creates symlinks in `bin/` for every busybox applet.
    ///
    /// Called on both full and incremental builds so that a rootfs directory
    /// created before this feature existed (or after a partial build) still ends
    /// up with all applet symlinks in the final ext2 image.  Existing entries
    /// (real binaries like `nl_dump`) are never overwritten.
    fn ensure_busybox_applets(bin: &Path) {
        // Base list of essential applets
        let mut applets: Vec<String> = vec![
            "cat",
            "cp",
            "echo",
            "false",
            "grep",
            "gzip",
            "ip",
            "kill",
            "ln",
            "ls",
            "mkdir",
            "mv",
            "pidof",
            "ping",
            "ps",
            "pwd",
            "rm",
            "rmdir",
            "sh",
            "sleep",
            "stat",
            "tar",
            "touch",
            "true",
            "uname",
            "usleep",
            "watch",
            "ifconfig",
            "route",
            "udhcpc",
            "udhcpc6",
            "sed",
            "awk",
            "cmp",
            "diff",
            "logger",
            "hostname",
            "cut",
            "sort",
            "uniq",
            "head",
            "tail",
            "wc",
            "xargs",
            "find",
            "test",
            "expr",
            "id",
            "date",
            "env",
            "chmod",
            "chown",
            "vi",
            "top",
            "less",
            "ssl_client",
            "ssl_server",
            "wget",
            "traceroute",
            "traceroute6",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        // Complement the list with `busybox --list` when it runs on the host.
        let busybox_bin = bin.join("busybox");
        if let Ok(out) = std::process::Command::new(&busybox_bin)
            .arg("--list")
            .output()
        {
            if out.status.success() {
                if let Ok(s) = String::from_utf8(out.stdout) {
                    for line in s.lines() {
                        let applet = line.trim().to_string();
                        if !applet.is_empty() && !applets.contains(&applet) {
                            applets.push(applet);
                        }
                    }
                }
            }
        }

        for applet in &applets {
            let link = bin.join(applet);
            if !link.exists() && !link.is_symlink() {
                #[cfg(unix)]
                let _ = std::os::unix::fs::symlink("busybox", &link);
            }
        }
    }

    const CA_PEM_URL: &str = "https://curl.se/ca/cacert.pem";

    /// Descarga (si hace falta) el bundle Mozilla y lo deja en `prebuilt/cacert.pem`.
    fn ensure_prebuilt_ca_pem() -> PathBuf {
        let prebuilt = PROJECT_DIR.join("prebuilt/cacert.pem");
        if prebuilt.is_file() {
            return prebuilt;
        }
        if let Some(parent) = prebuilt.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        println!(
            "Fetching CA bundle from {} -> {}",
            Self::CA_PEM_URL,
            prebuilt.display()
        );
        let status = std::process::Command::new("wget")
            .args(["-q", "--show-progress", "-O"])
            .arg(&prebuilt)
            .arg(Self::CA_PEM_URL)
            .status()
            .expect("failed to run wget for CA bundle");
        if !status.success() || !prebuilt.is_file() {
            panic!(
                "CA bundle missing: could not download {}\n\
                 Fix: wget -O prebuilt/cacert.pem {}\n\
                 or run: cargo xtask linux rootfs --arch <arch> --clear",
                prebuilt.display(),
                Self::CA_PEM_URL
            );
        }
        prebuilt
    }

    /// Instala certificados raíz en el rootfs (requerido para wget https).
    fn install_ca_certs(root: &Path) {
        let src = Self::ensure_prebuilt_ca_pem();
        let certs_dir = root.join("etc/ssl/certs");
        fs::create_dir_all(&certs_dir).unwrap();
        let bundle = certs_dir.join("ca-certificates.crt");
        fs::copy(&src, &bundle).unwrap();
        // Alias usado por varias herramientas.
        let alias = certs_dir.join("ca-bundle.crt");
        let _ = fs::remove_file(&alias);
        #[cfg(unix)]
        unix::fs::symlink("ca-certificates.crt", &alias).unwrap();
        #[cfg(not(unix))]
        fs::copy(&bundle, &alias).unwrap();
        println!(
            "Installed CA bundle ({} bytes) -> {}",
            fs::metadata(&bundle).map(|m| m.len()).unwrap_or(0),
            bundle.display()
        );
    }

    /// 将 musl 动态库放入 rootfs。
    pub fn put_musl_libs(&self) -> PathBuf {
        // 递归 rootfs
        self.make(false);
        let dir = self.0.linux_musl_cross();
        self.put_libs(&dir, dir.join(format!("{}-linux-musl", self.0.name())));
        dir
    }

    /// 指定架构的 rootfs 路径。
    #[inline]
    pub fn path(&self) -> PathBuf {
        PROJECT_DIR.join("rootfs").join(self.0.name())
    }

    /// 编译 busybox。
    fn busybox(&self, musl: impl AsRef<Path>) -> PathBuf {
        // 最终文件路径
        let target = self.0.target().join("busybox");
        // 如果文件存在，直接退出
        let executable = target.join("busybox");
        if executable.is_file() {
            return executable;
        }
        // 获得源码
        let source = REPOS.join("busybox");
        if !source.is_dir() {
            fetch_online!(source, |tmp| {
                Git::clone("https://git.busybox.net/busybox.git")
                    .dir(tmp)
                    .single_branch()
                    .depth(1)
                    .done()
            });
        }
        // 拷贝
        dir::rm(&target).unwrap();
        dircpy::copy_dir(source, &target).unwrap();
        // 配置
        Make::new().current_dir(&target).arg("defconfig").invoke();
        // Force static linking and disable PIE (Type EXEC is more stable in zCore)
        Ext::new("sed")
            .current_dir(&target)
            .arg("-i")
            .arg(
                "s/.*CONFIG_STATIC.*/CONFIG_STATIC=y/;\
                  s/.*CONFIG_PIE.*/CONFIG_PIE=n/;\
                  s/.*CONFIG_FEATURE_INDIVIDUAL.*/CONFIG_FEATURE_INDIVIDUAL=n/;\
                  s/.*CONFIG_FEATURE_SHARED_BUSYBOX.*/CONFIG_FEATURE_SHARED_BUSYBOX=n/;\
                  s/.*CONFIG_FEATURE_WGET_OPENSSL.*/CONFIG_FEATURE_WGET_OPENSSL=n/;\
                  s/.*CONFIG_FEATURE_WGET_HTTPS.*/CONFIG_FEATURE_WGET_HTTPS=y/;\
                  s/.*CONFIG_SSL_CLIENT.*/CONFIG_SSL_CLIENT=y/;\
                  s/.*CONFIG_FEATURE_IPV6.*/CONFIG_FEATURE_IPV6=y/;\
                  s/.*CONFIG_UDHCPC6.*/CONFIG_UDHCPC6=y/;\
                  s/.*CONFIG_FEATURE_UDHCPC6_RFC3646.*/CONFIG_FEATURE_UDHCPC6_RFC3646=y/;\
                  s/^# CONFIG_INIT is not set$/CONFIG_INIT=y/;\
                  s/^# CONFIG_FEATURE_USE_INITTAB is not set$/CONFIG_FEATURE_USE_INITTAB=y/",
            )
            .arg(".config")
            .invoke();
        Ext::new("sh")
            .current_dir(&target)
            .arg("-c")
            .arg("yes '' | make oldconfig")
            .invoke();

        // Pin the DHCP client dispatcher scripts explicitly.
        //
        // `udhcpc6 -i eth0` (without `-s`) uses CONFIG_UDHCPC6_DEFAULT_SCRIPT. Its
        // default value differs across busybox versions: older trees point it at the
        // IPv4 `default.script`, whose `deconfig` runs `ip -4 addr flush` (wiping the
        // IPv4 lease) and whose `bound` has no `ip -6 addr add` (so the DHCPv6 lease
        // is never applied). Force the IPv6 client to use `default6.script` and keep
        // the IPv4 client on `default.script` so both are deterministic. Done after
        // `oldconfig` so the (now enabled) UDHCPC6 symbols already exist in `.config`.
        Ext::new("sed")
            .current_dir(&target)
            .arg("-i")
            .arg(
                "s#^CONFIG_UDHCPC_DEFAULT_SCRIPT=.*#CONFIG_UDHCPC_DEFAULT_SCRIPT=\"/usr/share/udhcpc/default.script\"#;\
                  s#^CONFIG_UDHCPC6_DEFAULT_SCRIPT=.*#CONFIG_UDHCPC6_DEFAULT_SCRIPT=\"/usr/share/udhcpc/default6.script\"#",
            )
            .arg(".config")
            .invoke();
        Ext::new("sh")
            .current_dir(&target)
            .arg("-c")
            .arg("yes '' | make oldconfig")
            .invoke();

        // 编译
        let musl = musl.as_ref().canonicalize().unwrap();
        let cross_compile = format!(
            "{musl}/bin/{arch}-linux-musl-",
            musl = musl.display(),
            arch = self.0.name(),
        );

        Make::new()
            .current_dir(&target)
            .arg(format!("CROSS_COMPILE={cross_compile}"))
            .arg("LDFLAGS=-static -no-pie")
            .arg("EXTRA_LDFLAGS=-static -no-pie")
            .arg("CFLAGS=-fno-PIC -fno-PIE")
            .arg("EXTRA_CFLAGS=-fno-PIC -fno-PIE")
            .arg("CONFIG_STATIC=y")
            .arg("CONFIG_PIE=n")
            .invoke();
        // 裁剪
        Ext::new(self.strip(musl))
            .arg("-s")
            .arg(&executable)
            .invoke();
        executable
    }

    /// Descarga (o actualiza) el binario estático de apk-tools desde Chimera Linux.
    ///
    /// El binario se almacena en `tools/apk/apk-<arch>.static`, junto al código
    /// fuente del que ya no se compilará apk.  Se usa `wget --timestamping` (-N)
    /// para que la descarga solo ocurra si el servidor publica una versión más
    /// nueva que la copia local — comportamiento idéntico a los mirrors de Alpine.
    ///
    /// Arquitecturas disponibles en Chimera Linux que también soporta Eclipse OS:
    ///   x86_64 · aarch64 · riscv64
    fn apk(&self, _musl: &Path) -> PathBuf {
        const CHIMERA_APK_BASE: &str = "https://repo.chimera-linux.org/apk/latest";

        let arch = self.0.name(); // "x86_64", "aarch64", "riscv64"
        let filename = format!("apk-{arch}.static");
        let url = format!("{CHIMERA_APK_BASE}/{filename}");

        // Almacenar en tools/apk/ junto al código fuente.
        let apk_src_dir = PROJECT_DIR.join("tools").join("apk");
        let stored = apk_src_dir.join(&filename); // e.g. tools/apk/apk-x86_64.static

        println!("Checking apk ({arch}) against Chimera Linux repo...");
        // -N / --timestamping: sólo descarga si el servidor tiene versión más nueva.
        // -q: silencioso excepto errores.  -P: directorio destino.
        let status = Ext::new("wget")
            .arg("-N")
            .arg("-q")
            .arg("--show-progress")
            .arg("-P")
            .arg(&apk_src_dir)
            .arg(&url)
            .status();

        if status.success() {
            if stored.is_file() {
                // Asegurarse de que tiene permisos de ejecución.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = fs::metadata(&stored).unwrap().permissions();
                    if perms.mode() & 0o111 == 0 {
                        perms.set_mode(perms.mode() | 0o755);
                        fs::set_permissions(&stored, perms).unwrap();
                    }
                }
            }
        } else {
            eprintln!(
                "warning: no se pudo descargar/actualizar apk ({arch}) desde {url}. \
                 Se usará la copia local si existe."
            );
        }

        stored
    }

    /// 编译 nl_dump (static netlink dump helper).
    fn nl_dump(&self, musl: &Path) -> PathBuf {
        let dir = PROJECT_DIR.join("tools").join("nl_dump");
        let executable = dir.join("nl_dump");
        let source = dir.join("nl_dump.c");
        // Rebuild if missing or if source is newer than the binary.
        if executable.is_file() && source.is_file() {
            if let (Ok(bin_meta), Ok(src_meta)) = (fs::metadata(&executable), fs::metadata(&source))
            {
                if let (Ok(bin_mtime), Ok(src_mtime)) = (bin_meta.modified(), src_meta.modified()) {
                    if bin_mtime >= src_mtime {
                        return executable;
                    }
                }
            }
        }

        println!("Compiling nl_dump...");
        let musl = musl.canonicalize().unwrap();
        let bin = musl.join("bin");
        let arch = self.0.name();
        let cc = format!("{}/{}-linux-musl-gcc", bin.display(), arch);
        let strip = self.strip(&musl);

        fs::create_dir_all(&dir).unwrap();
        let status = Ext::new(&cc)
            .current_dir(&dir)
            .arg("-static")
            .arg("-O2")
            .arg("-s")
            .arg("-o")
            .arg(&executable)
            .arg(&source)
            .status();
        if !status.success() {
            println!("Failed to compile nl_dump");
            return executable;
        }

        Ext::new(strip).arg("-s").arg(&executable).status();
        executable
    }

    /// 编译 edhcpc (static DHCPv4 client for Eclipse OS).
    fn edhcpc(&self, musl: &Path) -> PathBuf {
        let dir = PROJECT_DIR.join("tools").join("edhcpc");
        let executable = dir.join("edhcpc");
        let source = dir.join("edhcpc.c");
        // Rebuild if missing or if source is newer than the binary.
        if executable.is_file() && source.is_file() {
            if let (Ok(bin_meta), Ok(src_meta)) = (fs::metadata(&executable), fs::metadata(&source))
            {
                if let (Ok(bin_mtime), Ok(src_mtime)) = (bin_meta.modified(), src_meta.modified()) {
                    if bin_mtime >= src_mtime {
                        return executable;
                    }
                }
            }
        }

        println!("Compiling edhcpc...");
        let musl = musl.canonicalize().unwrap();
        let bin = musl.join("bin");
        let arch = self.0.name();
        let cc = format!("{}/{}-linux-musl-gcc", bin.display(), arch);
        let strip = self.strip(&musl);

        fs::create_dir_all(&dir).unwrap();
        let status = Ext::new(&cc)
            .current_dir(&dir)
            .arg("-static")
            .arg("-O2")
            .arg("-s")
            .arg("-o")
            .arg(&executable)
            .arg(&source)
            .status();
        if !status.success() {
            println!("Failed to compile edhcpc");
            return executable;
        }

        Ext::new(strip).arg("-s").arg(&executable).status();
        executable
    }

    /// Build libeclipse_dns.so (LD_PRELOAD resolver shim).
    fn libeclipse_dns(&self, musl: &Path) -> PathBuf {
        let dir = PROJECT_DIR.join("tools").join("eclipse-resolv");
        let lib = dir.join("libeclipse_dns.so");
        let source = dir.join("resolv.c");
        if lib.is_file() && source.is_file() {
            if let (Ok(lib_meta), Ok(src_meta)) = (fs::metadata(&lib), fs::metadata(&source)) {
                if let (Ok(lib_mtime), Ok(src_mtime)) = (lib_meta.modified(), src_meta.modified()) {
                    if lib_mtime >= src_mtime {
                        return lib;
                    }
                }
            }
        }

        println!("Compiling libeclipse_dns.so...");
        let musl = musl.canonicalize().unwrap();
        let arch = self.0.name();
        let cc = format!("{}/{}-linux-musl-gcc", musl.join("bin").display(), arch);
        fs::create_dir_all(&dir).unwrap();
        let status = Ext::new(&cc)
            .current_dir(&dir)
            .arg("-shared")
            .arg("-fPIC")
            .arg("-O2")
            .arg("-o")
            .arg(&lib)
            .arg(&source)
            .status();
        if !status.success() {
            eprintln!("warning: failed to compile libeclipse_dns.so");
        }
        lib
    }

    /// Build libeclipse_spawnfix.so (LD_PRELOAD for labwc's double-fork spawn).
    fn libeclipse_spawnfix(&self, musl: &Path) -> PathBuf {
        let dir = PROJECT_DIR.join("tools").join("eclipse-spawnfix");
        let lib = dir.join("libeclipse_spawnfix.so");
        let source = dir.join("spawnfix.c");
        if lib.is_file() && source.is_file() {
            if let (Ok(lib_meta), Ok(src_meta)) = (fs::metadata(&lib), fs::metadata(&source)) {
                if let (Ok(lib_mtime), Ok(src_mtime)) = (lib_meta.modified(), src_meta.modified()) {
                    if lib_mtime >= src_mtime {
                        return lib;
                    }
                }
            }
        }

        println!("Compiling libeclipse_spawnfix.so...");
        let musl = musl.canonicalize().unwrap();
        let arch = self.0.name();
        let cc = format!("{}/{}-linux-musl-gcc", musl.join("bin").display(), arch);
        fs::create_dir_all(&dir).unwrap();
        let status = Ext::new(&cc)
            .current_dir(&dir)
            .arg("-shared")
            .arg("-fPIC")
            .arg("-O2")
            .arg("-o")
            .arg(&lib)
            .arg(&source)
            .arg("-ldl")
            .arg("-lpthread")
            .status();
        if !status.success() {
            eprintln!("warning: failed to compile libeclipse_spawnfix.so");
        }
        lib
    }

    /// Build eclipse-resolv CLI (static).
    fn eclipse_resolv(&self, musl: &Path) -> PathBuf {
        let dir = PROJECT_DIR.join("tools").join("eclipse-resolv");
        let executable = dir.join("eclipse-resolv");
        let source = dir.join("eclipse-resolv.c");
        if executable.is_file() && source.is_file() {
            if let (Ok(bin_meta), Ok(src_meta)) = (fs::metadata(&executable), fs::metadata(&source))
            {
                if let (Ok(bin_mtime), Ok(src_mtime)) = (bin_meta.modified(), src_meta.modified()) {
                    if bin_mtime >= src_mtime {
                        return executable;
                    }
                }
            }
        }

        println!("Compiling eclipse-resolv...");
        let musl = musl.canonicalize().unwrap();
        let arch = self.0.name();
        let cc = format!("{}/{}-linux-musl-gcc", musl.join("bin").display(), arch);
        let strip = self.strip(&musl);
        fs::create_dir_all(&dir).unwrap();
        let status = Ext::new(&cc)
            .current_dir(&dir)
            .arg("-static")
            .arg("-O2")
            .arg("-s")
            .arg("-o")
            .arg(&executable)
            .arg(&source)
            .status();
        if !status.success() {
            eprintln!("warning: failed to compile eclipse-resolv");
            return executable;
        }
        Ext::new(strip).arg("-s").arg(&executable).status();
        executable
    }

    /// 编译 install-eclipse (static installer for Eclipse OS).
    fn install_eclipse(&self, musl: &Path) -> PathBuf {
        let dir = PROJECT_DIR.join("tools").join("install-eclipse");
        let executable = dir.join("install-eclipse");
        let source = dir.join("install-eclipse.c");
        // Rebuild if missing or if source is newer than the binary.
        if executable.is_file() && source.is_file() {
            if let (Ok(bin_meta), Ok(src_meta)) = (fs::metadata(&executable), fs::metadata(&source))
            {
                if let (Ok(bin_mtime), Ok(src_mtime)) = (bin_meta.modified(), src_meta.modified()) {
                    if bin_mtime >= src_mtime {
                        return executable;
                    }
                }
            }
        }

        println!("Compiling install-eclipse...");
        let musl = musl.canonicalize().unwrap();
        let bin = musl.join("bin");
        let arch = self.0.name();
        let cc = format!("{}/{}-linux-musl-gcc", bin.display(), arch);
        let strip = self.strip(&musl);
        let zlib = PROJECT_DIR.join("tools").join("zlib");
        let zlib_sources = [
            "adler32.c",
            "crc32.c",
            "inflate.c",
            "inffast.c",
            "inftrees.c",
            "zutil.c",
            "gzlib.c",
            "gzread.c",
            "gzclose.c",
        ];

        fs::create_dir_all(&dir).unwrap();
        let mut cmd = Ext::new(&cc);
        cmd.current_dir(&dir)
            .arg("-static")
            .arg("-O2")
            .arg("-s")
            .arg("-D_LARGEFILE64_SOURCE=1")
            .arg("-DNO_GZCOMPRESS")
            .arg(format!("-I{}", zlib.display()))
            .arg("-o")
            .arg(&executable)
            .arg(&source);
        for src in zlib_sources {
            cmd.arg(zlib.join(src));
        }
        let status = cmd.status();
        if !status.success() {
            println!("Failed to compile install-eclipse");
            return executable;
        }

        Ext::new(strip).arg("-s").arg(&executable).status();
        executable
    }

    /// 编译 eclipse-useradd (static user/group manager for Eclipse OS).
    fn eclipse_useradd(&self, musl: &Path) -> PathBuf {
        let dir = PROJECT_DIR.join("tools").join("eclipse-useradd");
        let executable = dir.join("eclipse-useradd");
        let source = dir.join("eclipse-useradd.c");
        if executable.is_file() && source.is_file() {
            if let (Ok(bin_meta), Ok(src_meta)) = (fs::metadata(&executable), fs::metadata(&source))
            {
                if let (Ok(bin_mtime), Ok(src_mtime)) = (bin_meta.modified(), src_meta.modified()) {
                    if bin_mtime >= src_mtime {
                        return executable;
                    }
                }
            }
        }

        println!("Compiling eclipse-useradd...");
        let musl = musl.canonicalize().unwrap();
        let bin = musl.join("bin");
        let arch = self.0.name();
        let cc = format!("{}/{}-linux-musl-gcc", bin.display(), arch);
        let strip = self.strip(&musl);

        fs::create_dir_all(&dir).unwrap();
        let status = Ext::new(&cc)
            .current_dir(&dir)
            .arg("-static")
            .arg("-O2")
            .arg("-s")
            .arg("-o")
            .arg(&executable)
            .arg(&source)
            .status();
        if !status.success() {
            println!("Failed to compile eclipse-useradd");
            return executable;
        }

        Ext::new(strip).arg("-s").arg(&executable).status();
        executable
    }

    /// Compile the eclipse-bench CPU/memory/disk/process benchmark (static musl)
    /// so it lands in the rootfs at /bin/eclipse-bench. Skips recompilation when
    /// the binary is newer than its single source file.
    fn eclipse_bench(&self, musl: &Path) -> PathBuf {
        let dir = PROJECT_DIR.join("tools").join("eclipse-bench");
        let executable = dir.join("eclipse-bench");
        let source = dir.join("eclipse-bench.c");
        if executable.is_file() && source.is_file() {
            if let (Ok(bin_meta), Ok(src_meta)) = (fs::metadata(&executable), fs::metadata(&source))
            {
                if let (Ok(bin_mtime), Ok(src_mtime)) = (bin_meta.modified(), src_meta.modified()) {
                    if bin_mtime >= src_mtime {
                        return executable;
                    }
                }
            }
        }

        println!("Compiling eclipse-bench...");
        let musl = musl.canonicalize().unwrap();
        let bin = musl.join("bin");
        let arch = self.0.name();
        let cc = format!("{}/{}-linux-musl-gcc", bin.display(), arch);
        let strip = self.strip(&musl);

        fs::create_dir_all(&dir).unwrap();
        let status = Ext::new(&cc)
            .current_dir(&dir)
            .arg("-static")
            .arg("-O2")
            .arg("-s")
            .arg("-o")
            .arg(&executable)
            .arg(&source)
            .status();
        if !status.success() {
            println!("Failed to compile eclipse-bench");
            return executable;
        }

        Ext::new(strip).arg("-s").arg(&executable).status();
        executable
    }

    fn strip(&self, musl: impl AsRef<Path>) -> PathBuf {
        musl.as_ref()
            .join("bin")
            .join(format!("{}-linux-musl-strip", self.0.name()))
    }

    /// Rust target triple for the userspace musl build of `eclipse-init`.
    fn musl_rust_triple(&self) -> &'static str {
        match self.0 {
            Arch::X86_64 => "x86_64-unknown-linux-musl",
            Arch::Aarch64 => "aarch64-unknown-linux-musl",
            Arch::Riscv64 => "riscv64gc-unknown-linux-musl",
        }
    }

    /// Cross-compile the Eclipse-native init (`tools/eclipse-init`, Rust) as a
    /// static, non-PIE musl binary and return its path. Best-effort: on failure
    /// the path may not exist and the caller keeps busybox init as PID 1.
    ///
    /// Built static + `relocation-model=static` (non-PIE) to match the rest of
    /// the rootfs (busybox/apk are static non-PIE ET_EXEC), which the Eclipse
    /// loader handles well.
    fn eclipse_init(&self, _musl: &Path) -> PathBuf {
        let dir = PROJECT_DIR.join("tools").join("eclipse-init");
        let triple = self.musl_rust_triple();
        let executable = dir
            .join("target")
            .join(triple)
            .join("release")
            .join("eclipse-init");
        let source = dir.join("src").join("main.rs");
        if executable.is_file() && source.is_file() {
            if let (Ok(b), Ok(s)) = (fs::metadata(&executable), fs::metadata(&source)) {
                if let (Ok(bm), Ok(sm)) = (b.modified(), s.modified()) {
                    if bm >= sm {
                        return executable;
                    }
                }
            }
        }

        println!("Compiling eclipse-init (Rust, {triple})...");
        // Make sure the userspace musl target is available (best-effort).
        let _ = Ext::new("rustup")
            .arg("target")
            .arg("add")
            .arg(triple)
            .status();

        let status = Ext::new("cargo")
            .current_dir(&dir)
            .arg("build")
            .arg("--release")
            .arg("--target")
            .arg(triple)
            // Static non-PIE, like busybox/apk in the rootfs.
            .env("RUSTFLAGS", "-C relocation-model=static")
            .status();
        if !status.success() {
            eprintln!("warning: eclipse-init build failed; busybox init remains PID 1");
        }
        executable
    }

    /// Cross-compile lunarbg (`tools/lunarbg`, Rust) as a static musl binary
    /// and return its path. Best-effort: if the build fails the desktop
    /// autostart falls back to swaybg (see xtask/src/linux/desktop.rs).
    fn lunarbg(&self) -> PathBuf {
        let dir = PROJECT_DIR.join("tools").join("lunarbg");
        let triple = self.musl_rust_triple();
        let executable = dir
            .join("target")
            .join(triple)
            .join("release")
            .join("lunarbg");
        // Rebuild when any source file is newer than the binary.
        let newest_src = ["src/main.rs", "src/scene.rs", "src/par.rs", "Cargo.toml"]
            .iter()
            .filter_map(|rel| fs::metadata(dir.join(rel)).ok()?.modified().ok())
            .max();
        if let (Ok(bin_meta), Some(src_mtime)) = (fs::metadata(&executable), newest_src) {
            if let Ok(bin_mtime) = bin_meta.modified() {
                if bin_mtime >= src_mtime {
                    return executable;
                }
            }
        }

        println!("Compiling lunarbg (Rust, {triple})...");
        let _ = Ext::new("rustup")
            .arg("target")
            .arg("add")
            .arg(triple)
            .status();
        let status = Ext::new("cargo")
            .current_dir(&dir)
            .arg("build")
            .arg("--release")
            .arg("--target")
            .arg(triple)
            .env("RUSTFLAGS", "-C relocation-model=static")
            .status();
        if !status.success() {
            eprintln!("warning: lunarbg build failed; swaybg fallback remains");
        }
        executable
    }

    /// Cross-compile lunarbar (`tools/lunarbar`, Rust) as a static musl binary
    /// and return its path. Best-effort: if the build fails the desktop
    /// autostart falls back to waybar (see xtask/src/linux/desktop.rs).
    fn lunarbar(&self) -> PathBuf {
        let dir = PROJECT_DIR.join("tools").join("lunarbar");
        let triple = self.musl_rust_triple();
        let executable = dir
            .join("target")
            .join(triple)
            .join("release")
            .join("lunarbar");
        // Rebuild when any source file is newer than the binary.
        let newest_src = ["src/main.rs", "src/apps.rs", "src/draw.rs", "src/sysinfo.rs", "src/par.rs", "Cargo.toml"]
            .iter()
            .filter_map(|rel| fs::metadata(dir.join(rel)).ok()?.modified().ok())
            .max();
        if let (Ok(bin_meta), Some(src_mtime)) = (fs::metadata(&executable), newest_src) {
            if let Ok(bin_mtime) = bin_meta.modified() {
                if bin_mtime >= src_mtime {
                    return executable;
                }
            }
        }

        println!("Compiling lunarbar (Rust, {triple})...");
        let _ = Ext::new("rustup")
            .arg("target")
            .arg("add")
            .arg(triple)
            .status();
        let status = Ext::new("cargo")
            .current_dir(&dir)
            .arg("build")
            .arg("--release")
            .arg("--target")
            .arg(triple)
            .env("RUSTFLAGS", "-C relocation-model=static")
            .status();
        if !status.success() {
            eprintln!("warning: lunarbar build failed; waybar fallback remains");
        }
        executable
    }

    /// Install the Eclipse-native init as the default PID 1: copy the binary to
    /// `/sbin/eclipse-init`, repoint `/sbin/init` at it (overriding the busybox
    /// fallback laid down by `install_busybox_init`), and seed
    /// `/etc/eclipse/services/` with a documented example. Returns `true` on
    /// success, `false` (leaving busybox init as PID 1) if the build is absent.
    fn install_eclipse_init(&self, rootfs: &Path, musl: &Path) -> bool {
        let bin = self.eclipse_init(musl);
        if !bin.is_file() {
            eprintln!("warning: eclipse-init not built; keeping busybox init as PID 1");
            return false;
        }
        let sbin = rootfs.join("sbin");
        let _ = fs::create_dir_all(&sbin);
        let dst = sbin.join("eclipse-init");
        let _ = fs::remove_file(&dst);
        fs::copy(&bin, &dst).unwrap();

        // /sbin/init -> eclipse-init (the kernel boots INIT=/sbin/init).
        let init_link = sbin.join("init");
        let _ = fs::remove_file(&init_link);
        #[cfg(unix)]
        {
            let _ = unix::fs::symlink("eclipse-init", &init_link);
        }

        // Service directory + a documented (inert) example, plus the default
        // boot services: DHCP, the seat manager and the labwc session.
        let svc_dir = rootfs.join("etc").join("eclipse").join("services");
        let _ = fs::create_dir_all(&svc_dir);
        fs::write(
            svc_dir.join("example.service.txt"),
            b"# Eclipse init service file. Copy to '<name>.service' to enable.\n\
              #\n\
              # exec  = /usr/sbin/mydaemon --foreground   (required; argv, space-split)\n\
              # type  = respawn                            (respawn | oneshot; default oneshot)\n\
              # after = othersvc                           (optional; space-separated deps)\n\
              #\n\
              # 'oneshot' runs to completion in order during boot; 'respawn' is\n\
              # supervised and restarted if it exits. No shell is involved.\n",
        )
        .unwrap();

        // udhcpc: the kernel brings the link up but sets no address, so DHCP is
        // what gives the machine an IP and default route at boot -- before, it
        // only happened when the desktop's prepare step ran udhcpc by hand.
        // Supervised: the foreground client keeps renewing the lease, and init
        // restarts it (with backoff) if it dies.
        fs::write(
            svc_dir.join("udhcpc.service"),
            b"# DHCP client (foreground). See /usr/local/bin/eclipse-udhcpc.\n\
              exec = /usr/local/bin/eclipse-udhcpc\n\
              type = respawn\n",
        )
        .unwrap();

        // seatd: the seat manager wlroots/labwc open DRM and input devices
        // through. Started before labwc so its socket exists when the
        // compositor connects (init's `after` orders the start; the labwc
        // wrapper also falls back to libseat's builtin backend if it is not up
        // yet, so a start-order race just costs one backoff retry).
        fs::write(
            svc_dir.join("seatd.service"),
            b"# Seat manager (foreground). See /usr/local/bin/eclipse-seatd.\n\
              # labwc-only: seatd/libseat is the Wayland seat path; Xorg on the\n\
              # framebuffer opens its devices directly and does not need it.\n\
              exec = /usr/local/bin/eclipse-seatd\n\
              type = respawn\n\
              desktop = labwc\n",
        )
        .unwrap();

        // labwc: the Wayland session, started at boot as a supervised service
        // (boot-to-desktop). The wrapper sets XDG_RUNTIME_DIR, the pixman
        // renderer and the seat backend; if the desktop packages are absent it
        // exits and init backs off rather than hot-looping.
        fs::write(
            svc_dir.join("labwc.service"),
            b"# labwc Wayland session. See /usr/local/bin/labwc.\n\
              exec = /usr/local/bin/labwc\n\
              type = respawn\n\
              after = seatd\n\
              desktop = labwc\n",
        )
        .unwrap();

        // Desktop clients: started by init, NOT by labwc's autostart.
        // labwc double-forks `sh ~/.config/labwc/autostart` and that ash
        // SIGSEGVs on this kernel (musl mallocng, NULL context). Single-
        // threaded eclipse-init forking these wrappers is fine.
        fs::write(
            svc_dir.join("lunarbg.service"),
            b"# Procedural wallpaper (wlr-layer-shell). See eclipse-lunarbg.\n\
              exec = /usr/local/bin/eclipse-lunarbg\n\
              type = respawn\n\
              after = labwc\n\
              desktop = labwc\n",
        )
        .unwrap();
        fs::write(
            svc_dir.join("lunarbar.service"),
            b"# Two-bar panel (wlr-layer-shell). See eclipse-lunarbar.\n\
              exec = /usr/local/bin/eclipse-lunarbar\n\
              type = respawn\n\
              after = labwc\n\
              desktop = labwc\n",
        )
        .unwrap();

        // Xorg session (the framebuffer/fbdev X stack). Selected instead of
        // labwc when the boot picks `desktop=xorg` (e.g. `make qemu`). Runs the
        // eclipse-xorg wrapper, which starts X + the .xinitrc session on VT.
        fs::write(
            svc_dir.join("xorg.service"),
            b"# Xorg session (fbdev on /dev/fb0). See /usr/local/bin/eclipse-xorg.\n\
              exec = /usr/local/bin/eclipse-xorg\n\
              type = respawn\n\
              desktop = xorg\n",
        )
        .unwrap();

        // The two init wrappers (the labwc one is written by desktop.rs).
        let localbin = rootfs.join("usr").join("local").join("bin");
        let _ = fs::create_dir_all(&localbin);
        fs::write(
            localbin.join("eclipse-udhcpc"),
            b"#!/bin/sh\n\
              # Eclipse OS: foreground DHCP on the first real interface, for\n\
              # eclipse-init. Runs udhcpc in the foreground (init supervises it)\n\
              # and keeps renewing the lease; if it dies, init respawns it.\n\
              command -v udhcpc >/dev/null 2>&1 || { echo 'eclipse-udhcpc: udhcpc not found' >&2; sleep 5; exit 127; }\n\
              # -s is NOT optional: busybox udhcpc's compiled-in default script\n\
              # path is not where Eclipse stages the script, so without -s the\n\
              # lease is obtained but never APPLIED -- the address, resolv.conf\n\
              # and route are all set by this script on the 'bound' event.\n\
              SCRIPT=/usr/share/udhcpc/default.script\n\
              [ -x \"$SCRIPT\" ] || SCRIPT=/etc/udhcpc/default.script\n\
              for i in $(ip -o link show 2>/dev/null | sed 's/^[0-9]*: //; s/[@:].*//' | grep -v '^lo'); do\n\
              \x20 echo \"eclipse-udhcpc: udhcpc -i $i -f -s $SCRIPT\"\n\
              \x20 exec udhcpc -i \"$i\" -f -R -s \"$SCRIPT\"\n\
              done\n\
              # No interface yet (late/hotplug probe): sleep so init's backoff\n\
              # is not a hot loop while the NIC appears, then exit to be retried.\n\
              echo 'eclipse-udhcpc: no non-loopback interface yet' >&2\n\
              sleep 3\n\
              exit 1\n",
        )
        .unwrap();
        // Persist contained kernel faults. The fault path records them in RAM
        // (`kernel_hal::oops_log` -> /proc/oops) because it cannot touch a
        // filesystem: interrupts are off, the heap may be the thing that just
        // got smashed, and `oops` only proceeds with NO kernel lock held. This
        // is the userspace half that turns that RAM record into a file that
        // survives the console scrollback -- the same split Linux uses between
        // the printk ring and syslogd.
        fs::write(
            localbin.join("eclipse-oopslog"),
            b"#!/bin/sh\n\
              # Drain /proc/oops into /var/log/oops.log.\n\
              # Contained faults leave the machine RUNNING, so this can take its\n\
              # time; it only needs to beat the next reboot.\n\
              OUT=/var/log/oops.log\n\
              mkdir -p /var/log 2>/dev/null\n\
              last=\n\
              while :; do\n\
              \x20 cur=$(cat /proc/oops 2>/dev/null)\n\
              \x20 case \"$cur\" in ''|'# no contained kernel faults since boot') : ;; *)\n\
              \x20 \x20 if [ \"$cur\" != \"$last\" ]; then\n\
              \x20 \x20 \x20 { echo \"=== $(date 2>/dev/null || echo 'boot+?') ===\"; echo \"$cur\"; } >> \"$OUT\"\n\
              \x20 \x20 \x20 last=$cur\n\
              \x20 \x20 \x20 echo 'eclipse-oopslog: a kernel fault was contained; see /var/log/oops.log' > /dev/console 2>/dev/null\n\
              \x20 \x20 fi\n\
              \x20 esac\n\
              \x20 sleep 10\n\
              done\n",
        )
        .unwrap();

        fs::write(
            svc_dir.join("oopslog.service"),
            b"# Persist contained kernel faults (/proc/oops -> /var/log/oops.log).\n\
              exec = /usr/local/bin/eclipse-oopslog\n\
              type = respawn\n",
        )
        .unwrap();

        fs::write(
            localbin.join("eclipse-seatd"),
            b"#!/bin/sh\n\
              # Eclipse OS: run the seatd daemon in the foreground for\n\
              # eclipse-init. As root it needs no -u/-g; the socket lands at\n\
              # /run/seatd.sock, which the (root) compositor can always open.\n\
              # Capture seatd's own log (init wires a service's stdio to\n\
              # /dev/null, same reason the labwc wrapper captures its log):\n\
              # a compositor that hangs right after \"Loading user-specified\n\
              # backends\" is most likely blocked in the libseat handshake, and\n\
              # without this there is NO visibility into what seatd is doing\n\
              # (or not doing) about that connection.\n\
              LOG=/tmp/seatd.log\n\
              : > \"$LOG\" 2>/dev/null || true\n\
              for d in /usr/bin /bin /usr/sbin /sbin; do\n\
              \x20 [ -x \"$d/seatd\" ] && exec \"$d/seatd\" -l info >>\"$LOG\" 2>&1\n\
              done\n\
              # NOT INSTALLED. Say so where it can actually be found: init wires\n\
              # a service's stdio to /dev/null, so a bare `>&2` here vanished and\n\
              # left an EMPTY /tmp/seatd.log next to an endless respawn storm --\n\
              # the single most confusing symptom this image can produce, and one\n\
              # that cost a full debugging session to trace back to a missing\n\
              # package. Write the reason INTO the log and onto the console.\n\
              MSG='eclipse-seatd: seatd is NOT INSTALLED -- the whole Wayland\n\
              session (labwc, lunarbg, lunarbar) cannot start without it. Fix:\n\
              apk add seatd labwc  (needs network at image-build time).'\n\
              echo \"$MSG\" >>\"$LOG\" 2>/dev/null || true\n\
              echo \"$MSG\" > /dev/console 2>/dev/null || true\n\
              echo \"$MSG\" >&2\n\
              # Back off HARD rather than every 5 s: a missing package is not\n\
              # going to appear on its own, and the tight respawn loop it caused\n\
              # (seatd 100x, labwc 67x per run) is pure fork/exec churn that\n\
              # buys nothing and stresses the kernel for no reason.\n\
              sleep 60\n\
              exit 127\n",
        )
        .unwrap();

        // Wait for labwc's Wayland socket, then exec the native client.
        // Shared by wallpaper + panel: init starts these AFTER labwc.service
        // (see after=), but the socket may still appear a moment later.
        let wait_wayland = "\
              : \"${XDG_RUNTIME_DIR:=/run/user/0}\"; export XDG_RUNTIME_DIR\n\
              [ -d \"$XDG_RUNTIME_DIR\" ] || { mkdir -p \"$XDG_RUNTIME_DIR\" && chmod 0700 \"$XDG_RUNTIME_DIR\"; }\n\
              export PATH=/usr/local/bin:/bin:/sbin:/usr/bin:/usr/sbin\n\
              export LANG=\"${LANG:-C.UTF-8}\"\n\
              i=0\n\
              while [ \"$i\" -lt 100 ]; do\n\
              \x20 for s in \"$XDG_RUNTIME_DIR\"/wayland-[0-9]*; do\n\
              \x20   [ -S \"$s\" ] || continue\n\
              \x20   WAYLAND_DISPLAY=$(basename \"$s\"); export WAYLAND_DISPLAY\n\
              \x20   break 2\n\
              \x20 done\n\
              \x20 sleep 0.1; i=$((i+1))\n\
              done\n\
              if [ -z \"${WAYLAND_DISPLAY:-}\" ]; then\n\
              \x20 echo \"$0: no wayland socket under $XDG_RUNTIME_DIR yet\" >&2\n\
              \x20 # No compositor. If labwc is not even installed, this can never\n\
              \x20 # succeed, so back off hard instead of respawning every ~15 s:\n\
              \x20 # the client is healthy, its compositor is simply absent.\n\
              \x20 for d in /usr/bin /bin /usr/sbin /sbin; do\n\
              \x20 \x20 [ -x \"$d/labwc\" ] && { sleep 2; exit 1; }\n\
              \x20 done\n\
              \x20 echo \"$0: labwc is not installed either -- backing off\" >&2\n\
              \x20 sleep 60; exit 1\n\
              fi\n";

        fs::write(
            localbin.join("eclipse-lunarbg"),
            format!(
                "#!/bin/sh\n\
                 # Eclipse OS: wallpaper client for eclipse-init (not labwc autostart).\n\
                 LOG=/tmp/lunarbg.log\n\
                 exec >>\"$LOG\" 2>&1\n\
                 {wait}\
                 command -v lunarbg >/dev/null 2>&1 || {{ echo 'eclipse-lunarbg: lunarbg missing'; sleep 5; exit 127; }}\n\
                 echo \"[eclipse-lunarbg] WAYLAND_DISPLAY=$WAYLAND_DISPLAY\"\n\
                 # labwc writes LUNARBG_ASPECT into its environment file, but\n\
                 # this client is started by eclipse-init — not as a labwc\n\
                 # child — so re-export a default for panels without EDID mm.\n\
                 export LUNARBG_ASPECT=\"${{LUNARBG_ASPECT:-16:9}}\"\n\
                 exec lunarbg --fps \"${{LUNARBG_FPS:-8}}\"\n",
                wait = wait_wayland
            )
            .as_bytes(),
        )
        .unwrap();
        fs::write(
            localbin.join("eclipse-lunarbar"),
            format!(
                "#!/bin/sh\n\
                 # Eclipse OS: panel client for eclipse-init (not labwc autostart).\n\
                 LOG=/tmp/lunarbar.log\n\
                 exec >>\"$LOG\" 2>&1\n\
                 {wait}\
                 command -v lunarbar >/dev/null 2>&1 || {{ echo 'eclipse-lunarbar: lunarbar missing'; sleep 5; exit 127; }}\n\
                 echo \"[eclipse-lunarbar] WAYLAND_DISPLAY=$WAYLAND_DISPLAY\"\n\
                 exec lunarbar\n",
                wait = wait_wayland
            )
            .as_bytes(),
        )
        .unwrap();

        fs::write(
            localbin.join("eclipse-xorg"),
            b"#!/bin/sh\n\
              # Eclipse OS: start the Xorg session (fbdev on /dev/fb0) for\n\
              # eclipse-init. startx reads /root/.xserverrc (which execs X with\n\
              # -ac) and /root/.xinitrc (the session: XFCE or a WM+terminal).\n\
              # It blocks until the session ends; init then respawns us.\n\
              export HOME=/root\n\
              : \"${XDG_RUNTIME_DIR:=/run/user/0}\"; export XDG_RUNTIME_DIR\n\
              [ -d \"$XDG_RUNTIME_DIR\" ] || { mkdir -p \"$XDG_RUNTIME_DIR\" && chmod 0700 \"$XDG_RUNTIME_DIR\"; }\n\
              if ! command -v startx >/dev/null 2>&1; then\n\
              \x20 echo 'eclipse-xorg: startx not found (apk add xinit xorg-server xf86-video-fbdev)' >&2\n\
              \x20 sleep 5; exit 127\n\
              fi\n\
              # NOTE: X is kept off DRM/card0 by the KERNEL, which does not create\n\
              # /dev/dri/card0 when the cmdline selects desktop=xorg (Xorg's\n\
              # platform probe of card0 hangs on this kernel's software-KMS).\n\
              # See linux-object fs/mod.rs. Nothing to do here.\n\
              # vt1: X takes the first VT directly (no udev/logind seat here).\n\
              exec startx -- vt1\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            for w in [
                "eclipse-udhcpc",
                "eclipse-seatd",
                "eclipse-xorg",
                "eclipse-lunarbg",
                "eclipse-lunarbar",
            ] {
                let _ = fs::set_permissions(
                    localbin.join(w),
                    fs::Permissions::from_mode(0o755),
                );
            }
        }

        // Default desktop selector: labwc, the hardware default. A boot with
        // `desktop=xorg` on the kernel cmdline (see `make qemu`) overrides this;
        // editing this file changes the default persistently. See
        // eclipse-init's `selected_desktop`.
        let eclipse_etc = rootfs.join("etc").join("eclipse");
        let _ = fs::create_dir_all(&eclipse_etc);
        fs::write(eclipse_etc.join("desktop"), b"labwc\n").unwrap();

        println!("Installed eclipse-init as PID 1 with udhcpc, seatd, labwc and xorg services.");
        true
    }

    /// 从安装目录拷贝所有 so 和 so 链接到 rootfs
    fn put_libs(&self, musl: impl AsRef<Path>, dir: impl AsRef<Path>) {
        let lib = self.path().join("lib");
        let musl_libc_protected = format!("ld-musl-{}.so.1", self.0.name());
        let musl_libc_ignored = "libc.so";
        let strip = self.strip(musl);
        dir.as_ref()
            .join("lib")
            .read_dir()
            .unwrap()
            .filter_map(|res| res.map(|e| e.path()).ok())
            .filter(|path| check_so(path))
            .for_each(|source| {
                let name = source.file_name().unwrap();
                let target = lib.join(name);
                if source.is_symlink() {
                    if name != musl_libc_protected.as_str() {
                        dir::rm(&target).unwrap();
                        // `fs::copy` 会拷贝文件内容
                        unix::fs::symlink(source.read_link().unwrap(), target).unwrap();
                    }
                } else if name != musl_libc_ignored {
                    dir::rm(&target).unwrap();
                    fs::copy(source, &target).unwrap();
                    Ext::new(&strip).arg("-s").arg(target).status();
                }
            });
    }
}

/// 为 PATH 环境变量附加路径。
fn join_path_env<I, S>(paths: I) -> OsString
where
    I: IntoIterator<Item = S>,
    S: AsRef<Path>,
{
    let mut path = OsString::new();
    let mut first = true;
    if let Ok(current) = env::var("PATH") {
        path.push(current);
        first = false;
    }
    for item in paths {
        if first {
            first = false;
        } else {
            path.push(":");
        }
        path.push(item.as_ref().canonicalize().unwrap().as_os_str());
    }
    path
}

/// 判断一个文件是动态库或动态库的符号链接。
fn check_so<P: AsRef<Path>>(path: P) -> bool {
    let path = path.as_ref();
    // 是符号链接或文件
    // 对于符号链接，`is_file` `exist` 等函数都会针对其指向的真实文件判断
    if !path.is_symlink() && !path.is_file() {
        return false;
    }
    // 对文件名分段
    let name = path.file_name().unwrap().to_string_lossy();
    let mut seg = name.split('.');
    // 不能以 . 开头
    if matches!(seg.next(), Some("") | None) {
        return false;
    }
    // 扩展名的第一项是 so
    if !matches!(seg.next(), Some("so")) {
        return false;
    }
    // so 之后全是纯十进制数字
    !seg.any(|it| !it.chars().all(|ch| ch.is_ascii_digit()))
}
