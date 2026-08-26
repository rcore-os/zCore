//! Tamper-evident forensic event log for the hunter security subsystem.
//!
//! Every security-relevant decision (a blocked syscall, a suspicious exec, a
//! W^X violation, a detected anomaly) is recorded as a structured [`LogEntry`]
//! and accounted in cheap lock-free [`Stats`] counters.
//!
//! Hardening (P10/P11): a single evictable ring let an attacker flood benign
//! events to silently evict attack evidence (finding IDS-1/EVADE-1). The log is
//! now split into **two severity-segregated rings** — a large evictable ring
//! for `Info`/`Notice` and a separate reserve for `Warning`/`Critical` — so a
//! flood of low-severity noise can never evict high-severity evidence. Eviction
//! is surfaced per-severity (`critical_dropped`) so an operator can never miss
//! that high-severity events were lost. An optional [`set_sink`] callback lets
//! the kernel stream `Warning`+ events to a durable off-ring sink (serial /
//! console) before any eviction can erase them.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use lock::Mutex;

use crate::clock;

lazy_static::lazy_static! {
    /// Evictable ring for routine `Info`/`Notice` events.
    static ref GENERAL_LOG: Mutex<IntrusionLog> = Mutex::new(IntrusionLog::new(512));
    /// Reserved ring for `Warning`/`Critical` evidence; far slower to evict
    /// because low-severity noise cannot land here.
    static ref PRIORITY_LOG: Mutex<IntrusionLog> = Mutex::new(IntrusionLog::new(256));
}

/// Monotonic sequence number handed to every recorded event.
static SEQ: AtomicU64 = AtomicU64::new(0);

// Lock-free running totals, so a `/proc/hunter` read or a health check never
// needs to walk (or lock) either ring.
static TOTAL: AtomicU64 = AtomicU64::new(0);
static BLOCKED: AtomicU64 = AtomicU64::new(0);
static WARNINGS: AtomicU64 = AtomicU64::new(0);
/// Report-mode violations that were logged but allowed to proceed.
static WARNINGS_ALLOWED: AtomicU64 = AtomicU64::new(0);
static CRITICALS: AtomicU64 = AtomicU64::new(0);
/// Low-severity events evicted from the general ring.
static DROPPED: AtomicU64 = AtomicU64::new(0);
/// High-severity events evicted from the priority ring (should stay ~0).
static CRITICAL_DROPPED: AtomicU64 = AtomicU64::new(0);

/// Optional durable sink for `Warning`+ events, stored as an erased pointer.
static SINK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Severity of a security event, ordered from least to most urgent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Informational; routine bookkeeping (subsystem init, policy changes).
    Info,
    /// Noteworthy but expected (a watched-but-allowed sensitive syscall).
    Notice,
    /// A policy violation or suspicious behaviour.
    Warning,
    /// A high-confidence attack indicator (active enforcement kicked in).
    Critical,
}

impl Severity {
    /// Short, fixed-width tag used in the rendered report.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "INFO",
            Severity::Notice => "NOTICE",
            Severity::Warning => "WARN",
            Severity::Critical => "CRIT",
        }
    }
    /// High-severity events go to the reserved ring and the durable sink.
    fn is_priority(self) -> bool {
        matches!(self, Severity::Warning | Severity::Critical)
    }
}

/// A single recorded security event.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Monotonic sequence number (gaps reveal evicted events).
    pub seq: u64,
    /// Timestamp in nanoseconds since boot (0 before the clock is wired).
    pub ts_ns: u64,
    /// Offending / acting process id (0 = kernel / subsystem itself).
    pub pid: u64,
    /// Severity of the event.
    pub severity: Severity,
    /// Coarse domain, e.g. `"SYSCALL"`, `"EXEC"`, `"WX"`, `"ANOMALY"`.
    pub category: &'static str,
    /// What hunter did, e.g. `"BLOCKED"`, `"WARNING"`, `"ALLOWED"`.
    pub action: &'static str,
    /// Free-form, human-readable detail.
    pub description: String,
}

/// Bounded ring buffer of [`LogEntry`].
pub struct IntrusionLog {
    entries: Vec<LogEntry>,
    head: usize,
    max_size: usize,
}

impl IntrusionLog {
    /// Creates an empty log holding at most `max_size` entries.
    pub const fn new(max_size: usize) -> Self {
        Self {
            entries: Vec::new(),
            head: 0,
            max_size,
        }
    }

    /// Appends an entry. Returns `true` if an older entry was evicted.
    pub fn push(&mut self, entry: LogEntry) -> bool {
        if self.entries.len() < self.max_size {
            self.entries.push(entry);
            false
        } else {
            // Overwrite in place so we never re-shift the whole Vec (O(1)).
            self.entries[self.head] = entry;
            self.head = (self.head + 1) % self.max_size;
            true
        }
    }

    /// Returns the entries in chronological (oldest-first) order.
    pub fn get_entries(&self) -> Vec<LogEntry> {
        if self.entries.len() < self.max_size {
            return self.entries.clone();
        }
        let mut out = Vec::with_capacity(self.entries.len());
        out.extend_from_slice(&self.entries[self.head..]);
        out.extend_from_slice(&self.entries[..self.head]);
        out
    }
}

/// Lock-free snapshot of the running counters.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub total: u64,
    pub blocked: u64,
    pub warnings: u64,
    pub warnings_allowed: u64,
    pub criticals: u64,
    pub dropped: u64,
    pub critical_dropped: u64,
}

/// Returns a snapshot of the global event counters.
pub fn stats() -> Stats {
    Stats {
        total: TOTAL.load(Ordering::Relaxed),
        blocked: BLOCKED.load(Ordering::Relaxed),
        warnings: WARNINGS.load(Ordering::Relaxed),
        warnings_allowed: WARNINGS_ALLOWED.load(Ordering::Relaxed),
        criticals: CRITICALS.load(Ordering::Relaxed),
        dropped: DROPPED.load(Ordering::Relaxed),
        critical_dropped: CRITICAL_DROPPED.load(Ordering::Relaxed),
    }
}

/// Registers a durable sink invoked (outside all ring locks) for every
/// `Warning`/`Critical` event, so evidence reaches serial/console before any
/// in-memory eviction can erase it.
pub fn set_sink(sink: fn(&LogEntry)) {
    SINK.store(sink as *mut (), Ordering::SeqCst);
}

/// Records a fully-specified security event.
pub fn record(
    pid: u64,
    severity: Severity,
    category: &'static str,
    action: &'static str,
    description: String,
) {
    TOTAL.fetch_add(1, Ordering::Relaxed);
    match severity {
        Severity::Warning => {
            WARNINGS.fetch_add(1, Ordering::Relaxed);
        }
        Severity::Critical => {
            CRITICALS.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }
    // An action hunter actively blocked vs. merely reported-and-allowed.
    if action == "BLOCKED" {
        BLOCKED.fetch_add(1, Ordering::Relaxed);
    } else if action == "WARNING" {
        WARNINGS_ALLOWED.fetch_add(1, Ordering::Relaxed);
    }

    let entry = LogEntry {
        seq: SEQ.fetch_add(1, Ordering::Relaxed),
        ts_ns: clock::now_ns(),
        pid,
        severity,
        category,
        action,
        description,
    };

    let evicted = if severity.is_priority() {
        PRIORITY_LOG.lock().push(entry.clone())
    } else {
        GENERAL_LOG.lock().push(entry.clone())
    };
    if evicted {
        if severity.is_priority() {
            CRITICAL_DROPPED.fetch_add(1, Ordering::Relaxed);
        } else {
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
    }

    // Stream high-severity evidence to the durable sink outside the lock.
    if severity.is_priority() {
        let p = SINK.load(Ordering::Acquire);
        if !p.is_null() {
            // SAFETY: `p` is a valid `fn(&LogEntry)` sealed by `set_sink`.
            let sink: fn(&LogEntry) = unsafe { core::mem::transmute(p) };
            sink(&entry);
        }
    }
}

/// Back-compat helper: appends an event without explicit severity/category.
pub fn log_event(pid: u64, action: &'static str, description: String) {
    let severity = match action {
        "BLOCKED" => Severity::Critical,
        "WARNING" | "ELF_BLOCKED" => Severity::Warning,
        _ => Severity::Info,
    };
    record(pid, severity, "SYSCALL", action, description);
}

/// Formats a nanosecond timestamp as `<secs>.<ms>` since boot.
fn fmt_ts(ts_ns: u64) -> String {
    let secs = ts_ns / 1_000_000_000;
    let millis = (ts_ns % 1_000_000_000) / 1_000_000;
    format!("{}.{:03}", secs, millis)
}

/// Renders both rings, merged into chronological order, for `/proc/hunter`.
pub fn render() -> String {
    let mut entries = GENERAL_LOG.lock().get_entries();
    entries.extend(PRIORITY_LOG.lock().get_entries());
    entries.sort_by_key(|e| e.seq);

    let mut out = String::new();
    if entries.is_empty() {
        out.push_str("(no security events recorded)\n");
        return out;
    }
    for e in &entries {
        out.push_str(&format!(
            "[{:>6}] +{:>10}s pid={:<5} {:<6} {:<9} {}: {}\n",
            e.seq,
            fmt_ts(e.ts_ns),
            e.pid,
            e.severity.as_str(),
            e.category,
            e.action,
            e.description,
        ));
    }
    out
}
