//! Boot-time loader for hunter's `/etc/hunter/` policy files.
//!
//! The kernel reads (never writes) two optional newline-delimited lists from
//! the root filesystem at boot:
//!
//! * `/etc/hunter/whitelist` — trusted programs that may always run.
//! * `/etc/hunter/blacklist` — denied programs that must never run.
//!
//! Each non-empty, non-`#` line is one entry. A trailing `/` makes it a
//! directory prefix (everything beneath it); otherwise it is an exact program
//! path. Missing files are fine — the lists simply stay empty.
//!
//! Learned programs (trust-on-first-use) live in kernel memory and are surfaced
//! at `/proc/hunter`; a userspace helper is expected to append them back to
//! `/etc/hunter/whitelist`. The kernel deliberately does not write the FS.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use rcore_fs::vfs::INode;

/// Largest config file we will read (1 MiB), bounding boot-time memory.
const MAX_CONFIG_BYTES: usize = 1 << 20;

/// Loads `/etc/hunter/{whitelist,blacklist}` from `root` into hunter's policy
/// and enables exec learning so safe programs are auto-trusted without denial.
pub fn load(root: &Arc<dyn INode>) {
    // Learning on by default: a "whitelist that never denies" which builds
    // itself from observed-safe execs. The blacklist remains the only deny.
    hunter::policy::set_exec_learning(true);
    let n_white = load_list(root, "/etc/hunter/whitelist", false);
    let n_black = load_list(root, "/etc/hunter/blacklist", true);
    kernel_hal::klog_info!(
        "hunter: loaded /etc/hunter (whitelist={}, blacklist={}, learning=on)",
        n_white,
        n_black
    );
}

/// Reads one list file and registers its entries. Returns how many were added.
/// A missing or unreadable file yields `0` without error.
fn load_list(root: &Arc<dyn INode>, path: &str, blacklist: bool) -> usize {
    let inode = match root.lookup(path) {
        Ok(i) => i,
        Err(_) => return 0,
    };
    let size = match inode.metadata() {
        Ok(m) => m.size.min(MAX_CONFIG_BYTES),
        Err(_) => return 0,
    };
    if size == 0 {
        return 0;
    }
    let mut buf = vec![0u8; size];
    let n = inode.read_at(0, &mut buf).unwrap_or(0);
    let text = match core::str::from_utf8(&buf[..n]) {
        Ok(t) => t,
        Err(_) => return 0,
    };
    let mut count = 0;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let is_prefix = line.ends_with('/');
        let entry = String::from(line);
        match (blacklist, is_prefix) {
            (false, false) => hunter::policy::add_trusted_exec_path(entry),
            (false, true) => hunter::policy::add_trusted_exec_prefix(entry),
            (true, false) => hunter::policy::add_blacklisted_exec_path(entry),
            (true, true) => hunter::policy::add_blacklisted_exec_prefix(entry),
        }
        count += 1;
    }
    count
}
