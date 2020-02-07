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
use alloc::vec::Vec;
use bitflags::bitflags;
use kernel_hal::user::UserOutPtr;
use linux_object::fs::vfs::FileType;

impl Syscall<'_> {
    pub fn sys_getcwd(&self, mut buf: UserOutPtr<u8>, len: usize) -> SysResult {
        info!("getcwd: buf={:?}, len={:#x}", buf, len);
        let proc = self.lock_linux_process();
        if proc.cwd.len() + 1 > len {
            return Err(LxError::ERANGE);
        }
        buf.write_cstring(&proc.cwd)?;
        Ok(buf.as_ptr() as usize)
    }

    pub fn sys_chdir(&self, path: UserInPtr<u8>) -> SysResult {
        let path = path.read_cstring()?;
        info!("chdir: path={:?}", path);

        let mut proc = self.lock_linux_process();
        let inode = proc.lookup_inode(&path)?;
        let info = inode.metadata()?;
        if info.type_ != FileType::Dir {
            return Err(LxError::ENOTDIR);
        }

        // BUGFIX: '..' and '.'
        if !path.is_empty() {
            let cwd = match path.as_bytes()[0] {
                b'/' => String::from("/"),
                _ => proc.cwd.clone(),
            };
            let mut cwd_vec: Vec<_> = cwd.split('/').filter(|&x| x != "").collect();
            let path_split = path.split('/').filter(|&x| x != "");
            for seg in path_split {
                if seg == ".." {
                    cwd_vec.pop();
                } else if seg == "." {
                    // nothing to do here.
                } else {
                    cwd_vec.push(seg);
                }
            }
            proc.cwd = String::from("");
            for seg in cwd_vec {
                proc.cwd.push_str("/");
                proc.cwd.push_str(seg);
            }
            if proc.cwd == "" {
                proc.cwd = String::from("/");
            }
        }
        Ok(0)
    }

    pub fn sys_mkdir(&self, path: UserInPtr<u8>, mode: usize) -> SysResult {
        self.sys_mkdirat(FileDesc::CWD, path, mode)
    }

    pub fn sys_mkdirat(&self, dirfd: FileDesc, path: UserInPtr<u8>, mode: usize) -> SysResult {
        let path = path.read_cstring()?;
        // TODO: check pathname
        info!(
            "mkdirat: dirfd={:?}, path={:?}, mode={:#o}",
            dirfd, path, mode
        );

        let (dir_path, file_name) = split_path(&path);
        let proc = self.lock_linux_process();
        let inode = proc.lookup_inode_at(dirfd, dir_path, true)?;
        if inode.find(file_name).is_ok() {
            return Err(LxError::EEXIST);
        }
        inode.create(file_name, FileType::Dir, mode as u32)?;
        Ok(0)
    }

    pub fn sys_rmdir(&self, path: UserInPtr<u8>) -> SysResult {
        let path = path.read_cstring()?;
        info!("rmdir: path={:?}", path);

        let (dir_path, file_name) = split_path(&path);
        let proc = self.lock_linux_process();
        let dir_inode = proc.lookup_inode(dir_path)?;
        let file_inode = dir_inode.find(file_name)?;
        if file_inode.metadata()?.type_ != FileType::Dir {
            return Err(LxError::ENOTDIR);
        }
        dir_inode.unlink(file_name)?;
        Ok(0)
    }

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
        let proc = self.lock_linux_process();
        let file = proc.get_file(fd)?;
        let info = file.metadata()?;
        if info.type_ != FileType::Dir {
            return Err(LxError::ENOTDIR);
        }
        let mut kbuf = vec![0; buf_size];
        let mut writer = DirentBufWriter::new(&mut kbuf);
        loop {
            let name = match file.read_entry() {
                Err(LxError::ENOENT) => break,
                r => r,
            }?;
            // TODO: get ino from dirent
            let ok = writer.try_write(0, DirentType::from(info.type_).bits(), &name);
            if !ok {
                break;
            }
        }
        buf.write_array(writer.as_slice())?;
        Ok(writer.written_size)
    }

    pub fn sys_link(&self, oldpath: UserInPtr<u8>, newpath: UserInPtr<u8>) -> SysResult {
        self.sys_linkat(FileDesc::CWD, oldpath, FileDesc::CWD, newpath, 0)
    }

    pub fn sys_linkat(
        &self,
        olddirfd: FileDesc,
        oldpath: UserInPtr<u8>,
        newdirfd: FileDesc,
        newpath: UserInPtr<u8>,
        flags: usize,
    ) -> SysResult {
        let oldpath = oldpath.read_cstring()?;
        let newpath = newpath.read_cstring()?;
        let flags = AtFlags::from_bits_truncate(flags);
        info!(
            "linkat: olddirfd={:?}, oldpath={:?}, newdirfd={:?}, newpath={:?}, flags={:?}",
            olddirfd, oldpath, newdirfd, newpath, flags
        );

        let proc = self.lock_linux_process();
        let (new_dir_path, new_file_name) = split_path(&newpath);
        let inode = proc.lookup_inode_at(olddirfd, &oldpath, true)?;
        let new_dir_inode = proc.lookup_inode_at(newdirfd, new_dir_path, true)?;
        new_dir_inode.link(new_file_name, &inode)?;
        Ok(0)
    }

    pub fn sys_unlink(&self, path: UserInPtr<u8>) -> SysResult {
        self.sys_unlinkat(FileDesc::CWD, path, 0)
    }

    pub fn sys_unlinkat(&self, dirfd: FileDesc, path: UserInPtr<u8>, flags: usize) -> SysResult {
        let path = path.read_cstring()?;
        let flags = AtFlags::from_bits_truncate(flags);
        info!(
            "unlinkat: dirfd={:?}, path={:?}, flags={:?}",
            dirfd, path, flags
        );

        let proc = self.lock_linux_process();
        let (dir_path, file_name) = split_path(&path);
        let dir_inode = proc.lookup_inode_at(dirfd, dir_path, true)?;
        let file_inode = dir_inode.find(file_name)?;
        if file_inode.metadata()?.type_ == FileType::Dir {
            return Err(LxError::EISDIR);
        }
        dir_inode.unlink(file_name)?;
        Ok(0)
    }

    pub fn sys_rename(&self, oldpath: UserInPtr<u8>, newpath: UserInPtr<u8>) -> SysResult {
        self.sys_renameat(FileDesc::CWD, oldpath, FileDesc::CWD, newpath)
    }

    pub fn sys_renameat(
        &self,
        olddirfd: FileDesc,
        oldpath: UserInPtr<u8>,
        newdirfd: FileDesc,
        newpath: UserInPtr<u8>,
    ) -> SysResult {
        let oldpath = oldpath.read_cstring()?;
        let newpath = newpath.read_cstring()?;
        info!(
            "renameat: olddirfd={:?}, oldpath={:?}, newdirfd={:?}, newpath={:?}",
            olddirfd, oldpath, newdirfd, newpath
        );

        let proc = self.lock_linux_process();
        let (old_dir_path, old_file_name) = split_path(&oldpath);
        let (new_dir_path, new_file_name) = split_path(&newpath);
        let old_dir_inode = proc.lookup_inode_at(olddirfd, old_dir_path, false)?;
        let new_dir_inode = proc.lookup_inode_at(newdirfd, new_dir_path, false)?;
        old_dir_inode.move_(old_file_name, &new_dir_inode, new_file_name)?;
        Ok(0)
    }

    pub fn sys_readlink(&self, path: UserInPtr<u8>, base: UserOutPtr<u8>, len: usize) -> SysResult {
        self.sys_readlinkat(FileDesc::CWD, path, base, len)
    }

    pub fn sys_readlinkat(
        &self,
        dirfd: FileDesc,
        path: UserInPtr<u8>,
        mut base: UserOutPtr<u8>,
        len: usize,
    ) -> SysResult {
        let path = path.read_cstring()?;
        info!(
            "readlinkat: dirfd={:?}, path={:?}, base={:?}, len={}",
            dirfd, path, base, len
        );

        let proc = self.lock_linux_process();
        let inode = proc.lookup_inode_at(dirfd, &path, false)?;
        if inode.metadata()?.type_ != FileType::SymLink {
            return Err(LxError::EINVAL);
        }
        // TODO: recursive link resolution and loop detection
        let mut buf = vec![0; len];
        let len = inode.read_at(0, &mut buf)?;
        base.write_array(&buf[..len])?;
        Ok(len)
    }
}

#[allow(dead_code)]
#[repr(packed)] // Don't use 'C'. Or its size will align up to 8 bytes.
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

struct DirentBufWriter<'a> {
    buf: &'a mut [u8],
    rest_size: usize,
    written_size: usize,
}

impl<'a> DirentBufWriter<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        DirentBufWriter {
            rest_size: buf.len(),
            written_size: 0,
            buf,
        }
    }

    fn try_write(&mut self, inode: u64, type_: u8, name: &str) -> bool {
        let len = core::mem::size_of::<LinuxDirent64>() + name.len() + 1;
        let len = (len + 7) / 8 * 8; // align up
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
    }
}
