//! # hunter — kernel security subsystem for Eclipse OS
//!
//! `hunter` is an in-kernel **security solution** combining an **LSM-style
//! enforcement layer** with a **behavioural intrusion-detection system**. The
//! kernel calls a handful of well-defined hooks; hunter consults its [`policy`]
//! engine, runs [`heuristics`], tracks memory protections in [`wx`], and records
//! every decision in a tamper-evident [`event_log`] readable at `/proc/hunter`.
//!
//! ## Hooks
//!
//! | Hook                  | Kernel call site            | Domain          |
//! |-----------------------|-----------------------------|-----------------|
//! | [`check_syscall`]     | syscall dispatch            | seccomp + IDS   |
//! | [`check_elf_binary`]  | `execve` / loader / interp  | exec integrity  |
//! | [`check_mmap`]        | `mmap`                      | W^X memory      |
//! | [`check_mprotect`]    | `mprotect`                  | W^X memory      |
//! | [`check_munmap`]      | `munmap`                    | W^X bookkeeping |
//! | [`task_fork`]         | `fork` / `clone`            | policy lifecycle|
//! | [`task_exec`]         | `execve`                    | policy lifecycle|
//! | [`task_exit`]         | process teardown            | state cleanup   |
//!
//! Each enforcement domain has an independent [`policy::Mode`] (`Off` /
//! `Report` / `Enforce`) and the control plane can be sealed tighten-only.

#![no_std]

extern crate alloc;
#[macro_use]
extern crate log;

pub mod clock;
pub mod event_log;
pub mod heuristics;
pub mod policy;
pub mod wx;

use alloc::format;
use alloc::string::String;

pub use clock::set_time_source;
pub use event_log::{set_sink, Severity};
pub use policy::Mode;

/// Reasons hunter rejects an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityViolation {
    /// A syscall was not on the calling process's whitelist, or was denied by
    /// the anomaly enforcer.
    SyscallBlocked,
    /// An ELF binary failed integrity or path policy checks.
    InvalidBinary,
    /// A memory mapping requested write+execute (W^X).
    WriteExecViolation,
}

/// Expected ELF `e_machine` for the build target (`None` if unknown).
const EXPECTED_MACHINE: Option<u16> = {
    #[cfg(target_arch = "x86_64")]
    {
        Some(62)
    }
    #[cfg(target_arch = "aarch64")]
    {
        Some(183)
    }
    #[cfg(target_arch = "riscv64")]
    {
        Some(243)
    }
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )))]
    {
        None
    }
};

/// Tracks whether [`init`] has already run, so the multiple boot call sites
/// (bare-metal `main` and the loader's first spawn) log the banner only once.
static INITIALIZED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Initializes the hunter security subsystem. Idempotent: safe to call from
/// more than one boot path. Call *after* [`set_time_source`] so the first event
/// carries a real timestamp.
pub fn init() {
    if INITIALIZED.swap(true, core::sync::atomic::Ordering::SeqCst) {
        return;
    }
    event_log::record(
        0,
        Severity::Info,
        "SYSTEM",
        "INIT",
        format!(
            "hunter security subsystem v{} initialized (syscall={}, wx={}, exec={}, anomaly={})",
            env!("CARGO_PKG_VERSION"),
            policy::syscall_mode().as_str(),
            policy::wx_mode().as_str(),
            policy::exec_mode().as_str(),
            policy::anomaly_mode().as_str(),
        ),
    );
    info!(
        "hunter: security subsystem v{} online",
        env!("CARGO_PKG_VERSION")
    );
}

/// Syscall hook. Enforces the per-process whitelist and feeds the anomaly
/// detector. Returns `Err` when the call must be denied.
pub fn check_syscall(pid: u64, num: u32, args: &[usize; 6]) -> Result<(), SecurityViolation> {
    match policy::is_syscall_allowed(pid, num) {
        Ok(()) => {}
        Err(enforce) => {
            let desc = format!("unauthorized syscall #{} args={:x?}", num, args);
            if enforce {
                event_log::record(pid, Severity::Critical, "SYSCALL", "BLOCKED", desc.clone());
                warn!("hunter: BLOCKED syscall [pid={}] {}", pid, desc);
                return Err(SecurityViolation::SyscallBlocked);
            }
            event_log::record(pid, Severity::Warning, "SYSCALL", "WARNING", desc.clone());
            warn!("hunter: syscall violation (report) [pid={}] {}", pid, desc);
        }
    }
    // Anomaly detection runs only on permitted calls; it may deny under Enforce.
    if !heuristics::on_syscall(pid, num) {
        return Err(SecurityViolation::SyscallBlocked);
    }
    Ok(())
}

/// `execve` / loader / interpreter hook. Validates ELF integrity and the
/// executable-path policy. Returns `true` if execution may proceed.
pub fn check_elf_binary(path: &str, elf_data: &[u8]) -> bool {
    // A valid executable is either an ELF image or a `#!` script (both of which
    // the loader knows how to run). Anything else is rejected — it would fail
    // to load anyway, and this records the attempt.
    let is_elf = elf_data.len() >= 4 && &elf_data[0..4] == b"\x7fELF";
    let is_script = elf_data.len() >= 2 && &elf_data[0..2] == b"#!";
    if !is_elf && !is_script {
        reject_exec(path, "unrecognized executable format");
        return false;
    }
    // ELF integrity: e_ident + e_type + e_machine when enough header is present.
    // (Scripts get path-policy only — magic validation is inapplicable.)
    if is_elf && elf_data.len() >= 20 {
        let class = elf_data[4];
        let data = elf_data[5];
        let version = elf_data[6];
        if class != 1 && class != 2 {
            reject_exec(path, "bad ELF class");
            return false;
        }
        if data != 1 && data != 2 {
            reject_exec(path, "bad ELF endianness");
            return false;
        }
        if version != 1 {
            reject_exec(path, "bad ELF version");
            return false;
        }
        let rd16 = |lo: usize| -> u16 {
            let a = elf_data[lo] as u16;
            let b = elf_data[lo + 1] as u16;
            if data == 1 {
                a | (b << 8)
            } else {
                (a << 8) | b
            }
        };
        let e_type = rd16(16);
        // ET_EXEC (2) or ET_DYN (3) are the only types we execute.
        if e_type != 2 && e_type != 3 {
            reject_exec(path, "unexpected ELF type");
            return false;
        }
        if let Some(expected) = EXPECTED_MACHINE {
            let e_machine = rd16(18);
            if e_machine != expected {
                reject_exec(path, "foreign ELF machine");
                return false;
            }
        }
    }

    // Path policy: refuse / flag execution from untrusted locations.
    check_exec_path(path)
}

/// Executable-path policy check only (no ELF integrity). Used for dynamic-linker
/// and shebang interpreters, where the target may legitimately be a script and
/// magic validation would be inappropriate (P7). Returns `true` if execution
/// from `path` may proceed under the current [`policy::exec_mode`].
pub fn check_exec_path(path: &str) -> bool {
    let exec_mode = policy::exec_mode();
    if exec_mode == Mode::Off {
        return true;
    }

    // 1. Blacklist — operator-curated hard deny, highest precedence. A
    //    blacklisted program is always blocked while the exec domain is active
    //    (this is the deny half of the "whitelist that never denies" model).
    if policy::is_exec_blacklisted(path) {
        event_log::record(
            0,
            Severity::Critical,
            "EXEC",
            "BLOCKED",
            format!("blacklisted program: {}", path),
        );
        warn!("hunter: BLOCKED blacklisted program: {}", path);
        return false;
    }

    // 2. Learning (trust-on-first-use) — never denies. A safe program (already
    //    proven a valid ELF/script by the caller, not blacklisted, not from a
    //    world-writable location) is auto-added to the allowlist and allowed.
    if policy::exec_learning_enabled() {
        if policy::is_exec_listed(path) {
            return true;
        }
        if !policy::is_world_writable_exec_path(path) {
            if policy::learn_exec_path(path) {
                event_log::record(
                    0,
                    Severity::Notice,
                    "EXEC",
                    "LEARN",
                    format!("learned trusted program: {}", policy::canonicalize(path)),
                );
            }
            return true;
        }
        // World-writable and not yet trusted: fall through to the mode policy.
    }

    // 3. Denylist of untrusted locations + optional deny-by-default allowlist.
    let reason = if policy::is_untrusted_exec_path(path) {
        "untrusted path"
    } else if policy::exec_allowlist_active() && !policy::is_exec_allowed(path) {
        "not on trusted-program allowlist"
    } else {
        return true;
    };
    if exec_mode == Mode::Enforce {
        event_log::record(
            0,
            Severity::Critical,
            "EXEC",
            "BLOCKED",
            format!("blocked exec ({}): {}", reason, path),
        );
        warn!("hunter: BLOCKED exec ({}): {}", reason, path);
        return false;
    }
    event_log::record(
        0,
        Severity::Warning,
        "EXEC",
        "WARNING",
        format!("exec policy violation ({}): {}", reason, path),
    );
    warn!(
        "hunter: exec policy violation ({}, report): {}",
        reason, path
    );
    true
}

fn reject_exec(path: &str, why: &str) {
    event_log::record(
        0,
        Severity::Warning,
        "EXEC",
        "BLOCKED",
        format!("{} for {}", why, path),
    );
    warn!("hunter: rejected binary ({}): {}", why, path);
}

/// W^X pre-map hook for `mmap`: decides on the *immediate* write+execute
/// conjunction using only the requested protections, so a violating mapping is
/// rejected *before* it is created. Returns `true` if the mapping may proceed.
/// The kernel must call [`record_mapping`] with the resolved address afterwards.
pub fn check_mmap(pid: u64, prot_write: bool, prot_exec: bool) -> bool {
    let mode = policy::wx_mode();
    if mode == Mode::Off || !(prot_write && prot_exec) {
        return true;
    }
    wx_violation(pid, mode, "mmap", "writable+executable mapping (W^X)")
}

/// Post-map bookkeeping: records that `[addr, addr+len)` was mapped writable,
/// so a later `mprotect(PROT_EXEC)` over it is recognised as the two-step
/// W-then-X sequence. Call after `mmap` resolves the real address.
pub fn record_mapping(pid: u64, addr: usize, len: usize, prot_write: bool) {
    if prot_write && policy::wx_mode() != Mode::Off {
        wx::record_writable(pid, addr, len);
    }
}

/// W^X hook for `mprotect`. Catches both the immediate write+execute request
/// and execute-over-an-ever-writable-region (the two-step bypass). Call
/// *before* applying the protection change. Returns `true` if it may proceed.
pub fn check_mprotect(
    pid: u64,
    addr: usize,
    len: usize,
    prot_write: bool,
    prot_exec: bool,
) -> bool {
    let mode = policy::wx_mode();
    if mode == Mode::Off {
        return true;
    }
    let immediate = prot_write && prot_exec;
    let w_then_x = prot_exec && wx::is_ever_writable(pid, addr, len);
    if immediate || w_then_x {
        let reason = if immediate {
            "writable+executable mapping (W^X)"
        } else {
            "executable mapping over an ever-writable region (W^X)"
        };
        if !wx_violation(pid, mode, "mprotect", reason) {
            return false;
        }
    }
    // Allowed (or report-only): if this grants WRITE, remember the region.
    if prot_write {
        wx::record_writable(pid, addr, len);
    }
    true
}

/// `munmap` hook: forgets writable-region bookkeeping for the unmapped range.
pub fn check_munmap(pid: u64, addr: usize, len: usize) {
    if policy::wx_mode() != Mode::Off {
        wx::clear_region(pid, addr, len);
    }
}

/// Logs a W^X violation and returns whether the action may proceed (`true` in
/// `Report`, `false` in `Enforce`). Must not be called in `Off`.
fn wx_violation(pid: u64, mode: Mode, op: &str, reason: &str) -> bool {
    if mode == Mode::Enforce {
        event_log::record(
            pid,
            Severity::Critical,
            "WX",
            "BLOCKED",
            format!("{}: blocked {}", op, reason),
        );
        warn!("hunter: BLOCKED W^X {} [pid={}]", op, pid);
        false
    } else {
        event_log::record(
            pid,
            Severity::Warning,
            "WX",
            "WARNING",
            format!("{}: {}", op, reason),
        );
        true
    }
}

/// Fork/clone hook: inherits the parent's whitelist into the child and seeds a
/// fresh anomaly window, so a process cannot shed policy or counters by forking.
pub fn task_fork(parent_pid: u64, child_pid: u64) {
    policy::inherit_policy(parent_pid, child_pid);
    heuristics::on_exec(child_pid);
}

/// Exec hook: re-applies any default whitelist to the new image and resets the
/// anomaly window so a benign-then-malicious exec cannot launder counters.
pub fn task_exec(pid: u64, _path: &str) {
    policy::apply_default_policy(pid);
    heuristics::on_exec(pid);
}

/// Process-exit hook. Releases all per-process security state so a recycled pid
/// never inherits a stale policy, anomaly counters, or W^X intervals.
pub fn task_exit(pid: u64) {
    policy::remove_policy(pid);
    heuristics::forget(pid);
    wx::forget(pid);
}

/// Renders the full `/proc/hunter` report: status header + recent event ring.
pub fn render_report() -> String {
    let s = event_log::stats();
    let mut out = String::new();
    out.push_str(&format!(
        "hunter security subsystem v{}\n",
        env!("CARGO_PKG_VERSION")
    ));
    out.push_str(&format!(
        "enforcement: syscall={} wx={} exec={} anomaly={}{}\n",
        policy::syscall_mode().as_str(),
        policy::wx_mode().as_str(),
        policy::exec_mode().as_str(),
        policy::anomaly_mode().as_str(),
        if policy::is_tighten_only() {
            " [tighten-only]"
        } else {
            ""
        },
    ));
    out.push_str(&format!(
        "events: total={} blocked={} warnings={} reported={} critical={} dropped={} critical_dropped={}\n",
        s.total, s.blocked, s.warnings, s.warnings_allowed, s.criticals, s.dropped, s.critical_dropped
    ));
    out.push_str(&format!(
        "active syscall policies: {}\n",
        policy::active_policy_count()
    ));
    out.push_str(&format!(
        "exec allowlist: {} (configured={}, learned={}); blacklist={}; learning={}\n",
        if policy::exec_allowlist_active() {
            "active"
        } else {
            "inactive"
        },
        policy::trusted_exec_count(),
        policy::learned_exec_count(),
        policy::blacklisted_exec_count(),
        if policy::exec_learning_enabled() {
            "on"
        } else {
            "off"
        },
    ));
    // Machine-readable dump of learned programs so a userspace helper can
    // persist them to /etc/hunter/whitelist (kernel never writes the FS).
    let learned = policy::learned_exec_paths();
    if !learned.is_empty() {
        out.push_str(&format!("learned-programs ({}):\n", learned.len()));
        for p in &learned {
            out.push_str("  ");
            out.push_str(p);
            out.push('\n');
        }
    }
    out.push_str("\nrecent events (oldest first):\n");
    out.push_str(&event_log::render());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    // hunter's state is global (atomics + lazy_static maps) and the
    // tighten-only latch is one-way, so the test sections must run in a fixed
    // order in a single thread. They are therefore driven sequentially from one
    // `#[test]`, with the latch exercised last.
    #[test]
    fn hunter_behaviour() {
        seccomp_section();
        elf_validation_section();
        elf_integrity_section();
        wx_section();
        allowlist_section();
        tighten_only_section();
    }

    fn seccomp_section() {
        policy::set_syscall_mode(Mode::Enforce);
        assert_eq!(check_syscall(42, 1, &[0; 6]), Ok(()));

        policy::register_policy(42, vec![1, 2]);
        assert_eq!(check_syscall(42, 1, &[0; 6]), Ok(()));
        assert_eq!(check_syscall(42, 2, &[0; 6]), Ok(()));
        assert_eq!(
            check_syscall(42, 3, &[0; 6]),
            Err(SecurityViolation::SyscallBlocked)
        );

        policy::set_syscall_mode(Mode::Report);
        assert_eq!(check_syscall(42, 3, &[0; 6]), Ok(()));

        policy::set_syscall_mode(Mode::Off);
        assert_eq!(check_syscall(42, 3, &[0; 6]), Ok(()));

        policy::set_syscall_mode(Mode::Enforce);
        task_exit(42);
        assert_eq!(check_syscall(42, 3, &[0; 6]), Ok(()));
    }

    fn elf_validation_section() {
        policy::set_exec_mode(Mode::Enforce);
        assert!(check_elf_binary("/bin/init", b"\x7fELFsomething"));
        // Shebang scripts are a valid executable format (path policy only).
        assert!(check_elf_binary("/usr/bin/run", b"#!/bin/sh\n"));
        assert!(!check_elf_binary("/tmp/run", b"#!/bin/sh\n")); // untrusted path
        assert!(!check_elf_binary("/tmp/malicious", b"\x7fELFsomething"));
        assert!(!check_elf_binary("/bin/../tmp/malicious", b"\x7fELF"));
        assert!(!check_elf_binary("/bin/init", b"MZsomething"));
        // /proc/self/fd magic-link is treated as untrusted.
        assert!(!check_elf_binary("/proc/self/fd/3", b"\x7fELF"));

        policy::set_exec_mode(Mode::Report);
        assert!(check_elf_binary("/tmp/tool", b"\x7fELF...."));
        assert!(!check_elf_binary("/tmp/tool", b"nope"));
    }

    fn elf_integrity_section() {
        policy::set_exec_mode(Mode::Off);
        // Minimal valid 64-bit LE ELF header for the host (x86_64=62).
        let mut h = [0u8; 20];
        h[0..4].copy_from_slice(b"\x7fELF");
        h[4] = 2; // 64-bit
        h[5] = 1; // little-endian
        h[6] = 1; // version
        h[16] = 2; // e_type ET_EXEC
        h[18] = 62; // e_machine x86_64
        assert!(check_elf_binary("/bin/ok", &h));
        // Foreign machine (aarch64=183) rejected on an x86_64 build.
        let mut bad = h;
        bad[18] = 183;
        assert!(!check_elf_binary("/bin/foreign", &bad));
        // Bogus e_type rejected.
        let mut badt = h;
        badt[16] = 9;
        assert!(!check_elf_binary("/bin/badtype", &badt));
    }

    fn wx_section() {
        policy::set_wx_mode(Mode::Enforce);
        // W only and X only are fine in isolation.
        assert!(check_mmap(7, true, false));
        assert!(check_mmap(7, false, true));
        // Immediate W+X blocked (pre-map).
        assert!(!check_mmap(7, true, true));

        // Two-step W-then-X: map writable (allowed + recorded), then mprotect
        // executable over the same range -> blocked.
        assert!(check_mmap(8, true, false));
        record_mapping(8, 0x20000, 0x1000, true);
        assert!(!check_mprotect(8, 0x20000, 0x1000, false, true));
        // After munmap the region is forgotten and X alone is fine again.
        check_munmap(8, 0x20000, 0x1000);
        assert!(check_mprotect(8, 0x20000, 0x1000, false, true));

        policy::set_wx_mode(Mode::Report);
        assert!(check_mmap(9, true, true)); // logged, allowed

        policy::set_wx_mode(Mode::Off);
        assert!(check_mmap(9, true, true));
        task_exit(7);
        task_exit(8);
        task_exit(9);
    }

    fn allowlist_section() {
        // --- deny-by-default allowlist (learning off) ---
        policy::set_exec_learning(false);
        policy::set_exec_mode(Mode::Enforce);
        assert!(!policy::exec_allowlist_active());
        assert!(check_elf_binary("/opt/app/foo", b"\x7fELF")); // inactive -> allowed

        policy::add_trusted_exec_path(String::from("/usr/bin/bash"));
        policy::add_trusted_exec_prefix(String::from("/bin/"));
        assert!(policy::exec_allowlist_active());
        assert!(check_elf_binary("/usr/bin/bash", b"\x7fELF")); // exact trusted
        assert!(check_elf_binary("/bin/ls", b"\x7fELF")); // under trusted dir
        assert!(!check_elf_binary("/opt/app/foo", b"\x7fELF")); // untrusted -> blocked
        assert!(!check_elf_binary("/bin/../opt/evil", b"\x7fELF")); // traversal

        policy::set_exec_mode(Mode::Report);
        assert!(check_elf_binary("/opt/app/foo", b"\x7fELF")); // report: allowed
        policy::clear_trusted_exec();
        assert!(!policy::exec_allowlist_active());

        // --- blacklist: hard deny even in Report (domain active) ---
        policy::add_blacklisted_exec_path(String::from("/usr/bin/evil"));
        policy::add_blacklisted_exec_prefix(String::from("/opt/danger/"));
        assert!(!check_elf_binary("/usr/bin/evil", b"\x7fELF"));
        assert!(!check_elf_binary("/opt/danger/x", b"\x7fELF"));
        assert!(check_elf_binary("/usr/bin/ok", b"\x7fELF")); // not blacklisted

        // --- learning: never denies, auto-adds safe programs ---
        policy::set_exec_learning(true);
        assert_eq!(policy::learned_exec_count(), 0);
        // A relative apk-style path is safe (not world-writable) -> learned.
        assert!(check_elf_binary(
            "lib/apk/exec/busybox-1.37.0-r",
            b"\x7fELF"
        ));
        assert!(policy::is_exec_listed("/lib/apk/exec/busybox-1.37.0-r"));
        assert_eq!(policy::learned_exec_count(), 1);
        // Re-exec does not double-learn.
        assert!(check_elf_binary(
            "/lib/apk/exec/busybox-1.37.0-r",
            b"\x7fELF"
        ));
        assert_eq!(policy::learned_exec_count(), 1);
        // World-writable programs are not auto-learned.
        assert!(!policy::is_world_writable_exec_path(
            "/lib/apk/exec/busybox-1.37.0-r"
        ));
        assert!(policy::is_world_writable_exec_path("/tmp/payload"));
        // Blacklist still wins under learning.
        assert!(!check_elf_binary("/usr/bin/evil", b"\x7fELF"));

        // reset
        policy::set_exec_learning(false);
        policy::clear_trusted_exec();
        policy::set_exec_mode(Mode::Off);
    }

    fn tighten_only_section() {
        policy::set_wx_mode(Mode::Report);
        policy::seal_tighten_only();
        // Relaxing is refused once sealed.
        policy::set_wx_mode(Mode::Off);
        assert_eq!(policy::wx_mode(), Mode::Report);
        // Tightening still works.
        policy::set_wx_mode(Mode::Enforce);
        assert_eq!(policy::wx_mode(), Mode::Enforce);
    }
}
