//! Per-mount flags shared between the mount table and mounted filesystems.

use lock::Mutex;

/// Linux mount(2) flag bits used by Eclipse.
pub const MS_RDONLY: usize = 1;
pub const MS_NOSUID: usize = 2;
pub const MS_NODEV: usize = 4;
pub const MS_NOEXEC: usize = 8;
pub const MS_REMOUNT: usize = 32;
pub const MS_BIND: usize = 4096;
pub const MS_MOVE: usize = 8192;
#[allow(dead_code)]
pub const MS_REC: usize = 16384;

/// umount2(2) flags.
pub const MNT_FORCE: usize = 1;
pub const MNT_DETACH: usize = 2;

/// Mutable mount options checked on write paths.
#[derive(Debug)]
pub struct MountState {
    pub read_only: Mutex<bool>,
}

impl MountState {
    pub fn new(read_only: bool) -> Self {
        Self {
            read_only: Mutex::new(read_only),
        }
    }

    pub fn is_read_only(&self) -> bool {
        *self.read_only.lock()
    }

    pub fn set_read_only(&self, read_only: bool) {
        *self.read_only.lock() = read_only;
    }
}

pub fn flags_read_only(flags: usize, data: &str) -> bool {
    if flags & MS_RDONLY != 0 {
        return true;
    }
    parse_option_flag(data, "ro")
}

pub fn parse_option_flag(data: &str, key: &str) -> bool {
    for part in data.split(',') {
        let part = part.trim();
        if part == key {
            return true;
        }
        if let Some((k, v)) = part.split_once('=') {
            if k.trim() == key && (v.trim() == "1" || v.trim().eq_ignore_ascii_case("true")) {
                return true;
            }
        }
    }
    false
}

pub fn build_options_string(flags: usize, data: &str) -> alloc::string::String {
    use alloc::string::String;
    let mut opts = if flags_read_only(flags, data) {
        String::from("ro")
    } else {
        String::from("rw")
    };
    if flags & MS_NOSUID != 0 {
        opts.push_str(",nosuid");
    }
    if flags & MS_NODEV != 0 {
        opts.push_str(",nodev");
    }
    if flags & MS_NOEXEC != 0 {
        opts.push_str(",noexec");
    }
    if flags & MS_BIND != 0 {
        opts.push_str(",bind");
    }
    if !data.is_empty() {
        opts.push(',');
        opts.push_str(data);
    }
    opts
}
