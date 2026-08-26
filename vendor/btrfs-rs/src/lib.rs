//! btrfs filesystem driver (no_std + alloc).
//!
//! Provides:
//! - read/write access to btrfs volumes through a [`device::BlockDevice`]
//!   (in-place editing, no copy-on-write; generations are left untouched so
//!   parent pointers stay consistent),
//! - an image builder ([`mkfs`]) producing filesystems mountable by Linux,
//! - auto-grow of the filesystem to the underlying device size.
//!
//! Data checksums are avoided by creating every file with the
//! `NODATASUM|NODATACOW` inode flags (also set on first write to foreign
//! files), so only tree-block checksums need to be maintained.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(warnings)]

extern crate alloc;

#[macro_use]
extern crate log;

pub mod alloc_ext;
pub mod crc;
pub mod device;
pub mod fs;
pub mod mkfs;
pub mod structs;
pub mod tree;
pub mod volume;

pub use device::BlockDevice;
pub use fs::{Btrfs, DirEntry, FsStat, InodeStat};
pub use structs::FileKind;

/// Crate-wide error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Device I/O failed.
    Io,
    /// Not a btrfs filesystem / bad superblock.
    BadSuperblock,
    /// On-disk structure is corrupt or unsupported.
    Corrupt(&'static str),
    /// Unsupported feature (RAID profile, csum type, ...).
    Unsupported(&'static str),
    /// Entry not found.
    NotFound,
    /// Entry already exists.
    Exists,
    /// Not a directory.
    NotDir,
    /// Is a directory.
    IsDir,
    /// Directory not empty.
    NotEmpty,
    /// No space left on device.
    NoSpace,
    /// Invalid argument (name too long, bad offset, ...).
    Invalid,
}

pub type Result<T> = core::result::Result<T, Error>;
