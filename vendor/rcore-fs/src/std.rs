use crate::vfs::*;
#[cfg(windows)]
use filetime::FileTime;
use std::io::Error;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
#[cfg(windows)]
use winapi::shared::minwindef::DWORD;
#[cfg(windows)]
use winapi::um::winnt;

impl std::error::Error for FsError {}

impl From<std::io::Error> for FsError {
    fn from(e: Error) -> Self {
        use std::io::ErrorKind;
        match e.kind() {
            ErrorKind::NotFound => FsError::EntryNotFound,
            // We do not have permission in our fs, just ignore the file
            ErrorKind::PermissionDenied => FsError::EntryNotFound,
            ErrorKind::AlreadyExists => FsError::EntryExist,
            ErrorKind::WouldBlock => FsError::Again,
            ErrorKind::InvalidInput => FsError::InvalidParam,
            ErrorKind::InvalidData => FsError::InvalidParam,
            // The host fs is the device here
            _ => FsError::DeviceError,
        }
    }
}

#[cfg(unix)]
impl From<std::fs::Metadata> for Metadata {
    fn from(m: std::fs::Metadata) -> Self {
        Metadata {
            dev: m.dev() as usize,
            inode: m.ino() as usize,
            size: m.size() as usize,
            blk_size: m.blksize() as usize,
            blocks: m.blocks() as usize,
            atime: Timespec {
                sec: m.atime(),
                nsec: m.atime_nsec() as i32,
            },
            mtime: Timespec {
                sec: m.mtime(),
                nsec: m.mtime_nsec() as i32,
            },
            ctime: Timespec {
                sec: m.ctime(),
                nsec: m.ctime_nsec() as i32,
            },
            type_: match (m.mode() & 0xf000) as _ {
                libc::S_IFCHR => FileType::CharDevice,
                libc::S_IFBLK => FileType::BlockDevice,
                libc::S_IFDIR => FileType::Dir,
                libc::S_IFREG => FileType::File,
                libc::S_IFLNK => FileType::SymLink,
                libc::S_IFSOCK => FileType::Socket,
                _ => unimplemented!("unknown file type"),
            },
            mode: m.mode() as u16 & 0o777,
            nlinks: m.nlink() as usize,
            uid: m.uid() as usize,
            gid: m.gid() as usize,
            rdev: m.rdev() as usize,
        }
    }
}

#[cfg(windows)]
impl From<std::fs::Metadata> for Metadata {
    fn from(m: std::fs::Metadata) -> Self {
        Metadata {
            dev: 0,
            inode: 0,
            size: m.file_size() as usize,
            blk_size: 0,
            blocks: 0,
            atime: {
                let atime = FileTime::from_last_access_time(&m);
                Timespec {
                    sec: atime.unix_seconds(),
                    nsec: atime.nanoseconds() as i32,
                }
            },
            mtime: {
                let mtime = FileTime::from_last_modification_time(&m);
                Timespec {
                    sec: mtime.unix_seconds(),
                    nsec: mtime.nanoseconds() as i32,
                }
            },
            ctime: {
                let mtime = FileTime::from_last_modification_time(&m);
                Timespec {
                    sec: mtime.unix_seconds(),
                    nsec: mtime.nanoseconds() as i32,
                }
            },
            type_: {
                let attr = m.file_attributes() as DWORD;
                if (attr & winnt::FILE_ATTRIBUTE_NORMAL) != 0 {
                    FileType::File
                } else if (attr & winnt::FILE_ATTRIBUTE_DIRECTORY) != 0 {
                    FileType::Dir
                } else if (attr & winnt::FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
                    FileType::SymLink
                } else {
                    unimplemented!("unknown file type")
                }
            },
            mode: 0,
            nlinks: 0,
            uid: 0,
            gid: 0,
            rdev: 0,
        }
    }
}
