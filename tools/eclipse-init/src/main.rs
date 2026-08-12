//! Eclipse OS init — a small, purpose-built PID 1 / service supervisor.
//!
//! Eclipse's kernel already mounts the root, brings up the network and spawns
//! the per-VT shells, so init does NOT need a heavyweight, shell-driven service
//! manager (OpenRC's per-step `busybox sh` fork/exec churn is exactly what
//! stressed the kernel's fragile paths). This init does only what PID 1 must:
//!
//!   * reap orphaned children forever (the defining duty of PID 1),
//!   * mount any pseudo-filesystems that are missing (idempotent, best-effort),
//!   * launch the userspace declared in `/etc/eclipse/services/*.service`
//!     (`oneshot` tasks run to completion in order; `respawn` services are
//!     supervised and restarted if they exit),
//!   * shut the system down cleanly on SIGTERM (halt) / SIGINT (reboot).
//!
//! Design borrowed from runit/s6/dinit (supervision, declarative services,
//! dependency ordering); implementation is our own so every syscall is under
//! our control on the still-maturing kernel. No shell is involved.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// A respawn service that exits sooner than this after starting is treated as
/// "crashing", not "finished a unit of work", and its restart is delayed.
const HEALTHY_UPTIME: Duration = Duration::from_secs(2);
/// Restart delay for a crashing service: starts here and doubles up to
/// [`MAX_BACKOFF`]. Without it, a service whose binary is missing or that dies
/// on start (labwc before the GPU is ready, udhcpc on a link with no DHCP)
/// would fork/exec at full speed forever, pinning a CPU.
const MIN_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(8);

/// Set by the SIGTERM handler: bring the system down (halt/power off).
static WANT_HALT: AtomicBool = AtomicBool::new(false);
/// Set by the SIGINT handler (Ctrl-Alt-Del is delivered to PID 1 as SIGINT):
/// bring the system down and reboot.
static WANT_REBOOT: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigterm(_sig: libc::c_int) {
    WANT_HALT.store(true, Ordering::SeqCst);
}
extern "C" fn on_sigint(_sig: libc::c_int) {
    WANT_REBOOT.store(true, Ordering::SeqCst);
}

/// How a service is managed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Run once to completion during boot (mounts, one-time setup).
    Oneshot,
    /// Long-running; supervised and restarted if it exits.
    Respawn,
}

struct Service {
    name: String,
    /// argv (argv[0] is the absolute program path).
    exec: Vec<String>,
    kind: Kind,
    /// Names of services that must be started before this one.
    after: Vec<String>,
    /// Unix socket path to wait for (bounded) before every start of this
    /// service — `after =` only orders the FORK of the dependency, not its
    /// readiness. Waiting natively here (a 10 ms stat poll) replaced the
    /// wrappers' `sleep 0.1`-per-iteration shell loops, which forked a busybox
    /// per poll exactly while the compositor was busy demand-paging itself.
    wait_socket: Option<String>,
    /// Desktop session this service belongs to (`labwc` or `xorg`). `None` means
    /// session-agnostic (always started). A tagged service starts only when the
    /// selected desktop (see [`selected_desktop`]) matches, so the same image
    /// boots labwc on hardware and Xorg under `make qemu` purely from a
    /// `desktop=` boot argument.
    desktop: Option<String>,
    /// If set, child stdout/stderr append here instead of `/dev/null`.
    log: Option<String>,
    /// Live child pid for a running `respawn` service.
    pid: Option<i32>,
    /// When the current child was last started (for crash-loop backoff).
    started_at: Option<Instant>,
    /// Current restart delay for a crashing respawn service; grows on repeated
    /// fast exits, resets once the service stays up past [`HEALTHY_UPTIME`].
    backoff: Duration,
}

/// Default environment handed to every service (and inherited by their
/// children). Includes the Wayland session vars that `/usr/local/bin/labwc`
/// used to inject: init execs the real `/usr/bin/labwc` (no ash wrapper), so
/// those vars must live here or wlroots falls back to GLES2 / empty GPU enum.
const CHILD_ENV: &[&str] = &[
    "PATH=/usr/local/bin:/bin:/sbin:/usr/bin:/usr/sbin",
    "HOME=/root",
    "TERM=xterm-256color",
    "LANG=C.UTF-8",
    "XDG_RUNTIME_DIR=/run/user/0",
    "XDG_CONFIG_HOME=/root/.config",
    "XCURSOR_THEME=Adwaita",
    "XCURSOR_SIZE=24",
    "WLR_RENDERER=pixman",
    "WLR_RENDERER_ALLOW_SOFTWARE=1",
    "WLR_BACKENDS=drm,libinput",
    "WLR_DRM_DEVICES=/dev/dri/card0",
    "WLR_LIBINPUT_NO_DEVICES=1",
];

fn log(msg: &str) {
    // PID 1 has stdout/stderr wired to the console by the kernel.
    println!("[eclipse-init] {msg}");
}

fn main() {
    log("starting");

    mount_pseudo_filesystems();
    install_signal_handlers();

    let mut services = load_services(Path::new("/etc/eclipse/services"));

    // Pick the desktop session and drop every service tagged for a different
    // one, so only the selected compositor/X stack is supervised.
    let desktop = selected_desktop();
    log(&format!("desktop session: {desktop}"));
    services.retain(|_, s| s.desktop.as_deref().map_or(true, |d| d == desktop));

    let order = ordered_names(&services);

    for name in &order {
        // Re-check shutdown between starts so a SIGTERM during boot is honoured.
        if WANT_HALT.load(Ordering::SeqCst) || WANT_REBOOT.load(Ordering::SeqCst) {
            break;
        }
        start_service(services.get_mut(name).expect("known service"));
    }

    log("entering supervision loop");
    supervise(&mut services);
}

// ---------------------------------------------------------------------------
// Pseudo-filesystems
// ---------------------------------------------------------------------------

/// Mount the standard pseudo-filesystems if they are not already present. The
/// Eclipse kernel already provides procfs/sysfs/devfs and treats these mounts
/// as successful no-ops, so this is cheap and idempotent; it is here so the
/// system is correct even on a kernel build where a mount point is empty.
fn mount_pseudo_filesystems() {
    // (source, target, fstype)
    let mounts = [
        ("proc", "/proc", "proc"),
        ("sysfs", "/sys", "sysfs"),
        ("devtmpfs", "/dev", "devtmpfs"),
        ("tmpfs", "/run", "tmpfs"),
        ("tmpfs", "/tmp", "tmpfs"),
    ];
    for (src, target, fstype) in mounts {
        if !Path::new(target).exists() {
            let _ = fs::create_dir_all(target);
        }
        let c_src = CString::new(src).unwrap();
        let c_target = CString::new(target).unwrap();
        let c_fstype = CString::new(fstype).unwrap();
        // SAFETY: all pointers are valid NUL-terminated strings; data is null.
        let rc = unsafe {
            libc::mount(
                c_src.as_ptr(),
                c_target.as_ptr(),
                c_fstype.as_ptr(),
                0,
                core::ptr::null(),
            )
        };
        if rc != 0 {
            // Already mounted / kernel-provided: not fatal.
            log(&format!("note: mount {fstype} on {target} skipped"));
        }
    }

    // The Eclipse kernel treats the tmpfs mounts above as successful NO-OPS,
    // so on an installed root /run and /tmp are btrfs directories that SURVIVE
    // reboots. Stale sockets from the previous boot (`/run/seatd.sock`,
    // `wayland-0`) then pass the wrappers' `[ -S ]`/wait checks before the
    // daemons are actually listening — clients connect to a dead socket, exit,
    // and burn respawn backoffs; seatd/wlroots may also refuse to bind over a
    // pre-existing path. Clear both trees before any service starts. On a real
    // tmpfs (or the live RAM image) they are already empty and this is a no-op.
    clean_runtime_dir(Path::new("/run"));
    clean_runtime_dir(Path::new("/tmp"));

    // Wayland compositor socket dir (matches CHILD_ENV XDG_RUNTIME_DIR).
    let xdg_run = Path::new("/run/user/0");
    if !xdg_run.exists() {
        let _ = fs::create_dir_all(xdg_run);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(xdg_run, fs::Permissions::from_mode(0o700));
        }
    }
}

/// Best-effort removal of everything INSIDE `dir` (the directory itself
/// stays). Symlinks are removed as entries, never followed.
fn clean_runtime_dir(dir: &Path) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut removed = 0u32;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_real_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let ok = if is_real_dir {
            fs::remove_dir_all(&path).is_ok()
        } else {
            fs::remove_file(&path).is_ok()
        };
        if ok {
            removed += 1;
        }
    }
    if removed > 0 {
        log(&format!(
            "cleared {removed} stale entr{} under {}",
            if removed == 1 { "y" } else { "ies" },
            dir.display()
        ));
    }
}

// ---------------------------------------------------------------------------
// Signals
// ---------------------------------------------------------------------------

fn install_signal_handlers() {
    install_handler(libc::SIGTERM, on_sigterm as usize);
    install_handler(libc::SIGINT, on_sigint as usize);
    // SIGCHLD is left at its default: the blocking `waitpid` in the supervision
    // loop reaps children directly, so no handler is needed for reaping.
}

fn install_handler(sig: libc::c_int, handler: usize) {
    // SAFETY: zeroed sigaction with a valid handler pointer; standard install.
    unsafe {
        let mut sa: libc::sigaction = core::mem::zeroed();
        sa.sa_sigaction = handler;
        libc::sigemptyset(&mut sa.sa_mask);
        // No SA_RESTART: we WANT `waitpid` to return EINTR so the loop notices
        // the shutdown flag promptly.
        sa.sa_flags = 0;
        libc::sigaction(sig, &sa, core::ptr::null_mut());
    }
}

// ---------------------------------------------------------------------------
// Desktop session selection
// ---------------------------------------------------------------------------

/// Which desktop session to start. Resolution order, first hit wins:
///   1. a `desktop=<name>` token on the kernel command line (`/proc/cmdline`) —
///      this is how `make qemu` selects Xorg while the same image, booted on
///      real hardware with the installed cmdline, gets none and falls through;
///   2. the `/etc/eclipse/desktop` file (first whitespace token) — a persistent
///      per-install override the user can edit;
///   3. `labwc` — the default Eclipse session.
fn selected_desktop() -> String {
    if let Some(d) = cmdline_desktop() {
        return d;
    }
    if let Ok(text) = fs::read_to_string("/etc/eclipse/desktop") {
        if let Some(tok) = text.split_whitespace().next() {
            if !tok.is_empty() {
                return tok.to_string();
            }
        }
    }
    String::from("labwc")
}

/// Extract `desktop=<name>` from `/proc/cmdline`. The Eclipse kernel joins boot
/// arguments with `:` (e.g. `LOG=error:ROOT=/dev/vda:desktop=xorg`), but a plain
/// space-separated cmdline works too — split on both.
fn cmdline_desktop() -> Option<String> {
    let cmdline = fs::read_to_string("/proc/cmdline").ok()?;
    cmdline
        .split(|c: char| c == ':' || c.is_whitespace())
        .find_map(|tok| tok.strip_prefix("desktop="))
        .filter(|d| !d.is_empty())
        .map(String::from)
}

// ---------------------------------------------------------------------------
// Service files
// ---------------------------------------------------------------------------

/// Parse every `*.service` file in `dir` into a map keyed by service name (the
/// file stem). Malformed or empty (no `exec`) files are skipped with a warning
/// rather than aborting boot.
fn load_services(dir: &Path) -> BTreeMap<String, Service> {
    let mut out = BTreeMap::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => {
            log(&format!("no service directory {} (nothing to start)", dir.display()));
            return out;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("service") {
            continue;
        }
        let name = match path.file_stem().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => {
                log(&format!("warning: cannot read {}", path.display()));
                continue;
            }
        };
        match parse_service(&name, &text) {
            Some(svc) => {
                out.insert(name, svc);
            }
            None => log(&format!("warning: {} has no 'exec', skipped", path.display())),
        }
    }
    out
}

/// Parse a single service file. Format is line-based `key = value`, `#`
/// comments and blank lines ignored:
///   exec    = /usr/sbin/foo --flag    (required; whitespace-split into argv)
///   type    = respawn | oneshot       (default: oneshot)
///   after   = bar baz                 (optional; space-separated dep names)
///   desktop = labwc | xorg            (optional; only start under that session)
///   log     = /tmp/foo.log            (optional; capture child stdout/stderr)
fn parse_service(name: &str, text: &str) -> Option<Service> {
    let mut exec: Vec<String> = Vec::new();
    let mut kind = Kind::Oneshot;
    let mut after: Vec<String> = Vec::new();
    let mut desktop: Option<String> = None;
    let mut log_path: Option<String> = None;
    let mut wait_socket: Option<String> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once('=') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => continue,
        };
        match key {
            "exec" => exec = value.split_whitespace().map(String::from).collect(),
            "type" => {
                kind = match value {
                    "respawn" => Kind::Respawn,
                    _ => Kind::Oneshot,
                }
            }
            "after" => after = value.split_whitespace().map(String::from).collect(),
            "desktop" => desktop = Some(value.to_string()),
            "log" => log_path = Some(value.to_string()),
            "wait_socket" => wait_socket = Some(value.to_string()),
            _ => {}
        }
    }

    if exec.is_empty() {
        return None;
    }
    Some(Service {
        name: name.to_string(),
        exec,
        kind,
        after,
        desktop,
        log: log_path,
        wait_socket,
        pid: None,
        started_at: None,
        backoff: MIN_BACKOFF,
    })
}

/// Produce a start order honouring `after =` dependencies: a service is only
/// emitted once every dependency it lists has been emitted. Remaining services
/// (missing deps or dependency cycles) are appended in name order so a bad
/// `after =` never wedges boot.
fn ordered_names(services: &BTreeMap<String, Service>) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    let mut pending: Vec<String> = services.keys().cloned().collect();

    loop {
        let mut progressed = false;
        let mut still_pending: Vec<String> = Vec::new();
        for name in pending {
            let deps = &services[&name].after;
            let ready = deps
                .iter()
                // A dep that doesn't exist can never be satisfied: ignore it
                // (treat as already-met) rather than deadlock.
                .all(|d| !services.contains_key(d) || order.contains(d));
            if ready {
                order.push(name);
                progressed = true;
            } else {
                still_pending.push(name);
            }
        }
        pending = still_pending;
        if pending.is_empty() {
            break;
        }
        if !progressed {
            // Cycle or unsatisfiable deps: emit the rest in name order.
            pending.sort();
            order.extend(pending);
            break;
        }
    }
    order
}

// ---------------------------------------------------------------------------
// Launching & supervision
// ---------------------------------------------------------------------------

/// Start a service. `oneshot` runs to completion (blocking) before returning;
/// `respawn` is forked and its pid recorded for the supervision loop.
fn start_service(svc: &mut Service) {
    // `after =` only orders the dependency's FORK; its socket may lag. Wait
    // natively here (both first start and crash-restarts pass through) so the
    // service doesn't die before its dependency is ready — and so no shell
    // wrapper has to fork a `sleep 0.1` busybox per poll instead. labwc keeps
    // its historical seatd wait even if its service file lacks the key.
    let wait = svc
        .wait_socket
        .clone()
        .or_else(|| (svc.name == "labwc").then(|| String::from("/run/seatd.sock")));
    if let Some(path) = wait {
        wait_for_socket(&path, Duration::from_secs(10));
    }
    match svc.kind {
        Kind::Oneshot => {
            log(&format!("oneshot: {}", svc.name));
            if let Some(pid) = spawn(&svc.exec, svc.log.as_deref()) {
                // Wait specifically for this child to finish.
                let mut status = 0;
                // SAFETY: pid is a child of ours.
                unsafe { libc::waitpid(pid, &mut status, 0) };
            }
        }
        Kind::Respawn => {
            log(&format!("respawn: {} (starting)", svc.name));
            svc.pid = spawn(&svc.exec, svc.log.as_deref());
            svc.started_at = Some(Instant::now());
        }
    }
}

/// Poll until `path` is a socket or `timeout` elapses (best-effort).
///
/// Fine-grained at first (10 ms) so the common case — seatd binds its socket
/// a few tens of ms after forking — releases the dependent service almost
/// immediately instead of rounding the wait up to a 100 ms slot; backs off to
/// 100 ms after the first second so a missing daemon costs no busy churn.
fn wait_for_socket(path: &str, timeout: Duration) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if is_unix_socket(path) {
            return;
        }
        let step = if start.elapsed() < Duration::from_secs(1) {
            Duration::from_millis(10)
        } else {
            Duration::from_millis(100)
        };
        std::thread::sleep(step);
    }
    log(&format!("warning: {path} not ready after {timeout:?}"));
}

fn is_unix_socket(path: &str) -> bool {
    let c_path = match CString::new(path) {
        Ok(p) => p,
        Err(_) => return false,
    };
    // SAFETY: path is a valid C string; st is stack-allocated.
    unsafe {
        let mut st: libc::stat = core::mem::zeroed();
        if libc::stat(c_path.as_ptr(), &mut st) != 0 {
            return false;
        }
        (st.st_mode & libc::S_IFMT) == libc::S_IFSOCK
    }
}

/// fork + execv the given argv. Returns the child pid in the parent, or `None`
/// if the fork failed. In the child, signal dispositions are reset to default
/// and a fresh session is started before exec.
/// Sleep for `d`, returning early if a signal (a shutdown request) interrupts
/// it. `nanosleep` returns EINTR on a delivered signal, which is exactly what
/// lets a Ctrl-Alt-Del during a service backoff bring the system down promptly.
// `libc::time_t` is a deprecated alias (musl 1.2 widened it) but it is still the
// exact field type of `libc::timespec`, so the cast requires it; the value fits
// regardless of width.
#[allow(deprecated)]
fn sleep_interruptible(d: Duration) {
    let req = libc::timespec {
        tv_sec: d.as_secs() as libc::time_t,
        tv_nsec: d.subsec_nanos() as libc::c_long,
    };
    // SAFETY: nanosleep with a valid timespec and a null remainder pointer.
    unsafe {
        libc::nanosleep(&req, core::ptr::null_mut());
    }
}

fn spawn(argv: &[String], log_path: Option<&str>) -> Option<i32> {
    let prog = CString::new(argv[0].as_str()).ok()?;
    let c_args: Vec<CString> = argv
        .iter()
        .map(|a| CString::new(a.as_str()).unwrap())
        .collect();
    let mut p_args: Vec<*const libc::c_char> = c_args.iter().map(|a| a.as_ptr()).collect();
    p_args.push(core::ptr::null());

    let c_env: Vec<CString> = CHILD_ENV
        .iter()
        .map(|e| CString::new(*e).unwrap())
        .collect();
    let mut p_env: Vec<*const libc::c_char> = c_env.iter().map(|e| e.as_ptr()).collect();
    p_env.push(core::ptr::null());

    // SAFETY: standard fork/exec. The child only calls async-signal-safe libc
    // functions (signal reset, setsid, open/dup2, execve) before exec.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        log(&format!("error: fork failed for {}", argv[0]));
        return None;
    }
    if pid == 0 {
        unsafe {
            // Reset signals to default so the child isn't born with init's
            // handlers, and give it its own session/process group.
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::setsid();
            // Detach from the console: stdin → /dev/null; stdout/stderr →
            // optional log file or /dev/null so service chatter never hits
            // the screen. init keeps the real console for its own lines.
            silence_stdio(log_path);
            libc::execve(prog.as_ptr(), p_args.as_ptr(), p_env.as_ptr());
            // execve only returns on failure.
            libc::_exit(127);
        }
    }
    Some(pid)
}

/// Redirect fds 0/1/2. stdin always `/dev/null`; stdout/stderr go to `log_path`
/// (append, create) when set, otherwise `/dev/null`. Async-signal-safe
/// (`open`/`dup2`/`close`); failures are ignored.
unsafe fn silence_stdio(log_path: Option<&str>) {
    let devnull = b"/dev/null\0";
    let null_fd = libc::open(devnull.as_ptr() as *const libc::c_char, libc::O_RDWR);
    if null_fd >= 0 {
        libc::dup2(null_fd, libc::STDIN_FILENO);
        if null_fd > libc::STDERR_FILENO {
            libc::close(null_fd);
        }
    }

    let out_fd = if let Some(path) = log_path {
        if let Ok(c_path) = CString::new(path) {
            libc::open(
                c_path.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND,
                0o644,
            )
        } else {
            -1
        }
    } else {
        -1
    };
    let out_fd = if out_fd >= 0 {
        out_fd
    } else {
        libc::open(devnull.as_ptr() as *const libc::c_char, libc::O_RDWR)
    };
    if out_fd >= 0 {
        libc::dup2(out_fd, libc::STDOUT_FILENO);
        libc::dup2(out_fd, libc::STDERR_FILENO);
        if out_fd > libc::STDERR_FILENO {
            libc::close(out_fd);
        }
    }
}

/// The PID 1 main loop: block in `waitpid`, reaping every child. A reaped
/// `respawn` service is restarted; orphans reparented to init are simply
/// reaped. A pending shutdown/reboot signal breaks out to `shutdown`.
fn supervise(services: &mut BTreeMap<String, Service>) {
    loop {
        if WANT_HALT.load(Ordering::SeqCst) {
            return shutdown(false, services);
        }
        if WANT_REBOOT.load(Ordering::SeqCst) {
            return shutdown(true, services);
        }

        let mut status = 0;
        // SAFETY: blocking wait for any child.
        let pid = unsafe { libc::waitpid(-1, &mut status, 0) };
        if pid < 0 {
            let err = errno();
            if err == libc::EINTR {
                // A signal arrived; loop to re-check the shutdown flags.
                continue;
            }
            if err == libc::ECHILD {
                // No children to wait on: pause until the next signal so we are
                // not a busy loop. Returns on EINTR (a delivered signal).
                unsafe { libc::pause() };
                continue;
            }
            // Unexpected: avoid spinning.
            unsafe { libc::pause() };
            continue;
        }

        // Did a supervised respawn service just exit? Decide its restart delay,
        // then clear its pid; a single restart pass below respawns it. Splitting
        // "decide" from "restart" keeps the mutable borrow off the sleep.
        let mut delay = Duration::ZERO;
        if let Some(svc) = services.values_mut().find(|s| s.pid == Some(pid)) {
            let uptime = svc.started_at.map(|t| t.elapsed()).unwrap_or_default();
            svc.pid = None;
            if uptime >= HEALTHY_UPTIME {
                // Up long enough to be healthy: restart now, reset the backoff.
                svc.backoff = MIN_BACKOFF;
                log(&format!("respawn: {} exited after {:?}, restarting", svc.name, uptime));
            } else {
                // Exited almost immediately: back off so a broken or
                // not-yet-ready service cannot pin a CPU.
                delay = svc.backoff;
                svc.backoff = (svc.backoff * 2).min(MAX_BACKOFF);
                log(&format!(
                    "respawn: {} exited after {:?} (crash), retry in {:?}",
                    svc.name, uptime, delay
                ));
            }
        }
        // Otherwise it was a oneshot's leftover or a reparented orphan: reaped.
        if !delay.is_zero() {
            // Interruptible by a shutdown signal; if one arrived, honour it
            // instead of respawning. Otherwise fall through to the restart pass
            // (NOT `continue`: with no other children the next waitpid would
            // ECHILD-pause and the backed-off service would never come back).
            sleep_interruptible(delay);
            if WANT_HALT.load(Ordering::SeqCst) || WANT_REBOOT.load(Ordering::SeqCst) {
                continue;
            }
        }
        // Restart pass: any respawn service now without a live pid is respawned.
        for svc in services.values_mut() {
            if svc.kind == Kind::Respawn && svc.pid.is_none() {
                svc.pid = spawn(&svc.exec, svc.log.as_deref());
                svc.started_at = Some(Instant::now());
            }
        }
    }
}

/// Stop everything and ask the kernel to reboot (`reboot == true`) or power
/// off. Best-effort: SIGTERM to all, a short grace period, SIGKILL, sync, then
/// the reboot syscall. If the kernel cannot reboot, halt in a pause loop.
fn shutdown(reboot: bool, _services: &mut BTreeMap<String, Service>) {
    log(if reboot { "rebooting" } else { "powering off" });

    unsafe {
        // Politely ask every process to terminate, then force it.
        libc::kill(-1, libc::SIGTERM);
        sleep_secs(2);
        libc::kill(-1, libc::SIGKILL);
        libc::sync();

        let cmd = if reboot {
            libc::RB_AUTOBOOT
        } else {
            libc::RB_POWER_OFF
        };
        libc::reboot(cmd);
        // reboot(2) failed (ENOSYS or denied): nothing left to do but idle.
        log("reboot syscall returned; halting");
        loop {
            libc::pause();
        }
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn errno() -> libc::c_int {
    // SAFETY: __errno_location returns a valid pointer on musl/glibc.
    unsafe { *libc::__errno_location() }
}

fn sleep_secs(secs: u64) {
    let ts = libc::timespec {
        tv_sec: secs as libc::time_t,
        tv_nsec: 0,
    };
    // SAFETY: valid timespec; null remainder.
    unsafe { libc::nanosleep(&ts, core::ptr::null_mut()) };
}
