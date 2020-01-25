//! Directory operations
//!
//! - mkdir(at)
//! - rmdir(at)
//! - getdents64
//! - link(at)
//! - unlink(at)
//! - rename(at)
//! - readlink(at)

use crate::util::UserOutPtr;
use bitflags::bitflags;
use rcore_fs::vfs::FileType;

//    pub fn sys_mkdir(&self, path: UserInPtr<u8>, mode: usize) -> SysResult {
//        self.sys_mkdirat(FileDesc::CWD, path, mode)
//    }
//
//    pub fn sys_mkdirat(&self, dirfd: FileDesc, path: UserInPtr<u8>, mode: usize) -> SysResult {
//        let proc = self.process();
//        let path = check_and_clone_cstr(path)?;
//        // TODO: check pathname
//        info!(
//            "mkdirat: dirfd={}, path={:?}, mode={:#o}",
//            dirfd as isize, path, mode
//        );
//
//        let (dir_path, file_name) = split_path(&path);
//        let inode = proc.lookup_inode_at(dirfd, dir_path, true)?;
//        if inode.find(file_name).is_ok() {
//            return Err(SysError::EEXIST);
//        }
//        inode.create(file_name, FileType::Dir, mode as u32)?;
//        Ok(0)
//    }
//
//    pub fn sys_rmdir(&self, path: UserInPtr<u8>) -> SysResult {
//        let proc = self.process();
//        let path = check_and_clone_cstr(path)?;
//        info!("rmdir: path={:?}", path);
//
//        let (dir_path, file_name) = split_path(&path);
//        let dir_inode = proc.lookup_inode(dir_path)?;
//        let file_inode = dir_inode.find(file_name)?;
//        if file_inode.metadata()?.type_ != FileType::Dir {
//            return Err(SysError::ENOTDIR);
//        }
//        dir_inode.unlink(file_name)?;
//        Ok(0)
//    }

//    pub fn sys_getdents64(
//        &self,
//        fd: FileDesc,
//        buf: *mut LinuxDirent64,
//        buf_size: usize,
//    ) -> SysResult {
//        info!(
//            "getdents64: fd={}, ptr={:?}, buf_size={}",
//            fd, buf, buf_size
//        );
//        let proc = self.process();
//        let buf = unsafe { self.vm().check_write_array(buf as *mut u8, buf_size)? };
//        let file = proc.get_file(fd)?;
//        let info = file.metadata()?;
//        if info.type_ != FileType::Dir {
//            return Err(SysError::ENOTDIR);
//        }
//        let mut writer = DirentBufWriter::new(buf);
//        loop {
//            let name = match file.read_entry() {
//                Err(FsError::EntryNotFound) => break,
//                r => r,
//            }?;
//            // TODO: get ino from dirent
//            let ok = writer.try_write(0, DirentType::from_type(&info.type_).bits(), &name);
//            if !ok {
//                break;
//            }
//        }
//        Ok(writer.written_size)
//    }
//    pub fn sys_link(&self, oldpath: UserInPtr<u8>, newpath: UserInPtr<u8>) -> SysResult {
//        self.sys_linkat(FileDesc::CWD, oldpath, FileDesc::CWD, newpath, 0)
//    }
//
//    pub fn sys_linkat(
//        &self,
//        olddirfd: FileDesc,
//        oldpath: UserInPtr<u8>,
//        newdirfd: FileDesc,
//        newpath: UserInPtr<u8>,
//        flags: usize,
//    ) -> SysResult {
//        let proc = self.process();
//        let oldpath = check_and_clone_cstr(oldpath)?;
//        let newpath = check_and_clone_cstr(newpath)?;
//        let flags = AtFlags::from_bits_truncate(flags);
//        info!(
//            "linkat: olddirfd={}, oldpath={:?}, newdirfd={}, newpath={:?}, flags={:?}",
//            olddirfd as isize, oldpath, newdirfd as isize, newpath, flags
//        );
//
//        let (new_dir_path, new_file_name) = split_path(&newpath);
//        let inode = proc.lookup_inode_at(olddirfd, &oldpath, true)?;
//        let new_dir_inode = proc.lookup_inode_at(newdirfd, new_dir_path, true)?;
//        new_dir_inode.link(new_file_name, &inode)?;
//        Ok(0)
//    }
//
//    pub fn sys_unlink(&self, path: UserInPtr<u8>) -> SysResult {
//        self.sys_unlinkat(FileDesc::CWD, path, 0)
//    }
//
//    pub fn sys_unlinkat(&self, dirfd: FileDesc, path: UserInPtr<u8>, flags: usize) -> SysResult {
//        let proc = self.process();
//        let path = check_and_clone_cstr(path)?;
//        let flags = AtFlags::from_bits_truncate(flags);
//        info!(
//            "unlinkat: dirfd={}, path={:?}, flags={:?}",
//            dirfd as isize, path, flags
//        );
//
//        let (dir_path, file_name) = split_path(&path);
//        let dir_inode = proc.lookup_inode_at(dirfd, dir_path, true)?;
//        let file_inode = dir_inode.find(file_name)?;
//        if file_inode.metadata()?.type_ == FileType::Dir {
//            return Err(SysError::EISDIR);
//        }
//        dir_inode.unlink(file_name)?;
//        Ok(0)
//    }

//    pub fn sys_rename(&self, oldpath: UserInPtr<u8>, newpath: UserInPtr<u8>) -> SysResult {
//        self.sys_renameat(FileDesc::CWD, oldpath, FileDesc::CWD, newpath)
//    }
//
//    pub fn sys_renameat(
//        &self,
//        olddirfd: FileDesc,
//        oldpath: UserInPtr<u8>,
//        newdirfd: FileDesc,
//        newpath: UserInPtr<u8>,
//    ) -> SysResult {
//        let proc = self.process();
//        let oldpath = check_and_clone_cstr(oldpath)?;
//        let newpath = check_and_clone_cstr(newpath)?;
//        info!(
//            "renameat: olddirfd={}, oldpath={:?}, newdirfd={}, newpath={:?}",
//            olddirfd as isize, oldpath, newdirfd as isize, newpath
//        );
//
//        let (old_dir_path, old_file_name) = split_path(&oldpath);
//        let (new_dir_path, new_file_name) = split_path(&newpath);
//        let old_dir_inode = proc.lookup_inode_at(olddirfd, old_dir_path, false)?;
//        let new_dir_inode = proc.lookup_inode_at(newdirfd, new_dir_path, false)?;
//        old_dir_inode.move_(old_file_name, &new_dir_inode, new_file_name)?;
//        Ok(0)
//    }

//    pub fn sys_readlink(&self, path: UserInPtr<u8>, base: *mut u8, len: usize) -> SysResult {
//        self.sys_readlinkat(FileDesc::CWD, path, base, len)
//    }
//
//    pub fn sys_readlinkat(
//        &self,
//        dirfd: FileDesc,
//        path: UserInPtr<u8>,
//        base: *mut u8,
//        len: usize,
//    ) -> SysResult {
//        let proc = self.process();
//        let path = check_and_clone_cstr(path)?;
//        let slice = unsafe { self.vm().check_write_array(base, len)? };
//        info!(
//            "readlinkat: dirfd={}, path={:?}, base={:?}, len={}",
//            dirfd as isize, path, base, len
//        );
//
//        let inode = proc.lookup_inode_at(dirfd, &path, false)?;
//        if inode.metadata()?.type_ == FileType::SymLink {
//            // TODO: recursive link resolution and loop detection
//            let len = inode.read_at(0, slice)?;
//            Ok(len)
//        } else {
//            Err(SysError::EINVAL)
//        }
//    }

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
    ptr: *mut LinuxDirent64,
    rest_size: usize,
    written_size: usize,
}

impl<'a> DirentBufWriter<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        DirentBufWriter {
            ptr: buf.as_mut_ptr() as *mut LinuxDirent64,
            rest_size: buf.len(),
            written_size: 0,
            buf,
        }
    }
    fn try_write(&mut self, inode: u64, type_: u8, name: &str) -> bool {
        let len = ::core::mem::size_of::<LinuxDirent64>() + name.len() + 1;
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
            self.ptr.write(dent);
            let mut name_ptr = UserOutPtr::<u8>::from(self.ptr.add(1) as usize);
            name_ptr.write_cstring(name).unwrap();
            self.ptr = (self.ptr as *const u8).add(len) as _;
        }
        self.rest_size -= len;
        self.written_size += len;
        true
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

impl DirentType {
    fn from_type(type_: &FileType) -> Self {
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
