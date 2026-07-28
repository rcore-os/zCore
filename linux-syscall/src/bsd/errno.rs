//! Translate a Linux `errno` (what every `sys_*` method returns) into the
//! FreeBSD `errno` a FreeBSD binary expects.
//!
//! The two systems agree on 1..=34 (`EPERM`..`ERANGE`) and then diverge. The
//! trap that makes a blind pass-through wrong is that `EAGAIN` and `EDEADLK`
//! are *swapped*: Linux has `EAGAIN = 11`, `EDEADLK = 35`; FreeBSD has
//! `EDEADLK = 11`, `EAGAIN = 35`. Handing a Linux `EAGAIN` (11) straight to a
//! FreeBSD program would read as `EDEADLK`, so the mapping is done value by
//! value against `sys/sys/errno.h`.

use super::consts::errno as bsd;
use linux_object::error::LxError;

/// Map a Linux `errno` integer to the FreeBSD one.
pub fn linux_to_freebsd(lin: i32) -> i32 {
    match lin {
        // Identical range: EPERM(1) .. ECHILD(10).
        1..=10 => lin,
        // The swap.
        11 => bsd::EAGAIN,  // Linux EAGAIN  -> FreeBSD 35
        35 => bsd::EDEADLK, // Linux EDEADLK -> FreeBSD 11
        // Identical range resumes: ENOMEM(12) .. ERANGE(34).
        12..=34 => lin,
        // Past 35 the numbers differ; map the ones LxError can produce.
        36 => bsd::ENAMETOOLONG,
        37 => bsd::ENOLCK,
        38 => bsd::ENOSYS,
        39 => bsd::ENOTEMPTY,
        40 => bsd::ELOOP,
        43 => bsd::EIDRM,
        61 => bsd::ENOATTR, // Linux ENODATA has no exact peer; ENOATTR is closest.
        88 => bsd::ENOTSOCK,
        90 => bsd::EMSGSIZE,
        92 => bsd::ENOPROTOOPT,
        95 => bsd::EOPNOTSUPP,
        96 => bsd::EPFNOSUPPORT,
        97 => bsd::EAFNOSUPPORT,
        98 => bsd::EADDRINUSE,
        105 => bsd::ENOBUFS,
        106 => bsd::EISCONN,
        107 => bsd::ENOTCONN,
        110 => bsd::ETIMEDOUT,
        111 => bsd::ECONNREFUSED,
        114 => bsd::EALREADY,
        115 => bsd::EINPROGRESS,
        // Unknown: EINVAL is the least-surprising fallback and cannot itself be
        // confused with success.
        _ => bsd::EINVAL,
    }
}

/// Convenience wrapper: map an [`LxError`] straight to a FreeBSD `errno`.
pub fn lx_to_freebsd(err: LxError) -> i32 {
    linux_to_freebsd(err as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_ranges_pass_through() {
        for n in 1..=10 {
            assert_eq!(linux_to_freebsd(n), n);
        }
        for n in 12..=34 {
            assert_eq!(linux_to_freebsd(n), n);
        }
    }

    #[test]
    fn eagain_and_edeadlk_are_swapped() {
        // The whole reason this table exists.
        assert_eq!(linux_to_freebsd(11), 35); // Linux EAGAIN  -> FreeBSD EAGAIN
        assert_eq!(linux_to_freebsd(35), 11); // Linux EDEADLK -> FreeBSD EDEADLK
    }

    #[test]
    fn diverging_values_map_to_freebsd_numbers() {
        assert_eq!(linux_to_freebsd(38), 78); // ENOSYS
        assert_eq!(linux_to_freebsd(40), 62); // ELOOP
        assert_eq!(linux_to_freebsd(36), 63); // ENAMETOOLONG
        assert_eq!(linux_to_freebsd(110), 60); // ETIMEDOUT
        assert_eq!(linux_to_freebsd(115), 36); // EINPROGRESS
    }

    #[test]
    fn lxerror_wrapper_matches() {
        assert_eq!(lx_to_freebsd(LxError::ENOSYS), 78);
        assert_eq!(lx_to_freebsd(LxError::EAGAIN), 35);
        assert_eq!(lx_to_freebsd(LxError::EPERM), 1);
    }

    #[test]
    fn unknown_falls_back_to_einval() {
        assert_eq!(linux_to_freebsd(9999), 22);
    }
}
