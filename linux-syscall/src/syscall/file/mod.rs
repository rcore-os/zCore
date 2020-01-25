use super::*;
use crate::fs::*;
use crate::process::ProcessExt as _;
use alloc::string::String;
use bitflags::bitflags;
use core::convert::TryFrom;
use rcore_fs::vfs::{FileType, FsError, INode};

mod dir;
//mod fd;
mod file;
mod poll;
mod stat;

/// Split a `path` str to `(base_path, file_name)`
fn split_path(path: &str) -> (&str, &str) {
    let mut split = path.trim_end_matches('/').rsplitn(2, '/');
    let file_name = split.next().unwrap();
    let mut dir_path = split.next().unwrap_or(".");
    if dir_path == "" {
        dir_path = "/";
    }
    (dir_path, file_name)
}

const FOLLOW_MAX_DEPTH: usize = 1;

//    pub fn sys_getcwd(&self, buf: *mut u8, len: usize) -> SysResult {
//        let proc = self.process();
//        if !proc.pid.is_init() {
//            // we trust pid 0 process
//            info!("getcwd: buf={:?}, len={:#x}", buf, len);
//        }
//        let buf = unsafe { self.vm().check_write_array(buf, len)? };
//        if proc.cwd.len() + 1 > len {
//            return Err(SysError::ERANGE);
//        }
//        unsafe { util::write_cstr(buf.as_mut_ptr(), &proc.cwd) }
//        Ok(buf.as_ptr() as usize)
//    }
//
//    pub fn sys_chdir(&self, path: *const u8) -> SysResult {
//        let proc = self.process();
//        let path = check_and_clone_cstr(path)?;
//        if !proc.pid.is_init() {
//            // we trust pid 0 process
//            info!("chdir: path={:?}", path);
//        }
//
//        let inode = proc.lookup_inode(&path)?;
//        let info = inode.metadata()?;
//        if info.type_ != FileType::Dir {
//            return Err(SysError::ENOTDIR);
//        }
//
//        // BUGFIX: '..' and '.'
//        if path.len() > 0 {
//            let cwd = match path.as_bytes()[0] {
//                b'/' => String::from("/"),
//                _ => proc.cwd.clone(),
//            };
//            let mut cwd_vec: Vec<_> = cwd.split("/").filter(|&x| x != "").collect();
//            let path_split = path.split("/").filter(|&x| x != "");
//            for seg in path_split {
//                if seg == ".." {
//                    cwd_vec.pop();
//                } else if seg == "." {
//                    // nothing to do here.
//                } else {
//                    cwd_vec.push(seg);
//                }
//            }
//            proc.cwd = String::from("");
//            for seg in cwd_vec {
//                proc.cwd.push_str("/");
//                proc.cwd.push_str(seg);
//            }
//            if proc.cwd == "" {
//                proc.cwd = String::from("/");
//            }
//        }
//        Ok(0)
//    }
