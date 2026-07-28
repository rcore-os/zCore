//! A small `sysctl(2)` implementation for the `kern.*` and `hw.*` leaves a
//! FreeBSD libc touches during process startup.
//!
//! FreeBSD libc resolves things like the page size, CPU count, OS release date
//! and the address of `ps_strings` through `sysctl`/`sysctlbyname` very early —
//! `__libc_start1` and the malloc initialiser both do — so returning sensible
//! answers here is what lets a FreeBSD program get as far as `main`. Leaves we
//! do not model return `ENOENT`, exactly as a FreeBSD kernel does for an
//! unknown MIB, which libc treats as "feature absent" and copes with.

use super::consts::{ctl, MACHINE, OSRELDATE, OSRELEASE, OSTYPE};
use alloc::string::String;
use alloc::vec::Vec;

/// Runtime values the static tables cannot know on their own (they depend on
/// the machine and the running process).
pub struct SysctlCtx {
    /// Number of online CPUs (`hw.ncpu`, `AT_NCPUS`).
    pub ncpus: u32,
    /// Software page size (`hw.pagesize`).
    pub page_size: u32,
    /// Top of the user stack (`kern.usrstack`).
    pub usrstack: u64,
    /// Address of the process's `ps_strings` (`kern.ps_strings`).
    pub ps_strings: u64,
    /// Physical memory in bytes (`hw.physmem`).
    pub physmem: u64,
    /// Current hostname (`kern.hostname`).
    pub hostname: String,
    /// 16 random bytes for `kern.arnd`.
    pub arnd: [u8; 16],
}

/// A resolved sysctl value, before it is copied out to the caller's buffer.
pub enum SysctlVal {
    /// A 32-bit integer (`CTLTYPE_INT`/`UINT`).
    Int(u32),
    /// A 64-bit integer (`CTLTYPE_LONG`/`ULONG`/`S64`/`U64`).
    Long(u64),
    /// A NUL-terminated string (`CTLTYPE_STRING`).
    Str(String),
    /// An opaque byte blob (`CTLTYPE_OPAQUE`).
    Bytes(Vec<u8>),
}

impl SysctlVal {
    /// Serialise to the exact bytes `sysctl` copies into the caller's `oldp`.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            SysctlVal::Int(v) => v.to_le_bytes().to_vec(),
            SysctlVal::Long(v) => v.to_le_bytes().to_vec(),
            // FreeBSD strings are returned with the trailing NUL included.
            SysctlVal::Str(s) => {
                let mut b = s.as_bytes().to_vec();
                b.push(0);
                b
            }
            SysctlVal::Bytes(b) => b.clone(),
        }
    }
}

/// Resolve an integer MIB (`__sysctl`) against the leaves we model.
pub fn query(mib: &[i32], ctx: &SysctlCtx) -> Option<SysctlVal> {
    match mib {
        [ctl::CTL_KERN, leaf] => query_kern(*leaf, ctx),
        [ctl::CTL_HW, leaf] => query_hw(*leaf, ctx),
        _ => None,
    }
}

fn query_kern(leaf: i32, ctx: &SysctlCtx) -> Option<SysctlVal> {
    Some(match leaf {
        ctl::KERN_OSTYPE => SysctlVal::Str(String::from(OSTYPE)),
        ctl::KERN_OSRELEASE => SysctlVal::Str(String::from(OSRELEASE)),
        ctl::KERN_OSREV => SysctlVal::Long(OSRELDATE as u64),
        ctl::KERN_VERSION => SysctlVal::Str(alloc::format!("FreeBSD {} Eclipse\n", OSRELEASE)),
        ctl::KERN_OSRELDATE => SysctlVal::Int(OSRELDATE),
        ctl::KERN_HOSTNAME => SysctlVal::Str(ctx.hostname.clone()),
        ctl::KERN_USRSTACK => SysctlVal::Long(ctx.usrstack),
        ctl::KERN_PS_STRINGS => SysctlVal::Long(ctx.ps_strings),
        ctl::KERN_MAXFILES => SysctlVal::Int(1024),
        ctl::KERN_ARGMAX => SysctlVal::Int(256 * 1024),
        ctl::KERN_IOV_MAX => SysctlVal::Int(1024),
        ctl::KERN_ARND => SysctlVal::Bytes(ctx.arnd.to_vec()),
        _ => return None,
    })
}

fn query_hw(leaf: i32, ctx: &SysctlCtx) -> Option<SysctlVal> {
    Some(match leaf {
        ctl::HW_MACHINE => SysctlVal::Str(String::from(MACHINE)),
        ctl::HW_MACHINE_ARCH => SysctlVal::Str(String::from(MACHINE)),
        ctl::HW_MODEL => SysctlVal::Str(String::from("Eclipse virtual CPU")),
        ctl::HW_NCPU => SysctlVal::Int(ctx.ncpus),
        ctl::HW_BYTEORDER => SysctlVal::Int(1234), // little-endian
        ctl::HW_PAGESIZE => SysctlVal::Int(ctx.page_size),
        ctl::HW_PHYSMEM => SysctlVal::Long(ctx.physmem),
        ctl::HW_USERMEM => SysctlVal::Long(ctx.physmem),
        _ => return None,
    })
}

/// Translate the handful of `sysctlbyname` names FreeBSD libc uses into the
/// integer MIB [`query`] understands. Anything not listed yields `None`, which
/// the caller reports as `ENOENT`.
pub fn name_to_mib(name: &str) -> Option<Vec<i32>> {
    let mib: &[i32] = match name {
        "kern.ostype" => &[ctl::CTL_KERN, ctl::KERN_OSTYPE],
        "kern.osrelease" => &[ctl::CTL_KERN, ctl::KERN_OSRELEASE],
        "kern.osreldate" => &[ctl::CTL_KERN, ctl::KERN_OSRELDATE],
        "kern.version" => &[ctl::CTL_KERN, ctl::KERN_VERSION],
        "kern.hostname" => &[ctl::CTL_KERN, ctl::KERN_HOSTNAME],
        "kern.usrstack" => &[ctl::CTL_KERN, ctl::KERN_USRSTACK],
        "kern.ps_strings" => &[ctl::CTL_KERN, ctl::KERN_PS_STRINGS],
        "kern.arandom" => &[ctl::CTL_KERN, ctl::KERN_ARND],
        // libc's sysconf(_SC_NPROCESSORS_*) and jemalloc both read these.
        "hw.ncpu" => &[ctl::CTL_HW, ctl::HW_NCPU],
        "hw.pagesize" => &[ctl::CTL_HW, ctl::HW_PAGESIZE],
        "hw.machine" => &[ctl::CTL_HW, ctl::HW_MACHINE],
        "hw.machine_arch" => &[ctl::CTL_HW, ctl::HW_MACHINE_ARCH],
        "hw.physmem" => &[ctl::CTL_HW, ctl::HW_PHYSMEM],
        "hw.usermem" => &[ctl::CTL_HW, ctl::HW_USERMEM],
        _ => return None,
    };
    Some(mib.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::convert::TryInto;

    fn ctx() -> SysctlCtx {
        SysctlCtx {
            ncpus: 4,
            page_size: 4096,
            usrstack: 0x7fff_ffff_0000,
            ps_strings: 0x7fff_fffe_0000,
            physmem: 512 * 1024 * 1024,
            hostname: String::from("eclipse"),
            arnd: [7; 16],
        }
    }

    #[test]
    fn ostype_is_freebsd_with_trailing_nul() {
        let v = query(&[ctl::CTL_KERN, ctl::KERN_OSTYPE], &ctx()).unwrap();
        assert_eq!(v.to_bytes(), b"FreeBSD\0");
    }

    #[test]
    fn pagesize_is_a_4_byte_int() {
        let v = query(&[ctl::CTL_HW, ctl::HW_PAGESIZE], &ctx()).unwrap();
        let b = v.to_bytes();
        assert_eq!(b.len(), 4);
        assert_eq!(u32::from_le_bytes(b.try_into().unwrap()), 4096);
    }

    #[test]
    fn osreldate_int_matches_advertised_release() {
        let v = query(&[ctl::CTL_KERN, ctl::KERN_OSRELDATE], &ctx()).unwrap();
        assert_eq!(
            u32::from_le_bytes(v.to_bytes().try_into().unwrap()),
            OSRELDATE
        );
    }

    #[test]
    fn usrstack_is_an_8_byte_long() {
        let v = query(&[ctl::CTL_KERN, ctl::KERN_USRSTACK], &ctx()).unwrap();
        let b = v.to_bytes();
        assert_eq!(b.len(), 8);
        assert_eq!(u64::from_le_bytes(b.try_into().unwrap()), 0x7fff_ffff_0000);
    }

    #[test]
    fn arnd_returns_16_bytes() {
        let v = query(&[ctl::CTL_KERN, ctl::KERN_ARND], &ctx()).unwrap();
        assert_eq!(v.to_bytes().len(), 16);
    }

    #[test]
    fn unknown_mib_is_none() {
        assert!(query(&[ctl::CTL_KERN, 9999], &ctx()).is_none());
        assert!(query(&[42, 1], &ctx()).is_none());
    }

    #[test]
    fn sysctlbyname_resolves_known_names() {
        assert_eq!(
            name_to_mib("hw.pagesize"),
            Some(alloc::vec![ctl::CTL_HW, ctl::HW_PAGESIZE])
        );
        assert_eq!(name_to_mib("kern.ostype").unwrap()[0], ctl::CTL_KERN);
        assert!(name_to_mib("kern.nonexistent").is_none());
    }
}
