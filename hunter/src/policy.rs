//! Policy engine for the hunter security subsystem.
//!
//! Holds the independently-tunable enforcement domains plus the data that
//! drives them:
//!
//! * **syscall** — per-process syscall whitelists (a lightweight seccomp).
//! * **wx**      — write-xor-execute memory policy (mmap / mprotect).
//! * **exec**    — which filesystem paths a binary may be executed from.
//! * **anomaly** — whether detected floods / fork bombs are blocked or only logged.
//!
//! Each domain has its own [`Mode`] (`Off` / `Report` / `Enforce`) so the
//! subsystem can be rolled out audit-first and tightened per-domain.
//!
//! Hardening (P13): every mutator now records an audit event of the
//! `old -> new` transition, mode loads/stores use `SeqCst` (these gate
//! enforcement; a racy read must not silently drop a check), and the control
//! plane can be put into a one-way **tighten-only** mode so a single relaxing
//! store cannot quietly neutralise hunter after boot.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::{format, vec};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use lock::Mutex;

use crate::event_log::{record, Severity};

/// Mirror of `GLOBAL_POLICIES.len()`, maintained by every mutator while it
/// still holds the map lock. Lets the per-syscall check skip the globally
/// contended lock entirely in the default system state (no whitelists
/// registered), which is semantically identical to a map miss.
static POLICY_COUNT: AtomicUsize = AtomicUsize::new(0);

lazy_static::lazy_static! {
    /// Registry of process-specific syscall whitelists. Whitelists are stored
    /// sorted so the per-syscall check is a binary search. Mutators must
    /// refresh `POLICY_COUNT` before releasing the lock.
    pub static ref GLOBAL_POLICIES: Mutex<BTreeMap<u64, Vec<u32>>> = Mutex::new(BTreeMap::new());

    /// Path prefixes a binary must never be executed from (untrusted, writable
    /// world locations). Configurable at runtime.
    pub static ref UNTRUSTED_EXEC_PREFIXES: Mutex<Vec<String>> =
        Mutex::new(default_untrusted_prefixes());

    /// Allowlist of trusted programs: exact canonical executable paths that are
    /// explicitly permitted to run. Empty by default (allowlist inactive).
    pub static ref TRUSTED_EXEC_PATHS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// Allowlist of trusted directories: any executable whose canonical path
    /// starts with one of these prefixes is permitted. Empty by default.
    pub static ref TRUSTED_EXEC_PREFIXES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// Programs auto-learned at runtime (trust-on-first-use). Kept separate from
    /// the operator-configured allowlist so a userspace helper can read them
    /// from `/proc/hunter` and persist them to `/etc/hunter/whitelist`.
    pub static ref LEARNED_EXEC_PATHS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// Blacklist of denied programs: exact canonical paths that must never run.
    pub static ref BLACKLISTED_EXEC_PATHS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// Blacklist of denied directories: any executable under one of these
    /// prefixes is denied.
    pub static ref BLACKLISTED_EXEC_PREFIXES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// Optional default whitelist applied to new images at exec time. `None`
    /// keeps the seccomp domain opt-in (default-permissive), preserving boot.
    pub static ref DEFAULT_WHITELIST: Mutex<Option<Vec<u32>>> = Mutex::new(None);
}

/// What hunter does when a policy in a given domain is violated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Ignore the domain entirely (no checks, no logging).
    Off,
    /// Log the violation but allow the action (audit / IDS mode).
    Report,
    /// Log the violation and block the action (active enforcement).
    Enforce,
}

impl Mode {
    fn to_u8(self) -> u8 {
        match self {
            Mode::Off => 0,
            Mode::Report => 1,
            Mode::Enforce => 2,
        }
    }
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Mode::Off,
            1 => Mode::Report,
            _ => Mode::Enforce,
        }
    }
    /// Human-readable tag for the `/proc/hunter` header.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Report => "report",
            Mode::Enforce => "enforce",
        }
    }
}

// Default stance: enforce explicit syscall whitelists (opt-in per process, so
// safe), but only *report* W^X / exec-path / anomaly violations so real
// dynamic linkers, JITs and the base system are never broken by default.
static SYSCALL_MODE: AtomicU8 = AtomicU8::new(2); // Enforce
static WX_MODE: AtomicU8 = AtomicU8::new(1); // Report
static EXEC_MODE: AtomicU8 = AtomicU8::new(1); // Report
static ANOMALY_MODE: AtomicU8 = AtomicU8::new(1); // Report

/// One-way latch: once set, modes may only move towards stricter enforcement.
static TIGHTEN_ONLY: AtomicBool = AtomicBool::new(false);

/// Whether exec learning (trust-on-first-use) is enabled: safe programs are
/// auto-added to the allowlist and never denied. Off by default (the crate
/// changes nothing until the kernel opts in at boot).
static EXEC_LEARN: AtomicBool = AtomicBool::new(false);

/// Cap on auto-learned entries, bounding kernel memory if exec churns through
/// many distinct binaries.
const MAX_LEARNED_EXEC: usize = 8192;

fn default_untrusted_prefixes() -> Vec<String> {
    vec![
        String::from("/tmp/"),
        String::from("/var/tmp/"),
        String::from("/dev/shm/"),
    ]
}

/// Applies a mode transition for one domain, honouring the tighten-only latch
/// and recording an audit event. Returns the mode actually in effect after.
fn apply_mode(slot: &AtomicU8, domain: &'static str, requested: Mode) -> Mode {
    let current = Mode::from_u8(slot.load(Ordering::SeqCst));
    if current == requested {
        return current;
    }
    // Under the tighten-only latch, refuse any relaxation.
    if TIGHTEN_ONLY.load(Ordering::SeqCst) && requested.to_u8() < current.to_u8() {
        record(
            0,
            Severity::Warning,
            "CONTROL",
            "WARNING",
            format!(
                "refused relaxing {} mode {} -> {} (tighten-only latch)",
                domain,
                current.as_str(),
                requested.as_str()
            ),
        );
        return current;
    }
    slot.store(requested.to_u8(), Ordering::SeqCst);
    record(
        0,
        Severity::Notice,
        "CONTROL",
        "CONFIG",
        format!(
            "{} mode {} -> {}",
            domain,
            current.as_str(),
            requested.as_str()
        ),
    );
    requested
}

/// Sets the enforcement mode for the syscall-filtering domain.
pub fn set_syscall_mode(mode: Mode) {
    apply_mode(&SYSCALL_MODE, "syscall", mode);
}
/// Returns the current syscall-filtering mode.
pub fn syscall_mode() -> Mode {
    Mode::from_u8(SYSCALL_MODE.load(Ordering::SeqCst))
}

/// Sets the enforcement mode for the W^X memory domain.
pub fn set_wx_mode(mode: Mode) {
    apply_mode(&WX_MODE, "wx", mode);
}
/// Returns the current W^X mode.
pub fn wx_mode() -> Mode {
    Mode::from_u8(WX_MODE.load(Ordering::SeqCst))
}

/// Sets the enforcement mode for the executable-path domain.
pub fn set_exec_mode(mode: Mode) {
    apply_mode(&EXEC_MODE, "exec", mode);
}
/// Returns the current executable-path mode.
pub fn exec_mode() -> Mode {
    Mode::from_u8(EXEC_MODE.load(Ordering::SeqCst))
}

/// Sets the enforcement mode for the anomaly (flood / fork-bomb) domain.
pub fn set_anomaly_mode(mode: Mode) {
    apply_mode(&ANOMALY_MODE, "anomaly", mode);
}
/// Returns the current anomaly mode.
pub fn anomaly_mode() -> Mode {
    Mode::from_u8(ANOMALY_MODE.load(Ordering::SeqCst))
}

/// Engages the one-way tighten-only latch: after this, no domain can be
/// relaxed (only moved towards `Enforce`). Typically called once boot is done.
pub fn seal_tighten_only() {
    if !TIGHTEN_ONLY.swap(true, Ordering::SeqCst) {
        record(
            0,
            Severity::Notice,
            "CONTROL",
            "CONFIG",
            String::from("tighten-only latch engaged"),
        );
    }
}
/// Whether the tighten-only latch is engaged.
pub fn is_tighten_only() -> bool {
    TIGHTEN_ONLY.load(Ordering::SeqCst)
}

// ---- Back-compat shims for the original boolean enforcement switch --------

/// Sets whether syscall violations block (`true`) or warn (`false`).
pub fn set_enforcement_mode(enabled: bool) {
    set_syscall_mode(if enabled { Mode::Enforce } else { Mode::Report });
}
/// Returns `true` when syscall violations are blocked.
pub fn get_enforcement_mode() -> bool {
    syscall_mode() == Mode::Enforce
}

// ---- Syscall whitelists ---------------------------------------------------

/// Registers a whitelist of allowed syscall numbers for a process.
pub fn register_policy(pid: u64, mut allowed_syscalls: Vec<u32>) {
    allowed_syscalls.sort_unstable();
    let mut map = GLOBAL_POLICIES.lock();
    map.insert(pid, allowed_syscalls);
    POLICY_COUNT.store(map.len(), Ordering::Release);
}

/// Removes the security policy for a process (e.g. when it exits).
pub fn remove_policy(pid: u64) {
    let mut map = GLOBAL_POLICIES.lock();
    map.remove(&pid);
    POLICY_COUNT.store(map.len(), Ordering::Release);
}

/// Number of processes that currently have a syscall whitelist registered.
pub fn active_policy_count() -> usize {
    GLOBAL_POLICIES.lock().len()
}

/// Sets (or clears) the default whitelist applied to freshly-exec'd images.
/// `None` keeps the syscall domain opt-in / default-permissive.
pub fn set_default_whitelist(list: Option<Vec<u32>>) {
    *DEFAULT_WHITELIST.lock() = list;
}

/// Applies the default whitelist to `pid` if one is configured (P4): makes the
/// seccomp domain reachable without changing behaviour when unset.
pub fn apply_default_policy(pid: u64) {
    if let Some(list) = DEFAULT_WHITELIST.lock().as_ref() {
        let mut sorted = list.clone();
        sorted.sort_unstable();
        let mut map = GLOBAL_POLICIES.lock();
        map.insert(pid, sorted);
        POLICY_COUNT.store(map.len(), Ordering::Release);
    }
}

/// Inherits the parent's whitelist into a forked child so a process cannot
/// shed its policy merely by forking (P4).
pub fn inherit_policy(parent_pid: u64, child_pid: u64) {
    let mut map = GLOBAL_POLICIES.lock();
    if let Some(list) = map.get(&parent_pid).cloned() {
        map.insert(child_pid, list);
        POLICY_COUNT.store(map.len(), Ordering::Release);
    }
}

/// Checks whether a syscall is allowed for a given process.
///
/// Returns `Ok(())` when allowed, or `Err(enforce)` on a violation where
/// `enforce` is `true` if the action should be blocked.
pub fn is_syscall_allowed(pid: u64, syscall_num: u32) -> Result<(), bool> {
    let mode = syscall_mode();
    if mode == Mode::Off {
        return Ok(());
    }
    // Lock-free fast path for the default system state: no whitelist is
    // registered for ANY process, so every pid resolves to the permissive
    // map-miss arm below. Skips a globally-contended, IRQ-off lock that would
    // otherwise serialize every CPU on every syscall. A racing first
    // registration is indistinguishable from this call having run before it.
    if POLICY_COUNT.load(Ordering::Acquire) == 0 {
        return Ok(());
    }
    let policies = GLOBAL_POLICIES.lock();
    match policies.get(&pid) {
        // Whitelists are kept sorted by the registration paths.
        Some(allowed) if allowed.binary_search(&syscall_num).is_ok() => Ok(()),
        Some(_) => Err(mode == Mode::Enforce),
        // No policy registered for this pid: default permissive.
        None => Ok(()),
    }
}

// ---- Executable-path policy ----------------------------------------------

/// Adds a path prefix to the untrusted-execution list (idempotent).
pub fn add_untrusted_exec_prefix(prefix: String) {
    let mut list = UNTRUSTED_EXEC_PREFIXES.lock();
    if !list.iter().any(|p| *p == prefix) {
        list.push(prefix);
    }
}

/// Lexically canonicalizes an absolute path: collapses `//`, drops `.`, and
/// resolves `..` against earlier components (without touching the filesystem).
/// `/bin/../tmp/x` becomes `/tmp/x`, so a traversal cannot smuggle an execution
/// past the untrusted-prefix check (P6, finding ELF-3).
pub fn canonicalize(path: &str) -> String {
    let mut stack: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            c => stack.push(c),
        }
    }
    let mut out = String::from("/");
    out.push_str(&stack.join("/"));
    out
}

/// Returns `true` when executing from `path` should be treated as untrusted:
/// a world-writable location (after canonicalization) or a `/proc/*/fd/*`
/// magic-link that smuggles past prefix matching (P6, finding ELF-2/ELF-3).
pub fn is_untrusted_exec_path(path: &str) -> bool {
    // /proc/self/fd/N and /proc/<pid>/fd/N resolve to an arbitrary opened
    // inode, defeating textual prefix checks — always treat as untrusted.
    if path.starts_with("/proc/") && path.contains("/fd/") {
        return true;
    }
    // A relative exec path is resolved against cwd at the FS layer; without that
    // context we cannot prove it lands somewhere trusted, so flag it.
    if !path.starts_with('/') {
        return true;
    }
    let canon = canonicalize(path);
    let list = UNTRUSTED_EXEC_PREFIXES.lock();
    list.iter().any(|p| canon.starts_with(p.as_str()))
}

// ---- Trusted-program allowlist (application allow-listing) ----------------
//
// An *allowlist* inverts the deny-by-location model into deny-by-default: when
// active, only programs whose canonical path is explicitly trusted (an exact
// match, or under a trusted directory) may execute; everything else is a
// violation handled by `exec_mode` (logged in Report, blocked in Enforce).
//
// The allowlist is **inactive** while both trusted sets are empty, so by
// default nothing changes — an operator opts in by registering trusted
// programs (and typically raising `exec_mode` to `Enforce`).

/// Adds an exact canonical executable path to the trusted-program allowlist.
pub fn add_trusted_exec_path(path: String) {
    let canon = canonicalize(&path);
    let mut list = TRUSTED_EXEC_PATHS.lock();
    if !list.iter().any(|p| *p == canon) {
        record(
            0,
            Severity::Notice,
            "CONTROL",
            "CONFIG",
            format!("trusted program added: {}", canon),
        );
        list.push(canon);
    }
}

/// Adds a trusted directory prefix: any executable under it is allowed.
pub fn add_trusted_exec_prefix(prefix: String) {
    let mut list = TRUSTED_EXEC_PREFIXES.lock();
    if !list.iter().any(|p| *p == prefix) {
        record(
            0,
            Severity::Notice,
            "CONTROL",
            "CONFIG",
            format!("trusted exec directory added: {}", prefix),
        );
        list.push(prefix);
    }
}

/// Seeds the allowlist with the standard read-only system program directories,
/// so the base system keeps working when an operator flips `exec_mode` to
/// `Enforce`. Not called by default — opt-in.
pub fn install_default_trusted_exec() {
    for d in [
        "/bin/",
        "/sbin/",
        "/usr/bin/",
        "/usr/sbin/",
        "/usr/local/bin/",
        "/usr/local/sbin/",
        "/lib/",
        "/lib64/",
        "/usr/lib/",
        "/usr/lib64/",
    ] {
        add_trusted_exec_prefix(String::from(d));
    }
}

/// Clears the trusted-program allowlist, deactivating it.
pub fn clear_trusted_exec() {
    TRUSTED_EXEC_PATHS.lock().clear();
    TRUSTED_EXEC_PREFIXES.lock().clear();
}

/// Number of trusted entries (exact paths + directory prefixes).
pub fn trusted_exec_count() -> usize {
    TRUSTED_EXEC_PATHS.lock().len() + TRUSTED_EXEC_PREFIXES.lock().len()
}

/// Whether the allowlist is active (any configured or learned entry). While
/// inactive, [`is_exec_allowed`] permits everything, preserving default boot.
pub fn exec_allowlist_active() -> bool {
    !TRUSTED_EXEC_PATHS.lock().is_empty()
        || !TRUSTED_EXEC_PREFIXES.lock().is_empty()
        || !LEARNED_EXEC_PATHS.lock().is_empty()
}

/// Explicit membership test (no "inactive ⇒ allow" shortcut): `true` only if
/// `path` actually matches a configured or learned trusted entry. The path is
/// canonicalized first so a traversal cannot masquerade as a trusted program.
pub fn is_exec_listed(path: &str) -> bool {
    let canon = canonicalize(path);
    TRUSTED_EXEC_PATHS.lock().iter().any(|p| *p == canon)
        || TRUSTED_EXEC_PREFIXES
            .lock()
            .iter()
            .any(|p| canon.starts_with(p.as_str()))
        || LEARNED_EXEC_PATHS.lock().iter().any(|p| *p == canon)
}

/// Returns `true` if `path` is trusted, or the allowlist is inactive.
pub fn is_exec_allowed(path: &str) -> bool {
    if !exec_allowlist_active() {
        return true;
    }
    is_exec_listed(path)
}

// ---- Exec learning (trust-on-first-use) -----------------------------------

/// Enables or disables exec learning. When enabled, a *safe* program (valid
/// format, not blacklisted, not from a world-writable location) seen at exec is
/// auto-added to the allowlist and never denied — a learning allowlist that
/// builds itself without breaking anything.
pub fn set_exec_learning(enabled: bool) {
    if EXEC_LEARN.swap(enabled, Ordering::SeqCst) != enabled {
        record(
            0,
            Severity::Notice,
            "CONTROL",
            "CONFIG",
            format!(
                "exec learning {}",
                if enabled { "enabled" } else { "disabled" }
            ),
        );
    }
}
/// Whether exec learning is enabled.
pub fn exec_learning_enabled() -> bool {
    EXEC_LEARN.load(Ordering::SeqCst)
}

/// Number of auto-learned programs.
pub fn learned_exec_count() -> usize {
    LEARNED_EXEC_PATHS.lock().len()
}

/// Snapshot of the learned programs, for `/proc/hunter` so a userspace helper
/// can persist them to `/etc/hunter/whitelist`.
pub fn learned_exec_paths() -> Vec<String> {
    LEARNED_EXEC_PATHS.lock().clone()
}

/// Adds `path` to the learned set if not already trusted and the cap allows.
/// Returns `true` if a new entry was learned (so the caller can log it once).
pub fn learn_exec_path(path: &str) -> bool {
    if is_exec_listed(path) {
        return false;
    }
    let canon = canonicalize(path);
    let mut learned = LEARNED_EXEC_PATHS.lock();
    if learned.len() >= MAX_LEARNED_EXEC || learned.iter().any(|p| *p == canon) {
        return false;
    }
    learned.push(canon);
    true
}

// ---- Exec blacklist (operator-curated hard deny) --------------------------

/// Adds an exact canonical program path to the exec blacklist.
pub fn add_blacklisted_exec_path(path: String) {
    let canon = canonicalize(&path);
    let mut list = BLACKLISTED_EXEC_PATHS.lock();
    if !list.iter().any(|p| *p == canon) {
        record(
            0,
            Severity::Notice,
            "CONTROL",
            "CONFIG",
            format!("blacklisted program: {}", canon),
        );
        list.push(canon);
    }
}

/// Adds a denied directory prefix to the exec blacklist.
pub fn add_blacklisted_exec_prefix(prefix: String) {
    let mut list = BLACKLISTED_EXEC_PREFIXES.lock();
    if !list.iter().any(|p| *p == prefix) {
        record(
            0,
            Severity::Notice,
            "CONTROL",
            "CONFIG",
            format!("blacklisted directory: {}", prefix),
        );
        list.push(prefix);
    }
}

/// Number of blacklist entries (exact paths + directory prefixes).
pub fn blacklisted_exec_count() -> usize {
    BLACKLISTED_EXEC_PATHS.lock().len() + BLACKLISTED_EXEC_PREFIXES.lock().len()
}

/// Returns `true` if `path` is on the exec blacklist (canonicalized first).
pub fn is_exec_blacklisted(path: &str) -> bool {
    let canon = canonicalize(path);
    BLACKLISTED_EXEC_PATHS.lock().iter().any(|p| *p == canon)
        || BLACKLISTED_EXEC_PREFIXES
            .lock()
            .iter()
            .any(|p| canon.starts_with(p.as_str()))
}

/// Returns `true` when `path` is in a world-writable location (`/tmp`,
/// `/var/tmp`, `/dev/shm`) or a `/proc/*/fd/*` magic-link — i.e. unsafe to
/// auto-trust. Unlike [`is_untrusted_exec_path`] this does *not* flag merely
/// relative paths, so a package manager exec'ing `lib/apk/.../busybox` with a
/// relative path is still learnable.
pub fn is_world_writable_exec_path(path: &str) -> bool {
    if path.starts_with("/proc/") && path.contains("/fd/") {
        return true;
    }
    let canon = canonicalize(path);
    UNTRUSTED_EXEC_PREFIXES
        .lock()
        .iter()
        .any(|p| canon.starts_with(p.as_str()))
}
