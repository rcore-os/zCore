//! Syscalls for files
#![deny(missing_docs)]
use super::*;
use bitflags::bitflags;
use linux_object::fs::vfs::{FileType, FsError};
use linux_object::fs::*;

mod dir;
mod fd;
#[allow(clippy::module_inception)]
mod file;
mod mount;
mod pidfd;
mod poll;
mod splice;
mod stat;

use self::dir::AtFlags;
