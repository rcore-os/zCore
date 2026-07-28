//! Bit-for-bit translation of the flag words whose numeric layout differs
//! between FreeBSD and Linux. A FreeBSD binary passes FreeBSD flag bits; the
//! `sys_*` methods this layer reuses parse Linux flag bits. Passing the raw
//! FreeBSD word through would silently mean something else — e.g. FreeBSD
//! `O_CREAT` (0x0200) is Linux `O_NOCTTY|O_DIRECTORY`-adjacent noise, and
//! FreeBSD `O_CLOEXEC` (0x0010_0000) is a bit Linux ignores entirely.

use super::consts::{lin_mman, lin_oflags, mman, oflags};

/// Translate FreeBSD `open(2)`/`openat(2)` flags to the Linux flag word that
/// [`crate::Syscall::sys_openat`] understands.
///
/// The access mode lives in the low two bits and is identical on both systems
/// (`O_RDONLY`=0, `O_WRONLY`=1, `O_RDWR`=2), so it is copied straight through;
/// every other bit is remapped individually.
pub fn open_flags_to_linux(bsd: i32) -> i32 {
    let mut out = bsd & oflags::O_ACCMODE; // access mode: same encoding.
    let map = [
        (oflags::O_NONBLOCK, lin_oflags::O_NONBLOCK),
        (oflags::O_APPEND, lin_oflags::O_APPEND),
        (oflags::O_CREAT, lin_oflags::O_CREAT),
        (oflags::O_TRUNC, lin_oflags::O_TRUNC),
        (oflags::O_EXCL, lin_oflags::O_EXCL),
        (oflags::O_NOCTTY, lin_oflags::O_NOCTTY),
        (oflags::O_NOFOLLOW, lin_oflags::O_NOFOLLOW),
        (oflags::O_DIRECTORY, lin_oflags::O_DIRECTORY),
        (oflags::O_CLOEXEC, lin_oflags::O_CLOEXEC),
        (oflags::O_DIRECT, lin_oflags::O_DIRECT),
        (oflags::O_PATH, lin_oflags::O_PATH),
        (oflags::O_DSYNC, lin_oflags::O_DSYNC),
    ];
    for (from, to) in map {
        if bsd & from != 0 {
            out |= to;
        }
    }
    out
}

/// Translate FreeBSD `*at` flags (the `AT_*` word) to Linux.
pub fn at_flags_to_linux(bsd: i32) -> i32 {
    let mut out = 0;
    let map = [
        (oflags::AT_SYMLINK_NOFOLLOW, lin_oflags::AT_SYMLINK_NOFOLLOW),
        (oflags::AT_SYMLINK_FOLLOW, lin_oflags::AT_SYMLINK_FOLLOW),
        (oflags::AT_EACCESS, lin_oflags::AT_EACCESS),
        (oflags::AT_REMOVEDIR, lin_oflags::AT_REMOVEDIR),
        (oflags::AT_EMPTY_PATH, lin_oflags::AT_EMPTY_PATH),
    ];
    for (from, to) in map {
        if bsd & from != 0 {
            out |= to;
        }
    }
    out
}

/// Translate FreeBSD `mmap(2)` flags to Linux. `PROT_*` are identical on both
/// systems and never pass through here — only the `MAP_*` word is remapped.
pub fn mmap_flags_to_linux(bsd: i32) -> i32 {
    let mut out = 0;
    let map = [
        (mman::MAP_SHARED, lin_mman::MAP_SHARED),
        (mman::MAP_PRIVATE, lin_mman::MAP_PRIVATE),
        (mman::MAP_FIXED, lin_mman::MAP_FIXED),
        (mman::MAP_ANON, lin_mman::MAP_ANONYMOUS),
        (mman::MAP_STACK, lin_mman::MAP_STACK),
    ];
    for (from, to) in map {
        if bsd & from != 0 {
            out |= to;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_mode_is_preserved() {
        assert_eq!(open_flags_to_linux(oflags::O_RDONLY) & 3, 0);
        assert_eq!(open_flags_to_linux(oflags::O_WRONLY) & 3, 1);
        assert_eq!(open_flags_to_linux(oflags::O_RDWR) & 3, 2);
    }

    #[test]
    fn create_flags_land_on_the_right_linux_bits() {
        // FreeBSD O_CREAT|O_TRUNC|O_WRONLY -> Linux O_CREAT|O_TRUNC|O_WRONLY.
        let bsd = oflags::O_WRONLY | oflags::O_CREAT | oflags::O_TRUNC;
        let lin = open_flags_to_linux(bsd);
        assert_eq!(lin & 3, 1);
        assert_ne!(lin & lin_oflags::O_CREAT, 0);
        assert_ne!(lin & lin_oflags::O_TRUNC, 0);
        // And it must NOT accidentally set the raw FreeBSD bit positions.
        assert_eq!(lin & lin_oflags::O_NONBLOCK, 0);
    }

    #[test]
    fn cloexec_is_remapped_not_passed_through() {
        // FreeBSD O_CLOEXEC is 0x0010_0000; Linux is 0x0008_0000. A pass-through
        // would leave CLOEXEC unset on the Linux side.
        let lin = open_flags_to_linux(oflags::O_CLOEXEC);
        assert_eq!(lin, lin_oflags::O_CLOEXEC);
    }

    #[test]
    fn at_nofollow_moves_from_0x200_to_0x100() {
        assert_eq!(
            at_flags_to_linux(oflags::AT_SYMLINK_NOFOLLOW),
            lin_oflags::AT_SYMLINK_NOFOLLOW
        );
        // AT_SYMLINK_FOLLOW happens to share 0x400 on both.
        assert_eq!(
            at_flags_to_linux(oflags::AT_SYMLINK_FOLLOW),
            lin_oflags::AT_SYMLINK_FOLLOW
        );
    }

    #[test]
    fn mmap_anon_moves_from_0x1000_to_0x20() {
        let lin = mmap_flags_to_linux(mman::MAP_PRIVATE | mman::MAP_ANON);
        assert_ne!(lin & lin_mman::MAP_PRIVATE, 0);
        assert_ne!(lin & lin_mman::MAP_ANONYMOUS, 0);
        assert_eq!(lin & !(lin_mman::MAP_PRIVATE | lin_mman::MAP_ANONYMOUS), 0);
    }
}
