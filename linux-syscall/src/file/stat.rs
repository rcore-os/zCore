//! File status
//!
//! - stat
//! - lstat
//! - fstat(at)

use super::*;
use linux_object::fs::vfs::{FileType, Metadata};

impl Syscall<'_> {
    /// Works exactly like the stat syscall, but if the file in question is a symbolic link,
    /// information on the link is returned rather than its target.
    /// - `path` – full path to file
    /// - `stat_ptr` – pointer to stat buffer
    pub fn sys_lstat(&self, path: UserInPtr<u8>, stat_ptr: UserOutPtr<Stat>) -> SysResult {
        self.sys_fstatat(
            FileDesc::CWD,
            path,
            stat_ptr,
            AtFlags::SYMLINK_NOFOLLOW.bits(),
        )
    }

    /// Works exactly like the stat syscall except a file descriptor (fd) is provided instead of a path.
    /// - `fd` – file descriptor
    /// - `stat_ptr` – pointer to stat buffer
    pub fn sys_fstat(&self, fd: FileDesc, mut stat_ptr: UserOutPtr<Stat>) -> SysResult {
        info!("fstat: fd={:?}, stat_ptr={:?}", fd, stat_ptr);

        let meta = self.linux_process().get_file(fd)?.metadata()?;
        stat_ptr.write(meta.into())?;
        Ok(0)
    }

    /// get file status relative to a directory file descriptor
    pub fn sys_fstatat(
        &self,
        dirfd: FileDesc,
        path: UserInPtr<u8>,
        mut stat_ptr: UserOutPtr<Stat>,
        flags: usize,
    ) -> SysResult {
        let path = path.as_c_str()?;
        let flags = AtFlags::from_bits_truncate(flags);
        info!(
            "fstatat: dirfd={:?}, path={:?}, stat_ptr={:?}, flags={:?}",
            dirfd, path, stat_ptr, flags
        );

        let follow = !flags.contains(AtFlags::SYMLINK_NOFOLLOW);
        let inode = self.linux_process().lookup_inode_at(dirfd, path, follow)?;
        let stat = inode.metadata()?;
        stat_ptr.write(stat.into())?;
        Ok(0)
    }

    /// Returns information about a file in a structure named stat.
    /// - `path` – pointer to the name of the file
    /// - `stat_ptr` –  pointer to the structure to receive file information
    pub fn sys_stat(&self, path: UserInPtr<u8>, stat_ptr: UserOutPtr<Stat>) -> SysResult {
        self.sys_fstatat(FileDesc::CWD, path, stat_ptr, 0)
    }

    /// statx system call
    pub fn sys_statx(
        &self,
        dirfd: FileDesc,
        pathname: UserInPtr<u8>,
        flags: usize,
        _mask: u32,
        mut statxbuf: UserOutPtr<Statx>,
    ) -> SysResult {
        let flags = AtFlags::from_bits_truncate(flags);
        let follow = !flags.contains(AtFlags::SYMLINK_NOFOLLOW);
        let proc = self.linux_process();
        let inode = if flags.contains(AtFlags::EMPTY_PATH)
            && (pathname.is_null() || pathname.as_c_str().map(|s| s.is_empty()).unwrap_or(false))
        {
            if dirfd == FileDesc::CWD {
                proc.root_inode()
                    .lookup(&proc.current_working_directory())?
            } else {
                proc.get_file(dirfd)?.inode()
            }
        } else {
            let path = pathname.as_c_str()?;
            proc.lookup_inode_at(dirfd, path, follow)?
        };

        let stat = inode.metadata()?;
        statxbuf.write(stat.into())?;
        Ok(0)
    }
}

#[cfg(not(target_arch = "mips"))]
use linux_object::time::TimeSpec;

#[cfg(target_arch = "mips")]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct TimeSpec {
    pub sec: i32,
    pub nsec: i32,
}

#[cfg(target_arch = "mips")]
impl From<linux_object::fs::vfs::TimeSpec> for TimeSpec {
    fn from(t: TimeSpec) -> Self {
        TimeSpec {
            sec: t.sec as _,
            nsec: t.nsec as _,
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[repr(C)]
#[derive(Debug)]
pub struct Stat {
    /// ID of device containing file
    dev: u64,
    /// inode number
    ino: u64,
    /// number of hard links
    nlink: u64,

    /// file type and mode
    mode: StatMode,
    /// user ID of owner
    uid: u32,
    /// group ID of owner
    gid: u32,
    /// padding
    _pad0: u32,
    /// device ID (if special file)
    rdev: u64,
    /// total size, in bytes
    size: u64,
    /// blocksize for filesystem I/O
    blksize: u64,
    /// number of 512B blocks allocated
    blocks: u64,

    /// last access time
    atime: TimeSpec,
    /// last modification time
    mtime: TimeSpec,
    /// last status change time
    ctime: TimeSpec,
}

#[cfg(target_arch = "mips")]
#[repr(C)]
#[derive(Debug)]
pub struct Stat {
    /// ID of device containing file
    dev: u64,
    /// padding
    _pad0: u64,
    /// inode number
    ino: u64,
    /// file type and mode
    mode: StatMode,
    /// number of hard links
    nlink: u32,

    /// user ID of owner
    uid: u32,
    /// group ID of owner
    gid: u32,
    /// device ID (if special file)
    rdev: u64,
    /// padding
    _pad1: u64,
    /// total size, in bytes
    size: u64,

    /// last access time
    atime: TimeSpec,
    /// last modification time
    mtime: TimeSpec,
    /// last status change time
    ctime: TimeSpec,

    /// blocksize for filesystem I/O
    blksize: u32,
    /// padding
    _pad2: u32,
    /// number of 512B blocks allocated
    blocks: u64,
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "mips")))]
#[repr(C)]
#[derive(Debug)]
pub struct Stat {
    /// ID of device containing file
    dev: u64,
    /// inode number
    ino: u64,
    /// file type and mode
    mode: StatMode,
    /// number of hard links
    nlink: u32,

    /// user ID of owner
    uid: u32,
    /// group ID of owner
    gid: u32,
    /// device ID (if special file)
    rdev: u64,
    /// padding
    _pad0: u64,
    /// total size, in bytes
    size: u64,
    /// blocksize for filesystem I/O
    blksize: u32,
    /// padding
    _pad1: u32,
    /// number of 512B blocks allocated
    blocks: u64,

    /// last access time
    atime: TimeSpec,
    /// last modification time
    mtime: TimeSpec,
    /// last status change time
    ctime: TimeSpec,
}

impl From<Metadata> for Stat {
    fn from(info: Metadata) -> Self {
        Stat {
            dev: info.dev as _,
            ino: info.inode as _,
            mode: StatMode::from_type_mode(info.type_, info.mode as _),
            nlink: info.nlinks as _,
            uid: info.uid as _,
            gid: info.gid as _,
            rdev: info.rdev as _,
            size: info.size as _,
            blksize: info.blk_size as _,
            blocks: info.blocks as _,
            atime: info.atime.into(),
            mtime: info.mtime.into(),
            ctime: info.ctime.into(),
            _pad0: 0,
            #[cfg(not(target_arch = "x86_64"))]
            _pad1: 0,
            #[cfg(target_arch = "mips")]
            _pad2: 0,
        }
    }
}

bitflags! {
    pub struct StatMode: u32 {
        /// Type
        const TYPE_MASK = 0o170_000;
        /// FIFO
        const FIFO  = 0o010_000;
        /// character device
        const CHAR  = 0o020_000;
        /// directory
        const DIR   = 0o040_000;
        /// block device
        const BLOCK = 0o060_000;
        /// ordinary regular file
        const FILE  = 0o100_000;
        /// symbolic link
        const LINK  = 0o120_000;
        /// socket
        const SOCKET = 0o140_000;

        /// Set-user-ID on execution.
        const SET_UID = 0o4_000;
        /// Set-group-ID on execution.
        const SET_GID = 0o2_000;

        /// Read, write, execute/search by owner.
        const OWNER_MASK = 0o700;
        /// Read permission, owner.
        const OWNER_READ = 0o400;
        /// Write permission, owner.
        const OWNER_WRITE = 0o200;
        /// Execute/search permission, owner.
        const OWNER_EXEC = 0o100;

        /// Read, write, execute/search by group.
        const GROUP_MASK = 0o70;
        /// Read permission, group.
        const GROUP_READ = 0o40;
        /// Write permission, group.
        const GROUP_WRITE = 0o20;
        /// Execute/search permission, group.
        const GROUP_EXEC = 0o10;

        /// Read, write, execute/search by others.
        const OTHER_MASK = 0o7;
        /// Read permission, others.
        const OTHER_READ = 0o4;
        /// Write permission, others.
        const OTHER_WRITE = 0o2;
        /// Execute/search permission, others.
        const OTHER_EXEC = 0o1;
    }
}

impl StatMode {
    fn from_type_mode(type_: FileType, mode: u16) -> Self {
        let type_ = match type_ {
            FileType::File => StatMode::FILE,
            FileType::Dir => StatMode::DIR,
            FileType::SymLink => StatMode::LINK,
            FileType::CharDevice => StatMode::CHAR,
            FileType::BlockDevice => StatMode::BLOCK,
            FileType::Socket => StatMode::SOCKET,
            FileType::NamedPipe => StatMode::FIFO,
        };
        let mode = StatMode::from_bits_truncate(mode as u32);
        type_ | mode
    }
}

const STATX_BASIC_STATS: u32 = 0x07ff;

/// timestamp structure for statx
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct StatxTimestamp {
    /// seconds
    pub tv_sec: i64,
    /// nanoseconds
    pub tv_nsec: u32,
    /// reserved
    pub __reserved: i32,
}

/// statx structure
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Statx {
    /// mask
    pub stx_mask: u32,
    /// block size
    pub stx_blksize: u32,
    /// attributes
    pub stx_attributes: u64,
    /// hard links
    pub stx_nlink: u32,
    /// owner uid
    pub stx_uid: u32,
    /// owner gid
    pub stx_gid: u32,
    /// mode
    pub stx_mode: u16,
    /// spare
    pub __spare0: [u16; 1],
    /// inode number
    pub stx_ino: u64,
    /// file size
    pub stx_size: u64,
    /// number of blocks
    pub stx_blocks: u64,
    /// attributes mask
    pub stx_attributes_mask: u64,
    /// access time
    pub stx_atime: StatxTimestamp,
    /// birth time
    pub stx_btime: StatxTimestamp,
    /// change time
    pub stx_ctime: StatxTimestamp,
    /// modification time
    pub stx_mtime: StatxTimestamp,
    /// rdev major
    pub stx_rdev_major: u32,
    /// rdev minor
    pub stx_rdev_minor: u32,
    /// dev major
    pub stx_dev_major: u32,
    /// dev minor
    pub stx_dev_minor: u32,
    /// spare2
    pub __spare2: [u64; 14],
}

impl From<Metadata> for Statx {
    fn from(info: Metadata) -> Self {
        let dev_major = ((info.dev >> 8) & 0xfff) as u32;
        let dev_minor = (info.dev & 0xff) as u32;
        let rdev_major = ((info.rdev >> 8) & 0xfff) as u32;
        let rdev_minor = (info.rdev & 0xff) as u32;

        Statx {
            stx_mask: STATX_BASIC_STATS,
            stx_blksize: info.blk_size as u32,
            stx_attributes: 0,
            stx_nlink: info.nlinks as u32,
            stx_uid: info.uid as u32,
            stx_gid: info.gid as u32,
            stx_mode: StatMode::from_type_mode(info.type_, info.mode as _).bits() as u16,
            __spare0: [0; 1],
            stx_ino: info.inode as u64,
            stx_size: info.size as u64,
            stx_blocks: info.blocks as u64,
            stx_attributes_mask: 0,
            stx_atime: StatxTimestamp {
                tv_sec: info.atime.sec,
                tv_nsec: info.atime.nsec as u32,
                __reserved: 0,
            },
            stx_btime: StatxTimestamp {
                tv_sec: info.ctime.sec,
                tv_nsec: info.ctime.nsec as u32,
                __reserved: 0,
            },
            stx_ctime: StatxTimestamp {
                tv_sec: info.ctime.sec,
                tv_nsec: info.ctime.nsec as u32,
                __reserved: 0,
            },
            stx_mtime: StatxTimestamp {
                tv_sec: info.mtime.sec,
                tv_nsec: info.mtime.nsec as u32,
                __reserved: 0,
            },
            stx_rdev_major: rdev_major,
            stx_rdev_minor: rdev_minor,
            stx_dev_major: dev_major,
            stx_dev_minor: dev_minor,
            __spare2: [0; 14],
        }
    }
}

#[cfg(all(test, target_arch = "x86_64"))]
mod abi_tests {
    use super::*;
    use core::mem::{align_of, size_of};

    #[test]
    fn stat_layout_is_stable() {
        // Lock layout: userspace musl binaries depend on this size/alignment.
        assert_eq!(size_of::<Stat>(), 120);
        assert_eq!(align_of::<Stat>(), 8);
    }
}
