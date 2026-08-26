//! System identification state shared by `uname(2)`, `sethostname(2)`,
//! `setdomainname(2)`, `/proc/version` and `/proc/sys/kernel/*`.
//!
//! Linux exposes the same six `utsname` strings through several interfaces and
//! real userspace mixes them freely (musl's `gethostname` reads `uname`,
//! `ps`/`procps` read `/proc/sys/kernel/osrelease`, glibc parses the release
//! at startup). Keeping the values here, in one place, is what makes those
//! views agree.

use alloc::string::{String, ToString};
use lock::Mutex;

/// `utsname.sysname` / `/proc/sys/kernel/ostype`. We are a Linux-compatible
/// kernel, and programs switch on this exact string.
pub const OS_TYPE: &str = "Eclipse";

/// `utsname.release` / `/proc/sys/kernel/osrelease` / `uname -r`.
///
/// The format must be Linux's `VERSION.PATCHLEVEL.SUBLEVEL-EXTRAVERSION`:
/// glibc aborts at startup ("FATAL: kernel too old") when the leading triple
/// parses below its build-time minimum, Go's runtime refuses pre-2.6.23
/// kernels, and countless scripts do `uname -r | cut -d. -f1`. The previous
/// value here was the crate version ("0.1.0-zcore"), which fails all of those
/// checks; 5.15 matches the LTS kernel whose ABI subset this tree implements.
#[cfg(target_os = "none")]
pub const OS_RELEASE: &str = "0.4.4";
/// LibOS builds keep their distinguishing suffix, still in parseable form.
#[cfg(not(target_os = "none"))]
pub const OS_RELEASE: &str = "0.4.4-libos";

/// Maximum length of a host or domain name, per POSIX HOST_NAME_MAX on Linux
/// (`sethostname(2)` answers EINVAL above this).
pub const HOST_NAME_MAX: usize = 64;

static HOSTNAME: Mutex<Option<String>> = Mutex::new(None);
static DOMAINNAME: Mutex<Option<String>> = Mutex::new(None);

/// Default `utsname.nodename` until `sethostname` is called (Alpine's boot
/// scripts then install /etc/hostname over it).
const DEFAULT_HOSTNAME: &str = "Eclipse";
/// Default NIS `utsname.domainname`; Linux uses "(none)" when unset.
const DEFAULT_DOMAINNAME: &str = "(none)";

/// `utsname.version` / `/proc/sys/kernel/version`: free-form build banner.
pub fn os_version() -> String {
    kernel_hal::vdso::vdso_constants()
        .version_string
        .as_str()
        .to_string()
}

/// Current hostname (`utsname.nodename`, `/proc/sys/kernel/hostname`).
pub fn hostname() -> String {
    HOSTNAME
        .lock()
        .clone()
        .unwrap_or_else(|| DEFAULT_HOSTNAME.to_string())
}

/// Set the hostname from `sethostname(2)`. The caller has validated length.
pub fn set_hostname(name: &str) {
    *HOSTNAME.lock() = Some(name.to_string());
}

/// Current NIS domain name (`utsname.domainname`,
/// `/proc/sys/kernel/domainname`).
pub fn domainname() -> String {
    DOMAINNAME
        .lock()
        .clone()
        .unwrap_or_else(|| DEFAULT_DOMAINNAME.to_string())
}

/// Set the NIS domain name from `setdomainname(2)`.
pub fn set_domainname(name: &str) {
    *DOMAINNAME.lock() = Some(name.to_string());
}

/// `/proc/version`, composed from the same values `uname(2)` reports so the
/// two never disagree (procps, neofetch and friends read this file).
pub fn proc_version() -> String {
    alloc::format!(
        "Eclipse version {} (eclipse@eclipse) {}\n",
        OS_RELEASE,
        os_version()
    )
}
