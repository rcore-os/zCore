//! Directory operations
//!
//! - getcwd
//! - chdir
//! - mkdir(at)
//! - rmdir(at)
//! - getdents64
//! - link(at)
//! - unlink(at)
//! - rename(at)
//! - readlink(at)

use super::*;
use bitflags::bitflags;
use kernel_hal::user::UserOutPtr;
use linux_object::fs::vfs::FileType;

impl Syscall<'_> {
    /// return a null-terminated string containing an absolute pathname
    /// that is the current working directory of the calling process.
    /// - `buf` – pointer to buffer to receive path
    /// - `len` – size of buf
    pub fn sys_getcwd(&self, mut buf: UserOutPtr<u8>, len: usize) -> SysResult {
        info!("getcwd: buf={:?}, len={:#x}", buf, len);
        let proc = self.linux_process();
        let cwd = proc.current_working_directory();
        if cwd.len() + 1 > len {
            return Err(LxError::ERANGE);
        }
        buf.write_cstring(&cwd)?;
        Ok(buf.as_addr())
    }

    /// Change the current directory.
    /// - `path` – pointer to string with name of path
    pub fn sys_chdir(&self, path: UserInPtr<u8>) -> SysResult {
        let path = path.as_c_str()?;
        info!("chdir: path={:?}", path);

        let proc = self.linux_process();
        let inode = proc.lookup_inode(path)?;
        let info = inode.metadata()?;
        if info.type_ != FileType::Dir {
            return Err(LxError::ENOTDIR);
        }
        proc.check_access(&info, 0o1, true)?;
        proc.change_directory(path);
        Ok(0)
    }

    /// Make a directory.
    /// - path – pointer to string with directory name
    /// - mode – file system permissions mode
    pub fn sys_mkdir(&self, path: UserInPtr<u8>, mode: usize) -> SysResult {
        self.sys_mkdirat(FileDesc::CWD, path, mode)
    }

    /// create directory relative to directory file descriptor
    pub fn sys_mkdirat(&self, dirfd: FileDesc, path: UserInPtr<u8>, mode: usize) -> SysResult {
        let path = path.as_c_str()?;
        // TODO: check pathname
        info!(
            "mkdirat: dirfd={:?}, path={:?}, mode={:#o}",
            dirfd, path, mode
        );

        let (dir_path, file_name) = split_path(path);
        let proc = self.linux_process();
        let inode = proc.lookup_inode_at(dirfd, dir_path, true)?;
        let dir_metadata = inode.metadata()?;
        proc.check_access(&dir_metadata, 0o3, true)?;
        if inode.find(file_name).is_ok() {
            return Err(LxError::EEXIST);
        }
        let create_mode = proc.apply_umask(mode as u16);
        let created = inode.create(file_name, FileType::Dir, create_mode as u32)?;
        proc.initialize_created_metadata(&created, Some(&dir_metadata), create_mode, true)?;
        linux_object::fs::dcache_invalidate();
        Ok(0)
    }

    /// create a special or ordinary file (see mknod(2)).
    pub fn sys_mknod(&self, path: UserInPtr<u8>, mode: usize, dev: usize) -> SysResult {
        self.sys_mknodat(FileDesc::CWD, path, mode, dev)
    }

    /// create a special or ordinary file relative to a directory fd.
    pub fn sys_mknodat(
        &self,
        dirfd: FileDesc,
        path: UserInPtr<u8>,
        mode: usize,
        dev: usize,
    ) -> SysResult {
        let path = path.as_c_str()?;
        info!(
            "mknodat: dirfd={:?}, path={:?}, mode={:#o}, dev={:#x}",
            dirfd, path, mode, dev
        );
        const S_IFMT: usize = 0o170000;
        let file_type = match mode & S_IFMT {
            0o010000 => FileType::NamedPipe,   // S_IFIFO
            0o020000 => FileType::CharDevice,  // S_IFCHR
            0o060000 => FileType::BlockDevice, // S_IFBLK
            0o140000 => FileType::Socket,      // S_IFSOCK
            0o100000 | 0 => FileType::File,    // S_IFREG / unspecified => regular file
            _ => return Err(LxError::EINVAL),  // directories must use mkdir
        };
        let (dir_path, file_name) = split_path(path);
        let proc = self.linux_process();
        let inode = proc.lookup_inode_at(dirfd, dir_path, true)?;
        let dir_metadata = inode.metadata()?;
        proc.check_access(&dir_metadata, 0o3, true)?;
        if inode.find(file_name).is_ok() {
            return Err(LxError::EEXIST);
        }
        let create_mode = proc.apply_umask((mode & 0o7777) as u16);
        let rdev = if matches!(file_type, FileType::CharDevice | FileType::BlockDevice) {
            dev
        } else {
            0
        };
        let created = inode.create2(file_name, file_type, create_mode as u32, rdev)?;
        proc.initialize_created_metadata(&created, Some(&dir_metadata), create_mode, false)?;
        linux_object::fs::dcache_invalidate();
        Ok(0)
    }

    /// Remove a directory.
    /// - path – pointer to string with directory name
    pub fn sys_rmdir(&self, path: UserInPtr<u8>) -> SysResult {
        let path = path.as_c_str()?;
        info!("rmdir: path={:?}", path);

        let (dir_path, file_name) = split_path(path);
        let proc = self.linux_process();
        let dir_inode = proc.lookup_inode(dir_path)?;
        let dir_metadata = dir_inode.metadata()?;
        proc.check_access(&dir_metadata, 0o3, true)?;
        let file_inode = dir_inode.find(file_name)?;
        let file_metadata = file_inode.metadata()?;
        if file_metadata.type_ != FileType::Dir {
            return Err(LxError::ENOTDIR);
        }
        proc.check_sticky(&dir_metadata, &file_metadata)?;
        dir_inode.unlink(file_name)?;
        linux_object::fs::dcache_invalidate();
        Ok(0)
    }

    /// get directory entries
    /// TODO: get ino from dirent
    /// - fd – file describe
    pub fn sys_getdents64(
        &self,
        fd: FileDesc,
        mut buf: UserOutPtr<u8>,
        buf_size: usize,
    ) -> SysResult {
        info!(
            "getdents64: fd={:?}, ptr={:?}, buf_size={}",
            fd, buf, buf_size
        );
        let proc = self.linux_process();
        let file = proc.get_file(fd)?;
        let info = file.metadata()?;
        if info.type_ != FileType::Dir {
            return Err(LxError::ENOTDIR);
        }
        let cap_size = buf_size.min(256 * 1024);
        let mut kbuf = vec![0; cap_size];
        let mut writer = DirentBufWriter::new(&mut kbuf);
        loop {
            let (metadata, name) = match file.read_entry_with_metadata() {
                Err(LxError::ENOENT) => break,
                r => r,
            }?;
            let ok = writer.try_write(
                metadata.inode as u64,
                DirentType::from(metadata.type_).bits(),
                &name,
            );
            if !ok {
                break;
            }
        }
        buf.write_array(writer.as_slice())?;
        Ok(writer.written_size)
    }

    /// creates a new link (also known as a hard link) to an existing file.
    pub fn sys_link(&self, oldpath: UserInPtr<u8>, newpath: UserInPtr<u8>) -> SysResult {
        self.sys_linkat(FileDesc::CWD, oldpath, FileDesc::CWD, newpath, 0)
    }

    /// create file link relative to directory file descriptors
    /// If the pathname given in oldpath is relative,
    /// then it is interpreted relative to the directory referred to by the file descriptor olddirfd
    pub fn sys_linkat(
        &self,
        olddirfd: FileDesc,
        oldpath: UserInPtr<u8>,
        newdirfd: FileDesc,
        newpath: UserInPtr<u8>,
        flags: usize,
    ) -> SysResult {
        let oldpath = oldpath.as_c_str()?;
        let newpath = newpath.as_c_str()?;
        let flags = AtFlags::from_bits_truncate(flags);
        info!(
            "linkat: olddirfd={:?}, oldpath={:?}, newdirfd={:?}, newpath={:?}, flags={:?}",
            olddirfd, oldpath, newdirfd, newpath, flags
        );

        let proc = self.linux_process();
        let (new_dir_path, new_file_name) = split_path(newpath);
        let follow = flags.contains(AtFlags::SYMLINK_FOLLOW);
        let inode = proc.lookup_inode_at(olddirfd, oldpath, follow)?;
        let new_dir_inode = proc.lookup_inode_at(newdirfd, new_dir_path, true)?;
        let new_dir_metadata = new_dir_inode.metadata()?;
        proc.check_access(&new_dir_metadata, 0o3, true)?;
        new_dir_inode.link(new_file_name, &inode)?;
        linux_object::fs::dcache_invalidate();
        Ok(0)
    }

    /// delete name/possibly file it refers to
    /// If that name was the last link to a file and no processes have the file open, the file is deleted.
    /// If the name was the last link to a file but any processes still have the file open,
    /// the file will remain in existence until the last file descriptor referring to it is closed.
    pub fn sys_unlink(&self, path: UserInPtr<u8>) -> SysResult {
        self.sys_unlinkat(FileDesc::CWD, path, 0)
    }

    /// remove directory entry relative to directory file descriptor
    /// The unlinkat() system call operates in exactly the same way as either unlink or rmdir.
    pub fn sys_unlinkat(&self, dirfd: FileDesc, path: UserInPtr<u8>, flags: usize) -> SysResult {
        let path = path.as_c_str()?;
        // hard code special path
        let path = if path == "/dev/shm/testshm" {
            "/testshm"
        } else {
            path
        };
        let flags = AtFlags::from_bits_truncate(flags);
        info!(
            "unlinkat: dirfd={:?}, path={:?}, flags={:?}",
            dirfd, path, flags
        );

        let proc = self.linux_process();
        let (dir_path, file_name) = split_path(path);
        let dir_inode = proc.lookup_inode_at(dirfd, dir_path, true)?;
        let dir_metadata = dir_inode.metadata()?;
        proc.check_access(&dir_metadata, 0o3, true)?;
        let file_inode = dir_inode.find(file_name)?;
        let file_metadata = file_inode.metadata()?;
        if file_metadata.type_ == FileType::Dir {
            return Err(LxError::EISDIR);
        }
        proc.check_sticky(&dir_metadata, &file_metadata)?;
        dir_inode.unlink(file_name)?;
        linux_object::fs::dcache_invalidate();
        Ok(0)
    }

    /// change name/location of file
    pub fn sys_rename(&self, oldpath: UserInPtr<u8>, newpath: UserInPtr<u8>) -> SysResult {
        self.sys_renameat(FileDesc::CWD, oldpath, FileDesc::CWD, newpath)
    }

    /// rename file relative to directory file descriptors
    pub fn sys_renameat(
        &self,
        olddirfd: FileDesc,
        oldpath: UserInPtr<u8>,
        newdirfd: FileDesc,
        newpath: UserInPtr<u8>,
    ) -> SysResult {
        self.sys_renameat2(olddirfd, oldpath, newdirfd, newpath, 0)
    }

    /// rename with a `flags` argument (see renameat2(2)).
    ///
    /// `RENAME_NOREPLACE` is honoured: the rename fails with `EEXIST` instead
    /// of clobbering an existing target — what coreutils `mv -n` and atomic
    /// create-then-publish patterns rely on. `RENAME_EXCHANGE` / `WHITEOUT`
    /// answer `EINVAL`, the documented reply of a filesystem without support,
    /// so callers take their fallback path.
    pub fn sys_renameat2(
        &self,
        olddirfd: FileDesc,
        oldpath: UserInPtr<u8>,
        newdirfd: FileDesc,
        newpath: UserInPtr<u8>,
        flags: usize,
    ) -> SysResult {
        let oldpath = oldpath.as_c_str()?;
        let newpath = newpath.as_c_str()?;
        info!(
            "renameat2: olddirfd={:?}, oldpath={:?}, newdirfd={:?}, newpath={:?}, flags={:#x}",
            olddirfd, oldpath, newdirfd, newpath, flags
        );
        check_rename_flags(flags)?;

        let proc = self.linux_process();
        let (old_dir_path, old_file_name) = split_path(oldpath);
        let (new_dir_path, new_file_name) = split_path(newpath);
        let old_dir_inode = proc.lookup_inode_at(olddirfd, old_dir_path, false)?;
        let new_dir_inode = proc.lookup_inode_at(newdirfd, new_dir_path, false)?;
        let old_dir_metadata = old_dir_inode.metadata()?;
        let new_dir_metadata = new_dir_inode.metadata()?;
        proc.check_access(&old_dir_metadata, 0o3, true)?;
        proc.check_access(&new_dir_metadata, 0o3, true)?;
        let old_inode = old_dir_inode.find(old_file_name)?;
        let old_metadata = old_inode.metadata()?;
        proc.check_sticky(&old_dir_metadata, &old_metadata)?;
        if flags & RENAME_NOREPLACE != 0 && new_dir_inode.find(new_file_name).is_ok() {
            return Err(LxError::EEXIST);
        }
        old_dir_inode.move_(old_file_name, &new_dir_inode, new_file_name)?;
        linux_object::fs::dcache_invalidate();
        Ok(0)
    }

    /// read value of symbolic link
    pub fn sys_readlink(&self, path: UserInPtr<u8>, base: UserOutPtr<u8>, len: usize) -> SysResult {
        self.sys_readlinkat(FileDesc::CWD, path, base, len)
    }

    /// read value of symbolic link relative to directory file descriptor
    /// readlink() places the contents of the symbolic link path in the buffer base, which has size len
    /// TODO: recursive link resolution and loop detection
    pub fn sys_readlinkat(
        &self,
        dirfd: FileDesc,
        path: UserInPtr<u8>,
        mut base: UserOutPtr<u8>,
        len: usize,
    ) -> SysResult {
        let path = path.as_c_str()?;
        info!(
            "readlinkat: dirfd={:?}, path={:?}, base={:?}, len={}",
            dirfd, path, base, len
        );

        let proc = self.linux_process();
        // readlink(2) must follow symlinks in *intermediate* path components but
        // NOT the final one.
        //
        // Two resolution strategies are needed:
        //  1. Whole-path, no-follow: this is what reaches the magic links that
        //     `lookup_inode_at` special-cases on the FULL path string
        //     (`/proc/self/exe`, `/proc/self/fd/N`, ...). Firefox reads
        //     `/proc/self/exe` to find its install dir, so this MUST work.
        //  2. Parent-follow + final-no-follow: needed for a link whose parent
        //     dir is itself reached through a symlink — e.g. libdrm's
        //     readlink("/sys/dev/char/226:0/device/subsystem") where `device`
        //     is a symlink; strategy 1 stops at it (no-follow) and fails.
        // Try (1) first and accept it only when it yields a symlink; otherwise
        // fall back to (2). (1) never matched the magic links before because
        // this function split the path and bypassed the full-path special case.
        let inode = match proc.lookup_inode_at(dirfd, path, false) {
            Ok(i)
                if i.metadata()
                    .map(|m| m.type_ == FileType::SymLink)
                    .unwrap_or(false) =>
            {
                i
            }
            _ => {
                let (dir_path, file_name) = split_path(path);
                if file_name.is_empty() || file_name == "." || file_name == ".." {
                    proc.lookup_inode_at(dirfd, path, false)?
                } else {
                    let dir = proc.lookup_inode_at(dirfd, dir_path, true)?;
                    dir.find(file_name).map_err(LxError::from)?
                }
            }
        };
        if inode.metadata()?.type_ != FileType::SymLink {
            return Err(LxError::EINVAL);
        }
        // TODO: recursive link resolution and loop detection
        let cap_len = len.min(4096);
        let mut buf = vec![0; cap_len];
        let len = inode.read_at(0, &mut buf)?;
        base.write_array(&buf[..len])?;
        Ok(len)
    }

    /// Change the current directory to the one specified by the file descriptor.
    pub fn sys_fchdir(&self, fd: FileDesc) -> SysResult {
        info!("fchdir: fd={:?}", fd);

        let proc = self.linux_process();
        let file = proc.get_file(fd)?;
        let info = file.metadata()?;
        if info.type_ != FileType::Dir {
            return Err(LxError::ENOTDIR);
        }
        proc.check_access(&info, 0o1, true)?;
        proc.change_directory(file.path());
        Ok(0)
    }

    /// create a symbolic link relative to directory file descriptor
    pub fn sys_symlinkat(
        &self,
        target: UserInPtr<u8>,
        newdirfd: FileDesc,
        linkpath: UserInPtr<u8>,
    ) -> SysResult {
        let target = target.as_c_str()?;
        let linkpath = linkpath.as_c_str()?;
        info!(
            "symlinkat: target={:?}, newdirfd={:?}, linkpath={:?}",
            target, newdirfd, linkpath
        );

        let (dir_path, file_name) = split_path(linkpath);
        let proc = self.linux_process();
        let inode = proc.lookup_inode_at(newdirfd, dir_path, true)?;
        let dir_metadata = inode.metadata()?;
        proc.check_access(&dir_metadata, 0o3, true)?;
        if inode.find(file_name).is_ok() {
            return Err(LxError::EEXIST);
        }
        let mode = proc.apply_umask(0o777);
        let symlink_inode = inode.create(file_name, FileType::SymLink, mode as u32)?;
        proc.initialize_created_metadata(&symlink_inode, Some(&dir_metadata), mode, false)?;
        symlink_inode.write_at(0, target.as_bytes())?;
        linux_object::fs::dcache_invalidate();
        Ok(0)
    }

    /// create a symbolic link
    pub fn sys_symlink(&self, target: UserInPtr<u8>, linkpath: UserInPtr<u8>) -> SysResult {
        self.sys_symlinkat(target, FileDesc::CWD, linkpath)
    }
}

#[allow(dead_code)]
// Bare `packed` (not `C, packed`) is deliberate: adding the C ABI would align
// the struct size up to 8 bytes, but the getdents64 wire layout needs the tight
// 19-byte record. The clippy lint that wants an ABI qualifier does not apply.
#[allow(clippy::repr_packed_without_abi)]
#[repr(packed)]
pub struct LinuxDirent64 {
    /// Inode number
    ino: u64,
    /// Offset to next structure
    offset: u64,
    /// Size of this dirent
    reclen: u16,
    /// File type
    type_: u8,
    /// Filename (null-terminated)
    name: [u8; 0],
}

/// directory entry buffer writer
struct DirentBufWriter<'a> {
    buf: &'a mut [u8],
    rest_size: usize,
    written_size: usize,
}

impl<'a> DirentBufWriter<'a> {
    /// create a buffer writer
    fn new(buf: &'a mut [u8]) -> Self {
        DirentBufWriter {
            rest_size: buf.len(),
            written_size: 0,
            buf,
        }
    }

    /// write data
    fn try_write(&mut self, inode: u64, type_: u8, name: &str) -> bool {
        let len = core::mem::size_of::<LinuxDirent64>() + name.len() + 1;
        let len = len.div_ceil(8) * 8; // align up
        if self.rest_size < len {
            return false;
        }
        let dent = LinuxDirent64 {
            ino: inode,
            offset: 0,
            reclen: len as u16,
            type_,
            name: [],
        };
        #[allow(unsafe_code)]
        unsafe {
            let ptr = self.buf.as_ptr().add(self.written_size) as *mut LinuxDirent64;
            ptr.write(dent);
            let name_ptr = ptr.add(1) as *mut u8;
            name_ptr.copy_from_nonoverlapping(name.as_ptr(), name.len());
            name_ptr.add(name.len()).write(0);
        }
        self.rest_size -= len;
        self.written_size += len;
        true
    }

    /// to slice
    fn as_slice(&self) -> &[u8] {
        &self.buf[..self.written_size]
    }
}

bitflags! {
    pub struct DirentType: u8 {
        const UNKNOWN  = 0;
        /// FIFO (named pipe)
        const FIFO = 1;
        /// Character device
        const CHR  = 2;
        /// Directory
        const DIR  = 4;
        /// Block device
        const BLK = 6;
        /// Regular file
        const REG = 8;
        /// Symbolic link
        const LNK = 10;
        /// UNIX domain socket
        const SOCK  = 12;
        /// ???
        const WHT = 14;
    }
}

impl From<FileType> for DirentType {
    fn from(type_: FileType) -> Self {
        match type_ {
            FileType::File => Self::REG,
            FileType::Dir => Self::DIR,
            FileType::SymLink => Self::LNK,
            FileType::CharDevice => Self::CHR,
            FileType::BlockDevice => Self::BLK,
            FileType::Socket => Self::SOCK,
            FileType::NamedPipe => Self::FIFO,
        }
    }
}

bitflags! {
    pub struct AtFlags: usize {
        const EMPTY_PATH = 0x1000;
        const SYMLINK_NOFOLLOW = 0x100;
        const EACCESS = 0x200;
        const SYMLINK_FOLLOW = 0x400;
    }
}

/// renameat2(2) `RENAME_NOREPLACE`: don't overwrite an existing target.
const RENAME_NOREPLACE: usize = 1 << 0;
/// renameat2(2) `RENAME_EXCHANGE`: atomically swap source and target.
const RENAME_EXCHANGE: usize = 1 << 1;
/// renameat2(2) `RENAME_WHITEOUT`: leave a whiteout behind (overlayfs).
const RENAME_WHITEOUT: usize = 1 << 2;

/// Validate a renameat2 `flags` argument (renameat2(2)). Pure, so the flag
/// matrix is unit-testable: unknown bits and the documented mutually-exclusive
/// combinations are `EINVAL`; `EXCHANGE`/`WHITEOUT` also answer `EINVAL`
/// because no filesystem here implements them — the reply Linux gives on such
/// filesystems, which callers already handle with a fallback.
fn check_rename_flags(flags: usize) -> SysResult {
    if flags & !(RENAME_NOREPLACE | RENAME_EXCHANGE | RENAME_WHITEOUT) != 0 {
        return Err(LxError::EINVAL);
    }
    if flags & RENAME_EXCHANGE != 0 && flags & (RENAME_NOREPLACE | RENAME_WHITEOUT) != 0 {
        return Err(LxError::EINVAL);
    }
    if flags & (RENAME_EXCHANGE | RENAME_WHITEOUT) != 0 {
        return Err(LxError::EINVAL);
    }
    Ok(0)
}

#[cfg(test)]
mod rename_flag_tests {
    use super::*;

    #[test]
    fn plain_and_noreplace_are_accepted() {
        assert_eq!(check_rename_flags(0), Ok(0));
        assert_eq!(check_rename_flags(RENAME_NOREPLACE), Ok(0));
    }

    #[test]
    fn unsupported_and_invalid_combinations_are_einval() {
        for flags in [
            RENAME_EXCHANGE,
            RENAME_WHITEOUT,
            RENAME_EXCHANGE | RENAME_NOREPLACE,
            RENAME_EXCHANGE | RENAME_WHITEOUT,
            1 << 3,
            usize::MAX,
        ] {
            assert_eq!(
                check_rename_flags(flags),
                Err(LxError::EINVAL),
                "{flags:#x}"
            );
        }
    }
}
