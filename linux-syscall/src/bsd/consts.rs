//! FreeBSD/amd64 ABI constants, transcribed from `freebsd-src`.
//!
//! Every value here is copied verbatim from a FreeBSD source header so the
//! numbers a FreeBSD userland binary passes in match what this layer expects.
//! The originating file is cited next to each block; keeping the provenance in
//! the comments is what lets a reviewer diff a group against upstream without
//! guessing which header it came from.

#![allow(dead_code)]
// This file is a transcription of C headers: the value *is* the documentation,
// and one-line doc comments on 200-odd constants would only add noise. The
// module-level docs above carry the provenance instead.
#![allow(missing_docs)]

/// FreeBSD/amd64 system-call numbers (`sys/sys/syscall.h`).
///
/// Only the numbers this layer knows how to service are listed; the dispatcher
/// answers any other number with `ENOSYS`, exactly as a FreeBSD kernel would
/// for a syscall compiled out of the running kernel.
pub mod sys {
    pub const SYSCALL: usize = 0;
    pub const EXIT: usize = 1;
    pub const FORK: usize = 2;
    pub const READ: usize = 3;
    pub const WRITE: usize = 4;
    pub const OPEN: usize = 5;
    pub const CLOSE: usize = 6;
    pub const WAIT4: usize = 7;
    pub const LINK: usize = 9;
    pub const UNLINK: usize = 10;
    pub const CHDIR: usize = 12;
    pub const FCHDIR: usize = 13;
    pub const CHMOD: usize = 15;
    pub const CHOWN: usize = 16;
    pub const BREAK: usize = 17;
    pub const GETPID: usize = 20;
    pub const SETUID: usize = 23;
    pub const GETUID: usize = 24;
    pub const GETEUID: usize = 25;
    pub const ACCESS: usize = 33;
    pub const SYNC: usize = 36;
    pub const KILL: usize = 37;
    pub const GETPPID: usize = 39;
    pub const DUP: usize = 41;
    pub const GETEGID: usize = 43;
    pub const GETGID: usize = 47;
    pub const IOCTL: usize = 54;
    pub const EXECVE: usize = 59;
    pub const UMASK: usize = 60;
    pub const MSYNC: usize = 65;
    pub const VFORK: usize = 66;
    pub const MUNMAP: usize = 73;
    pub const MPROTECT: usize = 74;
    pub const MADVISE: usize = 75;
    pub const GETPGRP: usize = 81;
    pub const SETPGID: usize = 82;
    pub const SETITIMER: usize = 83;
    pub const DUP2: usize = 90;
    pub const FCNTL: usize = 92;
    pub const SELECT: usize = 93;
    pub const FSYNC: usize = 95;
    pub const SETPRIORITY: usize = 96;
    pub const SOCKET: usize = 97;
    pub const CONNECT: usize = 98;
    pub const GETPRIORITY: usize = 100;
    pub const BIND: usize = 104;
    pub const SETSOCKOPT: usize = 105;
    pub const LISTEN: usize = 106;
    pub const GETTIMEOFDAY: usize = 116;
    pub const GETRUSAGE: usize = 117;
    pub const GETSOCKOPT: usize = 118;
    pub const READV: usize = 120;
    pub const WRITEV: usize = 121;
    pub const FCHOWN: usize = 123;
    pub const FCHMOD: usize = 124;
    pub const RENAME: usize = 128;
    pub const FLOCK: usize = 131;
    pub const SENDTO: usize = 133;
    pub const SHUTDOWN: usize = 134;
    pub const SOCKETPAIR: usize = 135;
    pub const MKDIR: usize = 136;
    pub const RMDIR: usize = 137;
    pub const SETSID: usize = 147;
    pub const SYSARCH: usize = 165;
    pub const SETGID: usize = 181;
    pub const SETEGID: usize = 182;
    pub const SETEUID: usize = 183;
    pub const PATHCONF: usize = 191;
    pub const FPATHCONF: usize = 192;
    pub const GETRLIMIT: usize = 194;
    pub const SETRLIMIT: usize = 195;
    pub const UNDER_SYSCALL: usize = 198; // __syscall
    pub const SYSCTL: usize = 202; // __sysctl
    pub const MUNLOCK: usize = 204;
    pub const GETPGID: usize = 207;
    pub const POLL: usize = 209;
    pub const CLOCK_GETTIME: usize = 232;
    pub const CLOCK_SETTIME: usize = 233;
    pub const CLOCK_GETRES: usize = 234;
    pub const NANOSLEEP: usize = 240;
    pub const CLOCK_NANOSLEEP: usize = 244;
    pub const ISSETUGID: usize = 253;
    pub const GETDIRENTRIES_OLD: usize = 196; // legacy freebsd11 getdirentries
    pub const GETSID: usize = 310;
    pub const SETRESUID: usize = 311;
    pub const SETRESGID: usize = 312;
    pub const YIELD: usize = 321;
    pub const GETCWD: usize = 326; // __getcwd
    pub const SCHED_YIELD: usize = 331;
    pub const SIGPROCMASK: usize = 340;
    pub const SIGACTION: usize = 416;
    pub const SIGRETURN: usize = 417;
    pub const GETRESUID: usize = 360;
    pub const GETRESGID: usize = 361;
    pub const KQUEUE: usize = 362;
    pub const SIGWAIT: usize = 429;
    pub const THR_CREATE: usize = 430;
    pub const THR_EXIT: usize = 431;
    pub const THR_SELF: usize = 432;
    pub const THR_KILL: usize = 433;
    pub const UMTX_OP: usize = 454; // _umtx_op
    pub const THR_NEW: usize = 455;
    pub const THR_SET_NAME: usize = 464;
    pub const PREAD: usize = 475;
    pub const PWRITE: usize = 476;
    pub const MMAP: usize = 477;
    pub const LSEEK: usize = 478;
    pub const TRUNCATE: usize = 479;
    pub const FTRUNCATE: usize = 480;
    pub const CPUSET_GETAFFINITY: usize = 487;
    pub const CPUSET_SETAFFINITY: usize = 488;
    pub const FACCESSAT: usize = 489;
    pub const FCHMODAT: usize = 490;
    pub const FCHOWNAT: usize = 491;
    pub const LINKAT: usize = 495;
    pub const MKDIRAT: usize = 496;
    pub const OPENAT: usize = 499;
    pub const READLINKAT: usize = 500;
    pub const RENAMEAT: usize = 501;
    pub const SYMLINKAT: usize = 502;
    pub const UNLINKAT: usize = 503;
    pub const POSIX_FALLOCATE: usize = 530;
    pub const POSIX_FADVISE: usize = 531;
    pub const PPOLL: usize = 545;
    pub const UTIMENSAT: usize = 547;
    pub const FDATASYNC: usize = 550;
    pub const FSTAT: usize = 551;
    pub const FSTATAT: usize = 552;
    pub const GETDIRENTRIES: usize = 554;
    pub const STATFS: usize = 555;
    pub const FSTATFS: usize = 556;
    pub const MKNODAT: usize = 559;
    pub const GETRANDOM: usize = 563;
    pub const COPY_FILE_RANGE: usize = 569;
    pub const SYSCTLBYNAME: usize = 570; // __sysctlbyname
    pub const CLOSE_RANGE: usize = 575;
    pub const SCHED_GETCPU: usize = 581;
}

/// FreeBSD `errno` values (`sys/sys/errno.h`).
///
/// FreeBSD and Linux agree through 34 (`ERANGE`) and diverge after it, so a
/// Linux `errno` cannot be handed back to a FreeBSD binary unchanged — see
/// [`super::errno::linux_to_freebsd`].
pub mod errno {
    pub const EPERM: i32 = 1;
    pub const ENOENT: i32 = 2;
    pub const ESRCH: i32 = 3;
    pub const EINTR: i32 = 4;
    pub const EIO: i32 = 5;
    pub const ENXIO: i32 = 6;
    pub const E2BIG: i32 = 7;
    pub const ENOEXEC: i32 = 8;
    pub const EBADF: i32 = 9;
    pub const ECHILD: i32 = 10;
    pub const EDEADLK: i32 = 11;
    pub const ENOMEM: i32 = 12;
    pub const EACCES: i32 = 13;
    pub const EFAULT: i32 = 14;
    pub const ENOTBLK: i32 = 15;
    pub const EBUSY: i32 = 16;
    pub const EEXIST: i32 = 17;
    pub const EXDEV: i32 = 18;
    pub const ENODEV: i32 = 19;
    pub const ENOTDIR: i32 = 20;
    pub const EISDIR: i32 = 21;
    pub const EINVAL: i32 = 22;
    pub const ENFILE: i32 = 23;
    pub const EMFILE: i32 = 24;
    pub const ENOTTY: i32 = 25;
    pub const ETXTBSY: i32 = 26;
    pub const EFBIG: i32 = 27;
    pub const ENOSPC: i32 = 28;
    pub const ESPIPE: i32 = 29;
    pub const EROFS: i32 = 30;
    pub const EMLINK: i32 = 31;
    pub const EPIPE: i32 = 32;
    pub const EDOM: i32 = 33;
    pub const ERANGE: i32 = 34;
    pub const EAGAIN: i32 = 35;
    pub const EINPROGRESS: i32 = 36;
    pub const EALREADY: i32 = 37;
    pub const ENOTSOCK: i32 = 38;
    pub const EDESTADDRREQ: i32 = 39;
    pub const EMSGSIZE: i32 = 40;
    pub const EPROTOTYPE: i32 = 41;
    pub const ENOPROTOOPT: i32 = 42;
    pub const EPROTONOSUPPORT: i32 = 43;
    pub const EOPNOTSUPP: i32 = 45;
    pub const EPFNOSUPPORT: i32 = 46;
    pub const EAFNOSUPPORT: i32 = 47;
    pub const EADDRINUSE: i32 = 48;
    pub const EADDRNOTAVAIL: i32 = 49;
    pub const ENETDOWN: i32 = 50;
    pub const ENETUNREACH: i32 = 51;
    pub const ECONNABORTED: i32 = 53;
    pub const ECONNRESET: i32 = 54;
    pub const ENOBUFS: i32 = 55;
    pub const EISCONN: i32 = 56;
    pub const ENOTCONN: i32 = 57;
    pub const ETIMEDOUT: i32 = 60;
    pub const ECONNREFUSED: i32 = 61;
    pub const ELOOP: i32 = 62;
    pub const ENAMETOOLONG: i32 = 63;
    pub const EHOSTUNREACH: i32 = 65;
    pub const ENOTEMPTY: i32 = 66;
    pub const EDQUOT: i32 = 69;
    pub const ENOLCK: i32 = 77;
    pub const ENOSYS: i32 = 78;
    pub const EIDRM: i32 = 82;
    pub const ENOMSG: i32 = 83;
    pub const EOVERFLOW: i32 = 84;
    pub const ECANCELED: i32 = 85;
    pub const EILSEQ: i32 = 86;
    pub const ENOATTR: i32 = 87;
    pub const EBADMSG: i32 = 89;
    pub const ENOTRECOVERABLE: i32 = 95;
    pub const EOWNERDEAD: i32 = 96;
}

/// FreeBSD `open(2)` / `fcntl(2)` flags (`sys/sys/fcntl.h`).
pub mod oflags {
    pub const O_RDONLY: i32 = 0x0000;
    pub const O_WRONLY: i32 = 0x0001;
    pub const O_RDWR: i32 = 0x0002;
    pub const O_ACCMODE: i32 = 0x0003;
    pub const O_NONBLOCK: i32 = 0x0004;
    pub const O_APPEND: i32 = 0x0008;
    pub const O_ASYNC: i32 = 0x0040;
    pub const O_FSYNC: i32 = 0x0080;
    pub const O_NOFOLLOW: i32 = 0x0100;
    pub const O_CREAT: i32 = 0x0200;
    pub const O_TRUNC: i32 = 0x0400;
    pub const O_EXCL: i32 = 0x0800;
    pub const O_NOCTTY: i32 = 0x8000;
    pub const O_DIRECT: i32 = 0x0001_0000;
    pub const O_DIRECTORY: i32 = 0x0002_0000;
    pub const O_EXEC: i32 = 0x0004_0000;
    pub const O_CLOEXEC: i32 = 0x0010_0000;
    pub const O_PATH: i32 = 0x0040_0000;
    pub const O_DSYNC: i32 = 0x0100_0000;

    /// `AT_FDCWD` for the `*at` syscalls (`sys/sys/fcntl.h`). FreeBSD's value is
    /// `-100`, the same as Linux's, but it is spelled out here so the
    /// translation is explicit rather than relying on the coincidence.
    pub const AT_FDCWD: i32 = -100;
    pub const AT_EACCESS: i32 = 0x0100;
    pub const AT_SYMLINK_NOFOLLOW: i32 = 0x0200;
    pub const AT_SYMLINK_FOLLOW: i32 = 0x0400;
    pub const AT_REMOVEDIR: i32 = 0x0800;
    pub const AT_RESOLVE_BENEATH: i32 = 0x2000;
    pub const AT_EMPTY_PATH: i32 = 0x4000;
}

/// Linux `open(2)` / `*at` flags, for the target side of the translation
/// (`include/uapi/asm-generic/fcntl.h`). Only the bits that differ from
/// FreeBSD are interesting, but the full set the loader's `sys_openat`
/// understands is spelled out.
pub mod lin_oflags {
    pub const O_RDONLY: i32 = 0o0;
    pub const O_WRONLY: i32 = 0o1;
    pub const O_RDWR: i32 = 0o2;
    pub const O_CREAT: i32 = 0o100;
    pub const O_EXCL: i32 = 0o200;
    pub const O_NOCTTY: i32 = 0o400;
    pub const O_TRUNC: i32 = 0o1000;
    pub const O_APPEND: i32 = 0o2000;
    pub const O_NONBLOCK: i32 = 0o4000;
    pub const O_DSYNC: i32 = 0o10000;
    pub const O_DIRECT: i32 = 0o40000;
    pub const O_DIRECTORY: i32 = 0o200000;
    pub const O_NOFOLLOW: i32 = 0o400000;
    pub const O_CLOEXEC: i32 = 0o2000000;
    pub const O_PATH: i32 = 0o10000000;

    pub const AT_FDCWD: i32 = -100;
    pub const AT_SYMLINK_NOFOLLOW: i32 = 0x100;
    pub const AT_REMOVEDIR: i32 = 0x200;
    pub const AT_SYMLINK_FOLLOW: i32 = 0x400;
    pub const AT_EACCESS: i32 = 0x200;
    pub const AT_EMPTY_PATH: i32 = 0x1000;
}

/// FreeBSD `mmap(2)` flags (`sys/sys/mman.h`). `PROT_*` match Linux; only the
/// `MAP_*` placement/backing bits differ.
pub mod mman {
    pub const MAP_SHARED: i32 = 0x0001;
    pub const MAP_PRIVATE: i32 = 0x0002;
    pub const MAP_FIXED: i32 = 0x0010;
    pub const MAP_STACK: i32 = 0x0400;
    pub const MAP_NOSYNC: i32 = 0x0800;
    pub const MAP_ANON: i32 = 0x1000;
    pub const MAP_GUARD: i32 = 0x0000_2000;
    pub const MAP_EXCL: i32 = 0x0000_4000;
    pub const MAP_NOCORE: i32 = 0x0002_0000;
    pub const MAP_PREFAULT_READ: i32 = 0x0004_0000;
    pub const MAP_32BIT: i32 = 0x0008_0000;
}

/// Linux `mmap(2)` flags (`include/uapi/asm-generic/mman-common.h`).
pub mod lin_mman {
    pub const MAP_SHARED: i32 = 0x01;
    pub const MAP_PRIVATE: i32 = 0x02;
    pub const MAP_FIXED: i32 = 0x10;
    pub const MAP_ANONYMOUS: i32 = 0x20;
    pub const MAP_STACK: i32 = 0x2_0000;
    pub const MAP_NORESERVE: i32 = 0x4000;
}

/// `sysctl(2)` top-level identifiers and the `kern.*` / `hw.*` leaves this
/// layer answers (`sys/sys/sysctl.h`).
pub mod ctl {
    pub const CTL_KERN: i32 = 1;
    pub const CTL_VM: i32 = 2;
    pub const CTL_HW: i32 = 6;

    pub const KERN_OSTYPE: i32 = 1;
    pub const KERN_OSRELEASE: i32 = 2;
    pub const KERN_OSREV: i32 = 3;
    pub const KERN_VERSION: i32 = 4;
    pub const KERN_MAXFILES: i32 = 7;
    pub const KERN_ARGMAX: i32 = 8;
    pub const KERN_HOSTNAME: i32 = 10;
    pub const KERN_OSRELDATE: i32 = 24;
    pub const KERN_PS_STRINGS: i32 = 32;
    pub const KERN_USRSTACK: i32 = 33;
    pub const KERN_IOV_MAX: i32 = 35;
    pub const KERN_ARND: i32 = 37;

    pub const HW_MACHINE: i32 = 1;
    pub const HW_MODEL: i32 = 2;
    pub const HW_NCPU: i32 = 3;
    pub const HW_BYTEORDER: i32 = 4;
    pub const HW_PHYSMEM: i32 = 5;
    pub const HW_USERMEM: i32 = 6;
    pub const HW_PAGESIZE: i32 = 7;
    pub const HW_MACHINE_ARCH: i32 = 11;
}

/// FreeBSD file-type bits and directory-entry types (`sys/sys/stat.h`,
/// `sys/sys/dirent.h`). Mode bits below `S_IFMT` (permissions, setuid/gid,
/// sticky) are identical to Linux, so only the type field needs care — and it,
/// too, happens to match, but is duplicated here to document the layout.
pub mod stat {
    pub const S_IFMT: u32 = 0o170_000;
    pub const S_IFIFO: u32 = 0o010_000;
    pub const S_IFCHR: u32 = 0o020_000;
    pub const S_IFDIR: u32 = 0o040_000;
    pub const S_IFBLK: u32 = 0o060_000;
    pub const S_IFREG: u32 = 0o100_000;
    pub const S_IFLNK: u32 = 0o120_000;
    pub const S_IFSOCK: u32 = 0o140_000;

    pub const DT_UNKNOWN: u8 = 0;
    pub const DT_FIFO: u8 = 1;
    pub const DT_CHR: u8 = 2;
    pub const DT_DIR: u8 = 4;
    pub const DT_BLK: u8 = 6;
    pub const DT_REG: u8 = 8;
    pub const DT_LNK: u8 = 10;
    pub const DT_SOCK: u8 = 12;
}

/// `sysarch(2)` operation codes for amd64 (`sys/x86/include/sysarch.h`).
pub mod sysarch {
    pub const AMD64_GET_FSBASE: i32 = 128;
    pub const AMD64_SET_FSBASE: i32 = 129;
    pub const AMD64_GET_GSBASE: i32 = 130;
    pub const AMD64_SET_GSBASE: i32 = 131;
}

/// ELF auxiliary-vector types for FreeBSD (`sys/sys/elf_common.h`).
///
/// The low entries (`AT_PHDR` = 3 .. `AT_ENTRY` = 9) share their numbers with
/// Linux; everything from `AT_EXECPATH` (15) up is FreeBSD-specific and does
/// not exist on Linux. A FreeBSD `crt1`/`rtld` reads these to find its program
/// headers, page size, SSP canary and the `ps_strings` block.
pub mod auxv {
    pub const AT_NULL: u64 = 0;
    pub const AT_IGNORE: u64 = 1;
    pub const AT_EXECFD: u64 = 2;
    pub const AT_PHDR: u64 = 3;
    pub const AT_PHENT: u64 = 4;
    pub const AT_PHNUM: u64 = 5;
    pub const AT_PAGESZ: u64 = 6;
    pub const AT_BASE: u64 = 7;
    pub const AT_FLAGS: u64 = 8;
    pub const AT_ENTRY: u64 = 9;
    pub const AT_NOTELF: u64 = 10;
    pub const AT_EXECPATH: u64 = 15;
    pub const AT_CANARY: u64 = 16;
    pub const AT_CANARYLEN: u64 = 17;
    pub const AT_OSRELDATE: u64 = 18;
    pub const AT_NCPUS: u64 = 19;
    pub const AT_PAGESIZES: u64 = 20;
    pub const AT_PAGESIZESLEN: u64 = 21;
    pub const AT_TIMEKEEP: u64 = 22;
    pub const AT_STACKPROT: u64 = 23;
    pub const AT_EHDRFLAGS: u64 = 24;
    pub const AT_HWCAP: u64 = 25;
    pub const AT_HWCAP2: u64 = 26;
    pub const AT_BSDFLAGS: u64 = 27;
    pub const AT_ARGC: u64 = 28;
    pub const AT_ARGV: u64 = 29;
    pub const AT_ENVC: u64 = 30;
    pub const AT_ENVV: u64 = 31;
    pub const AT_PS_STRINGS: u64 = 32;
    pub const AT_USRSTACKBASE: u64 = 35;
    pub const AT_USRSTACKLIM: u64 = 36;
}

/// A plausible `__FreeBSD_version` for the ABI we advertise (FreeBSD 14.0).
/// libc's `__getosreldate()` and several `sysctl` leaves report it; programs
/// gate feature use on it, so it must parse as a real release.
pub const OSRELDATE: u32 = 1_400_097;

/// `KERN_OSRELEASE` string paired with [`OSRELDATE`].
pub const OSRELEASE: &str = "14.0-RELEASE";

/// `KERN_OSTYPE`.
pub const OSTYPE: &str = "FreeBSD";

/// `HW_MACHINE` / `HW_MACHINE_ARCH` for amd64.
pub const MACHINE: &str = "amd64";
