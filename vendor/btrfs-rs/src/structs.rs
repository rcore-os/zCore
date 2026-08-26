//! btrfs on-disk format: constants, keys, and (de)serialization of items.
//!
//! All multi-byte integers on disk are little-endian. Instead of `repr(C,
//! packed)` structs we use explicit offset-based accessors over byte slices,
//! which avoids alignment pitfalls in no_std kernels.

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

pub const SUPERBLOCK_MAGIC: u64 = 0x4D5F53665248425F; // "_BHRfS_M" LE
pub const SUPERBLOCK_OFFSETS: [u64; 3] = [0x1_0000, 0x400_0000, 0x40_0000_0000];
pub const SUPERBLOCK_SIZE: usize = 4096;
pub const CSUM_SIZE: usize = 32;
pub const HEADER_SIZE: usize = 101;
pub const ITEM_SIZE: usize = 25; // disk_key(17) + offset(4) + size(4)
pub const KEY_PTR_SIZE: usize = 33; // disk_key(17) + blockptr(8) + generation(8)
pub const KEY_SIZE: usize = 17;

// Tree object ids.
pub const ROOT_TREE: u64 = 1;
pub const EXTENT_TREE: u64 = 2;
pub const CHUNK_TREE: u64 = 3;
pub const DEV_TREE: u64 = 4;
pub const FS_TREE: u64 = 5;
pub const ROOT_TREE_DIR: u64 = 6;
pub const CSUM_TREE: u64 = 7;
pub const UUID_TREE: u64 = 9;
pub const DATA_RELOC_TREE: u64 = 0xFFFF_FFFF_FFFF_FFF7; // -9
pub const DEV_ITEMS_OBJECTID: u64 = 1;
pub const EXTENT_CSUM_OBJECTID: u64 = 0xFFFF_FFFF_FFFF_FFF6; // -10
pub const FIRST_CHUNK_TREE_OBJECTID: u64 = 256;
pub const FIRST_FREE_OBJECTID: u64 = 256;
pub const LAST_FREE_OBJECTID: u64 = 0xFFFF_FFFF_FFFF_FEFF; // -256

// Item types.
pub const INODE_ITEM_KEY: u8 = 1;
pub const INODE_REF_KEY: u8 = 12;
pub const XATTR_ITEM_KEY: u8 = 24;
pub const DIR_ITEM_KEY: u8 = 84;
pub const DIR_INDEX_KEY: u8 = 96;
pub const EXTENT_DATA_KEY: u8 = 108;
pub const EXTENT_CSUM_KEY: u8 = 128;
pub const ROOT_ITEM_KEY: u8 = 132;
pub const ROOT_BACKREF_KEY: u8 = 144;
pub const ROOT_REF_KEY: u8 = 156;
pub const EXTENT_ITEM_KEY: u8 = 168;
pub const METADATA_ITEM_KEY: u8 = 169;
pub const TREE_BLOCK_REF_KEY: u8 = 176;
pub const EXTENT_DATA_REF_KEY: u8 = 178;
pub const SHARED_BLOCK_REF_KEY: u8 = 182;
pub const SHARED_DATA_REF_KEY: u8 = 184;
pub const BLOCK_GROUP_ITEM_KEY: u8 = 192;
pub const DEV_EXTENT_KEY: u8 = 204;
pub const DEV_ITEM_KEY: u8 = 216;
pub const CHUNK_ITEM_KEY: u8 = 228;

// Block group / chunk type flags.
pub const BLOCK_GROUP_DATA: u64 = 1 << 0;
pub const BLOCK_GROUP_SYSTEM: u64 = 1 << 1;
pub const BLOCK_GROUP_METADATA: u64 = 1 << 2;
pub const BLOCK_GROUP_RAID_MASK: u64 =
    !(BLOCK_GROUP_DATA | BLOCK_GROUP_SYSTEM | BLOCK_GROUP_METADATA);
pub const BLOCK_GROUP_DUP: u64 = 1 << 5;

// Extent item flags.
pub const EXTENT_FLAG_DATA: u64 = 1 << 0;
pub const EXTENT_FLAG_TREE_BLOCK: u64 = 1 << 1;

// Header flags.
pub const HEADER_FLAG_WRITTEN: u64 = 1 << 0;
pub const BACKREF_REV_MIXED: u64 = 1 << 56; // top byte = 1

// Superblock incompat flags.
pub const INCOMPAT_MIXED_BACKREF: u64 = 1 << 0;
pub const INCOMPAT_DEFAULT_SUBVOL: u64 = 1 << 1;
pub const INCOMPAT_MIXED_GROUPS: u64 = 1 << 2;
pub const INCOMPAT_COMPRESS_LZO: u64 = 1 << 3;
pub const INCOMPAT_COMPRESS_ZSTD: u64 = 1 << 4;
pub const INCOMPAT_BIG_METADATA: u64 = 1 << 5;
pub const INCOMPAT_EXTENDED_IREF: u64 = 1 << 6;
pub const INCOMPAT_RAID56: u64 = 1 << 7;
pub const INCOMPAT_SKINNY_METADATA: u64 = 1 << 8;
pub const INCOMPAT_NO_HOLES: u64 = 1 << 9;
/// Incompat flags this driver understands; anything else fails the mount.
pub const INCOMPAT_SUPPORTED: u64 = INCOMPAT_MIXED_BACKREF
    | INCOMPAT_DEFAULT_SUBVOL
    | INCOMPAT_MIXED_GROUPS
    | INCOMPAT_BIG_METADATA
    | INCOMPAT_EXTENDED_IREF
    | INCOMPAT_SKINNY_METADATA
    | INCOMPAT_NO_HOLES;

// Compat-RO flags (must not write if unknown ones are set).
pub const COMPAT_RO_FREE_SPACE_TREE: u64 = 1 << 0;
pub const COMPAT_RO_FREE_SPACE_TREE_VALID: u64 = 1 << 1;

// Inode flags.
pub const INODE_NODATASUM: u64 = 1 << 0;
pub const INODE_NODATACOW: u64 = 1 << 1;

// inode mode bits (subset, standard POSIX values).
pub const S_IFMT: u32 = 0o170000;
pub const S_IFSOCK: u32 = 0o140000;
pub const S_IFLNK: u32 = 0o120000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFBLK: u32 = 0o060000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_IFIFO: u32 = 0o010000;

// File extent types.
pub const FILE_EXTENT_INLINE: u8 = 0;
pub const FILE_EXTENT_REG: u8 = 1;
pub const FILE_EXTENT_PREALLOC: u8 = 2;

pub const INODE_ITEM_LEN: usize = 160;
pub const ROOT_ITEM_LEN: usize = 439;
pub const BLOCK_GROUP_ITEM_LEN: usize = 24;
pub const DEV_EXTENT_LEN: usize = 48;
pub const DEV_ITEM_LEN: usize = 98;
pub const CHUNK_ITEM_HDR_LEN: usize = 48;
pub const STRIPE_LEN: usize = 32;
pub const FILE_EXTENT_HDR_LEN: usize = 21;
pub const FILE_EXTENT_REG_LEN: usize = 53;
pub const DIR_ITEM_HDR_LEN: usize = 30;
pub const EXTENT_ITEM_LEN: usize = 24; // refs + generation + flags

pub const MAX_NAME_LEN: usize = 255;

/// Directory entry type byte (as stored in dir items, == DT_* >> ...).
pub const FT_UNKNOWN: u8 = 0;
pub const FT_REG_FILE: u8 = 1;
pub const FT_DIR: u8 = 2;
pub const FT_CHRDEV: u8 = 3;
pub const FT_BLKDEV: u8 = 4;
pub const FT_FIFO: u8 = 5;
pub const FT_SOCK: u8 = 6;
pub const FT_SYMLINK: u8 = 7;
pub const FT_XATTR: u8 = 8;

/// High-level file kind used by the public API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Regular,
    Dir,
    Symlink,
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
}

impl FileKind {
    pub fn from_mode(mode: u32) -> Self {
        match mode & S_IFMT {
            S_IFDIR => FileKind::Dir,
            S_IFLNK => FileKind::Symlink,
            S_IFCHR => FileKind::CharDevice,
            S_IFBLK => FileKind::BlockDevice,
            S_IFIFO => FileKind::Fifo,
            S_IFSOCK => FileKind::Socket,
            _ => FileKind::Regular,
        }
    }

    pub fn dir_type(self) -> u8 {
        match self {
            FileKind::Regular => FT_REG_FILE,
            FileKind::Dir => FT_DIR,
            FileKind::Symlink => FT_SYMLINK,
            FileKind::CharDevice => FT_CHRDEV,
            FileKind::BlockDevice => FT_BLKDEV,
            FileKind::Fifo => FT_FIFO,
            FileKind::Socket => FT_SOCK,
        }
    }

    pub fn mode_bits(self) -> u32 {
        match self {
            FileKind::Regular => S_IFREG,
            FileKind::Dir => S_IFDIR,
            FileKind::Symlink => S_IFLNK,
            FileKind::CharDevice => S_IFCHR,
            FileKind::BlockDevice => S_IFBLK,
            FileKind::Fifo => S_IFIFO,
            FileKind::Socket => S_IFSOCK,
        }
    }
}

// ---------------------------------------------------------------------------
// Little-endian helpers
// ---------------------------------------------------------------------------

#[inline]
pub fn get_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}
#[inline]
pub fn get_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
#[inline]
pub fn get_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        b[off],
        b[off + 1],
        b[off + 2],
        b[off + 3],
        b[off + 4],
        b[off + 5],
        b[off + 6],
        b[off + 7],
    ])
}
#[inline]
pub fn put_u16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
#[inline]
pub fn put_u32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
#[inline]
pub fn put_u64(b: &mut [u8], off: usize, v: u64) {
    b[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

/// A btrfs key: ordering is (objectid, type, offset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub objectid: u64,
    pub item_type: u8,
    pub offset: u64,
}

impl Key {
    pub const MIN: Key = Key::new(0, 0, 0);
    pub const MAX: Key = Key::new(u64::MAX, u8::MAX, u64::MAX);

    pub const fn new(objectid: u64, item_type: u8, offset: u64) -> Self {
        Self {
            objectid,
            item_type,
            offset,
        }
    }

    pub fn read(b: &[u8], off: usize) -> Self {
        Self {
            objectid: get_u64(b, off),
            item_type: b[off + 8],
            offset: get_u64(b, off + 9),
        }
    }

    pub fn write(&self, b: &mut [u8], off: usize) {
        put_u64(b, off, self.objectid);
        b[off + 8] = self.item_type;
        put_u64(b, off + 9, self.offset);
    }

    /// The smallest key strictly greater than `self`.
    pub fn successor(&self) -> Key {
        if self.offset != u64::MAX {
            Key::new(self.objectid, self.item_type, self.offset + 1)
        } else if self.item_type != u8::MAX {
            Key::new(self.objectid, self.item_type + 1, 0)
        } else {
            Key::new(self.objectid + 1, 0, 0)
        }
    }
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        self.objectid
            .cmp(&other.objectid)
            .then(self.item_type.cmp(&other.item_type))
            .then(self.offset.cmp(&other.offset))
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// ---------------------------------------------------------------------------
// Tree block header accessors (operate on a whole tree-block buffer)
// ---------------------------------------------------------------------------

pub mod header {
    use super::*;

    pub const OFF_CSUM: usize = 0;
    pub const OFF_FSID: usize = 32;
    pub const OFF_BYTENR: usize = 48;
    pub const OFF_FLAGS: usize = 56;
    pub const OFF_CHUNK_TREE_UUID: usize = 64;
    pub const OFF_GENERATION: usize = 80;
    pub const OFF_OWNER: usize = 88;
    pub const OFF_NRITEMS: usize = 96;
    pub const OFF_LEVEL: usize = 100;

    pub fn bytenr(b: &[u8]) -> u64 {
        get_u64(b, OFF_BYTENR)
    }
    pub fn set_bytenr(b: &mut [u8], v: u64) {
        put_u64(b, OFF_BYTENR, v)
    }
    pub fn generation(b: &[u8]) -> u64 {
        get_u64(b, OFF_GENERATION)
    }
    pub fn set_generation(b: &mut [u8], v: u64) {
        put_u64(b, OFF_GENERATION, v)
    }
    pub fn owner(b: &[u8]) -> u64 {
        get_u64(b, OFF_OWNER)
    }
    pub fn set_owner(b: &mut [u8], v: u64) {
        put_u64(b, OFF_OWNER, v)
    }
    pub fn nritems(b: &[u8]) -> u32 {
        get_u32(b, OFF_NRITEMS)
    }
    pub fn set_nritems(b: &mut [u8], v: u32) {
        put_u32(b, OFF_NRITEMS, v)
    }
    pub fn level(b: &[u8]) -> u8 {
        b[OFF_LEVEL]
    }
    pub fn set_level(b: &mut [u8], v: u8) {
        b[OFF_LEVEL] = v
    }
    pub fn set_flags(b: &mut [u8], v: u64) {
        put_u64(b, OFF_FLAGS, v)
    }

    /// Recompute the tree-block checksum over `[CSUM_SIZE..]`.
    pub fn update_csum(b: &mut [u8]) {
        let sum = crate::crc::checksum(&b[CSUM_SIZE..]);
        b[..CSUM_SIZE].fill(0);
        put_u32(b, 0, sum);
    }

    pub fn csum_ok(b: &[u8]) -> bool {
        get_u32(b, 0) == crate::crc::checksum(&b[CSUM_SIZE..])
    }
}

// ---------------------------------------------------------------------------
// Leaf / node accessors
// ---------------------------------------------------------------------------

pub mod leaf {
    use super::*;

    /// Key of item `slot`.
    pub fn key(b: &[u8], slot: usize) -> Key {
        Key::read(b, HEADER_SIZE + slot * ITEM_SIZE)
    }
    pub fn set_key(b: &mut [u8], slot: usize, key: &Key) {
        key.write(b, HEADER_SIZE + slot * ITEM_SIZE);
    }
    /// Data offset (relative to end of header) of item `slot`.
    pub fn data_off(b: &[u8], slot: usize) -> usize {
        get_u32(b, HEADER_SIZE + slot * ITEM_SIZE + KEY_SIZE) as usize
    }
    pub fn set_data_off(b: &mut [u8], slot: usize, v: usize) {
        put_u32(b, HEADER_SIZE + slot * ITEM_SIZE + KEY_SIZE, v as u32)
    }
    pub fn data_size(b: &[u8], slot: usize) -> usize {
        get_u32(b, HEADER_SIZE + slot * ITEM_SIZE + KEY_SIZE + 4) as usize
    }
    pub fn set_data_size(b: &mut [u8], slot: usize, v: usize) {
        put_u32(b, HEADER_SIZE + slot * ITEM_SIZE + KEY_SIZE + 4, v as u32)
    }
    /// Item data of `slot` as a slice.
    pub fn data(b: &[u8], slot: usize) -> &[u8] {
        let off = HEADER_SIZE + data_off(b, slot);
        &b[off..off + data_size(b, slot)]
    }
    pub fn data_mut(b: &mut [u8], slot: usize) -> &mut [u8] {
        let off = HEADER_SIZE + data_off(b, slot);
        let size = data_size(b, slot);
        &mut b[off..off + size]
    }

    /// Lowest data offset used by any item (== free-space end). For an empty
    /// leaf this is the data-area size.
    pub fn data_start(b: &[u8], nodesize: usize) -> usize {
        let n = header::nritems(b) as usize;
        let mut min = nodesize - HEADER_SIZE;
        for slot in 0..n {
            min = min.min(data_off(b, slot));
        }
        min
    }

    /// Free bytes between the item table and the data area.
    pub fn free_space(b: &[u8], nodesize: usize) -> usize {
        let n = header::nritems(b) as usize;
        data_start(b, nodesize).saturating_sub(n * ITEM_SIZE)
    }
}

pub mod node {
    use super::*;

    pub fn key(b: &[u8], slot: usize) -> Key {
        Key::read(b, HEADER_SIZE + slot * KEY_PTR_SIZE)
    }
    pub fn set_key(b: &mut [u8], slot: usize, key: &Key) {
        key.write(b, HEADER_SIZE + slot * KEY_PTR_SIZE);
    }
    pub fn blockptr(b: &[u8], slot: usize) -> u64 {
        get_u64(b, HEADER_SIZE + slot * KEY_PTR_SIZE + KEY_SIZE)
    }
    pub fn set_blockptr(b: &mut [u8], slot: usize, v: u64) {
        put_u64(b, HEADER_SIZE + slot * KEY_PTR_SIZE + KEY_SIZE, v)
    }
    pub fn generation(b: &[u8], slot: usize) -> u64 {
        get_u64(b, HEADER_SIZE + slot * KEY_PTR_SIZE + KEY_SIZE + 8)
    }
    pub fn set_generation(b: &mut [u8], slot: usize, v: u64) {
        put_u64(b, HEADER_SIZE + slot * KEY_PTR_SIZE + KEY_SIZE + 8, v)
    }

    pub fn max_items(nodesize: usize) -> usize {
        (nodesize - HEADER_SIZE) / KEY_PTR_SIZE
    }
}

// ---------------------------------------------------------------------------
// Inode item
// ---------------------------------------------------------------------------

/// btrfs_inode_item (160 bytes).
#[derive(Debug, Clone, Default)]
pub struct InodeItem {
    pub generation: u64,
    pub transid: u64,
    pub size: u64,
    pub nbytes: u64,
    pub block_group: u64,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
    pub rdev: u64,
    pub flags: u64,
    pub sequence: u64,
    pub atime: (u64, u32),
    pub ctime: (u64, u32),
    pub mtime: (u64, u32),
    pub otime: (u64, u32),
}

fn get_time(b: &[u8], off: usize) -> (u64, u32) {
    (get_u64(b, off), get_u32(b, off + 8))
}
fn put_time(b: &mut [u8], off: usize, t: (u64, u32)) {
    put_u64(b, off, t.0);
    put_u32(b, off + 8, t.1);
}

impl InodeItem {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < INODE_ITEM_LEN {
            return None;
        }
        Some(Self {
            generation: get_u64(b, 0),
            transid: get_u64(b, 8),
            size: get_u64(b, 16),
            nbytes: get_u64(b, 24),
            block_group: get_u64(b, 32),
            nlink: get_u32(b, 40),
            uid: get_u32(b, 44),
            gid: get_u32(b, 48),
            mode: get_u32(b, 52),
            rdev: get_u64(b, 56),
            flags: get_u64(b, 64),
            sequence: get_u64(b, 72),
            // 4 reserved u64 at 80..112
            atime: get_time(b, 112),
            ctime: get_time(b, 124),
            mtime: get_time(b, 136),
            otime: get_time(b, 148),
        })
    }

    pub fn encode(&self) -> [u8; INODE_ITEM_LEN] {
        let mut b = [0u8; INODE_ITEM_LEN];
        put_u64(&mut b, 0, self.generation);
        put_u64(&mut b, 8, self.transid);
        put_u64(&mut b, 16, self.size);
        put_u64(&mut b, 24, self.nbytes);
        put_u64(&mut b, 32, self.block_group);
        put_u32(&mut b, 40, self.nlink);
        put_u32(&mut b, 44, self.uid);
        put_u32(&mut b, 48, self.gid);
        put_u32(&mut b, 52, self.mode);
        put_u64(&mut b, 56, self.rdev);
        put_u64(&mut b, 64, self.flags);
        put_u64(&mut b, 72, self.sequence);
        put_time(&mut b, 112, self.atime);
        put_time(&mut b, 124, self.ctime);
        put_time(&mut b, 136, self.mtime);
        put_time(&mut b, 148, self.otime);
        b
    }

    pub fn kind(&self) -> FileKind {
        FileKind::from_mode(self.mode)
    }
}

// ---------------------------------------------------------------------------
// Root item
// ---------------------------------------------------------------------------

/// btrfs_root_item (439 bytes); only the fields we care about.
#[derive(Debug, Clone)]
pub struct RootItem {
    pub inode: InodeItem,
    pub generation: u64,
    pub root_dirid: u64,
    pub bytenr: u64,
    pub bytes_used: u64,
    pub refs: u32,
    pub level: u8,
    pub uuid: [u8; 16],
}

impl RootItem {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < ROOT_ITEM_LEN {
            return None;
        }
        Some(Self {
            inode: InodeItem::parse(&b[..INODE_ITEM_LEN])?,
            generation: get_u64(b, 160),
            root_dirid: get_u64(b, 168),
            bytenr: get_u64(b, 176),
            bytes_used: get_u64(b, 192),
            refs: get_u32(b, 216),
            level: b[238],
            uuid: b[247..263].try_into().ok()?,
        })
    }

    pub fn encode(&self) -> [u8; ROOT_ITEM_LEN] {
        let mut b = [0u8; ROOT_ITEM_LEN];
        b[..INODE_ITEM_LEN].copy_from_slice(&self.inode.encode());
        put_u64(&mut b, 160, self.generation); // generation
        put_u64(&mut b, 168, self.root_dirid); // root_dirid
        put_u64(&mut b, 176, self.bytenr); // bytenr
                                           // byte_limit (184) = 0
        put_u64(&mut b, 192, self.bytes_used); // bytes_used
                                               // last_snapshot (200) = 0, flags (208) = 0
        put_u32(&mut b, 216, self.refs); // refs
                                         // drop_progress key (220..237) zero, drop_level (237) zero
        b[238] = self.level;
        // generation_v2 must match generation for modern kernels.
        put_u64(&mut b, 239, self.generation);
        b[247..263].copy_from_slice(&self.uuid);
        b
    }
}

// ---------------------------------------------------------------------------
// File extents
// ---------------------------------------------------------------------------

/// Parsed btrfs_file_extent_item.
#[derive(Debug, Clone)]
pub enum FileExtent {
    Inline {
        /// Uncompressed length (== inline data length, we don't support
        /// compression).
        ram_bytes: u64,
        /// Offset of the inline data inside the item.
        data_off: usize,
    },
    Regular {
        disk_bytenr: u64,
        disk_num_bytes: u64,
        /// Offset into the (decompressed) extent where this mapping starts.
        offset: u64,
        num_bytes: u64,
    },
}

impl FileExtent {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < FILE_EXTENT_HDR_LEN {
            return None;
        }
        let compression = b[16];
        let ty = b[20];
        if compression != 0 || b[17] != 0 {
            return None; // compressed/encrypted not supported
        }
        match ty {
            FILE_EXTENT_INLINE => Some(FileExtent::Inline {
                ram_bytes: get_u64(b, 8),
                data_off: FILE_EXTENT_HDR_LEN,
            }),
            FILE_EXTENT_REG | FILE_EXTENT_PREALLOC => {
                if b.len() < FILE_EXTENT_REG_LEN {
                    return None;
                }
                Some(FileExtent::Regular {
                    disk_bytenr: get_u64(b, 21),
                    disk_num_bytes: get_u64(b, 29),
                    offset: get_u64(b, 37),
                    num_bytes: get_u64(b, 45),
                })
            }
            _ => None,
        }
    }

    pub fn encode_regular(
        generation: u64,
        disk_bytenr: u64,
        disk_num_bytes: u64,
        offset: u64,
        num_bytes: u64,
    ) -> [u8; FILE_EXTENT_REG_LEN] {
        let mut b = [0u8; FILE_EXTENT_REG_LEN];
        put_u64(&mut b, 0, generation);
        put_u64(&mut b, 8, num_bytes); // ram_bytes
        b[20] = FILE_EXTENT_REG;
        put_u64(&mut b, 21, disk_bytenr);
        put_u64(&mut b, 29, disk_num_bytes);
        put_u64(&mut b, 37, offset);
        put_u64(&mut b, 45, num_bytes);
        b
    }

    pub fn encode_inline(generation: u64, data: &[u8]) -> Vec<u8> {
        let mut b = alloc::vec![0u8; FILE_EXTENT_HDR_LEN + data.len()];
        put_u64(&mut b, 0, generation);
        put_u64(&mut b, 8, data.len() as u64); // ram_bytes
        b[20] = FILE_EXTENT_INLINE;
        b[FILE_EXTENT_HDR_LEN..].copy_from_slice(data);
        b
    }
}

// ---------------------------------------------------------------------------
// Directory items
// ---------------------------------------------------------------------------

/// One entry inside a DIR_ITEM / DIR_INDEX item.
#[derive(Debug, Clone)]
pub struct DirEntryRaw {
    pub location: Key,
    pub transid: u64,
    pub dir_type: u8,
    pub name: Vec<u8>,
    pub data: Vec<u8>,
}

impl DirEntryRaw {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = alloc::vec![0u8; DIR_ITEM_HDR_LEN + self.name.len() + self.data.len()];
        self.location.write(&mut b, 0);
        put_u64(&mut b, 17, self.transid);
        put_u16(&mut b, 25, self.data.len() as u16);
        put_u16(&mut b, 27, self.name.len() as u16);
        b[29] = self.dir_type;
        b[DIR_ITEM_HDR_LEN..DIR_ITEM_HDR_LEN + self.name.len()].copy_from_slice(&self.name);
        b[DIR_ITEM_HDR_LEN + self.name.len()..].copy_from_slice(&self.data);
        b
    }
}

/// Iterate the (possibly multiple, on hash collision) entries packed in one
/// DIR_ITEM/DIR_INDEX item. Returns (byte_range_in_item, entry).
pub fn parse_dir_entries(b: &[u8]) -> Vec<(core::ops::Range<usize>, DirEntryRaw)> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + DIR_ITEM_HDR_LEN <= b.len() {
        let location = Key::read(b, off);
        let transid = get_u64(b, off + 17);
        let data_len = get_u16(b, off + 25) as usize;
        let name_len = get_u16(b, off + 27) as usize;
        let dir_type = b[off + 29];
        let end = off + DIR_ITEM_HDR_LEN + name_len + data_len;
        if name_len == 0 || end > b.len() {
            break;
        }
        let name = b[off + DIR_ITEM_HDR_LEN..off + DIR_ITEM_HDR_LEN + name_len].to_vec();
        let data = b[off + DIR_ITEM_HDR_LEN + name_len..end].to_vec();
        out.push((
            off..end,
            DirEntryRaw {
                location,
                transid,
                dir_type,
                name,
                data,
            },
        ));
        off = end;
    }
    out
}

/// Parse an INODE_REF item: sequence of (index u64, name_len u16, name).
pub fn parse_inode_refs(b: &[u8]) -> Vec<(core::ops::Range<usize>, u64, Vec<u8>)> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 10 <= b.len() {
        let index = get_u64(b, off);
        let name_len = get_u16(b, off + 8) as usize;
        let end = off + 10 + name_len;
        if end > b.len() {
            break;
        }
        out.push((off..end, index, b[off + 10..end].to_vec()));
        off = end;
    }
    out
}

pub fn encode_inode_ref(index: u64, name: &[u8]) -> Vec<u8> {
    let mut b = alloc::vec![0u8; 10 + name.len()];
    put_u64(&mut b, 0, index);
    put_u16(&mut b, 8, name.len() as u16);
    b[10..].copy_from_slice(name);
    b
}

// ---------------------------------------------------------------------------
// Chunks / dev items / block groups
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct Stripe {
    pub devid: u64,
    pub offset: u64,
}

#[derive(Debug, Clone)]
pub struct ChunkItem {
    pub length: u64,
    pub owner: u64,
    pub stripe_len: u64,
    pub type_: u64,
    pub io_align: u32,
    pub io_width: u32,
    pub sector_size: u32,
    pub sub_stripes: u16,
    pub stripes: Vec<Stripe>,
}

impl ChunkItem {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < CHUNK_ITEM_HDR_LEN {
            return None;
        }
        let num_stripes = get_u16(b, 44) as usize;
        if num_stripes == 0 || b.len() < CHUNK_ITEM_HDR_LEN + num_stripes * STRIPE_LEN {
            return None;
        }
        let mut stripes = Vec::with_capacity(num_stripes);
        for i in 0..num_stripes {
            let off = CHUNK_ITEM_HDR_LEN + i * STRIPE_LEN;
            stripes.push(Stripe {
                devid: get_u64(b, off),
                offset: get_u64(b, off + 8),
            });
        }
        Some(Self {
            length: get_u64(b, 0),
            owner: get_u64(b, 8),
            stripe_len: get_u64(b, 16),
            type_: get_u64(b, 24),
            io_align: get_u32(b, 32),
            io_width: get_u32(b, 36),
            sector_size: get_u32(b, 40),
            sub_stripes: get_u16(b, 46),
            stripes,
        })
    }

    pub fn encode(&self, dev_uuid: &[u8; 16]) -> Vec<u8> {
        let mut b = alloc::vec![0u8; CHUNK_ITEM_HDR_LEN + self.stripes.len() * STRIPE_LEN];
        put_u64(&mut b, 0, self.length);
        put_u64(&mut b, 8, self.owner);
        put_u64(&mut b, 16, self.stripe_len);
        put_u64(&mut b, 24, self.type_);
        put_u32(&mut b, 32, self.io_align);
        put_u32(&mut b, 36, self.io_width);
        put_u32(&mut b, 40, self.sector_size);
        put_u16(&mut b, 44, self.stripes.len() as u16);
        put_u16(&mut b, 46, self.sub_stripes);
        for (i, s) in self.stripes.iter().enumerate() {
            let off = CHUNK_ITEM_HDR_LEN + i * STRIPE_LEN;
            put_u64(&mut b, off, s.devid);
            put_u64(&mut b, off + 8, s.offset);
            b[off + 16..off + 32].copy_from_slice(dev_uuid);
        }
        b
    }
}

/// btrfs_dev_item (98 bytes).
#[derive(Debug, Clone)]
pub struct DevItem {
    pub devid: u64,
    pub total_bytes: u64,
    pub bytes_used: u64,
    pub uuid: [u8; 16],
    pub fsid: [u8; 16],
}

impl DevItem {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < DEV_ITEM_LEN {
            return None;
        }
        Some(Self {
            devid: get_u64(b, 0),
            total_bytes: get_u64(b, 8),
            bytes_used: get_u64(b, 16),
            uuid: b[66..82].try_into().ok()?,
            fsid: b[82..98].try_into().ok()?,
        })
    }

    pub fn encode(&self, sector_size: u32) -> [u8; DEV_ITEM_LEN] {
        let mut b = [0u8; DEV_ITEM_LEN];
        put_u64(&mut b, 0, self.devid);
        put_u64(&mut b, 8, self.total_bytes);
        put_u64(&mut b, 16, self.bytes_used);
        put_u32(&mut b, 24, sector_size); // io_align
        put_u32(&mut b, 28, sector_size); // io_width
        put_u32(&mut b, 32, sector_size); // sector_size
                                          // type(36)=0, generation(44)=0, start_offset(52)=0, dev_group(60)=0
                                          // seek_speed(64)=0, bandwidth(65)=0
        b[66..82].copy_from_slice(&self.uuid);
        b[82..98].copy_from_slice(&self.fsid);
        b
    }
}

/// btrfs_dev_extent (48 bytes). Key: (devid, DEV_EXTENT, physical offset).
#[derive(Debug, Clone, Copy)]
pub struct DevExtent {
    pub chunk_offset: u64,
    pub length: u64,
}

impl DevExtent {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < DEV_EXTENT_LEN {
            return None;
        }
        Some(Self {
            chunk_offset: get_u64(b, 16),
            length: get_u64(b, 24),
        })
    }

    pub fn encode(&self, chunk_tree_uuid: &[u8; 16]) -> [u8; DEV_EXTENT_LEN] {
        let mut b = [0u8; DEV_EXTENT_LEN];
        put_u64(&mut b, 0, CHUNK_TREE);
        put_u64(&mut b, 8, FIRST_CHUNK_TREE_OBJECTID);
        put_u64(&mut b, 16, self.chunk_offset);
        put_u64(&mut b, 24, self.length);
        b[32..48].copy_from_slice(chunk_tree_uuid);
        b
    }
}

/// btrfs_block_group_item (24 bytes). Key: (start, BLOCK_GROUP_ITEM, length).
#[derive(Debug, Clone, Copy)]
pub struct BlockGroupItem {
    pub used: u64,
    pub flags: u64,
}

impl BlockGroupItem {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < BLOCK_GROUP_ITEM_LEN {
            return None;
        }
        Some(Self {
            used: get_u64(b, 0),
            flags: get_u64(b, 16),
        })
    }

    pub fn encode(&self) -> [u8; BLOCK_GROUP_ITEM_LEN] {
        let mut b = [0u8; BLOCK_GROUP_ITEM_LEN];
        put_u64(&mut b, 0, self.used);
        put_u64(&mut b, 8, FIRST_CHUNK_TREE_OBJECTID); // chunk_objectid
        put_u64(&mut b, 16, self.flags);
        b
    }
}

// ---------------------------------------------------------------------------
// Superblock
// ---------------------------------------------------------------------------

/// Field offsets within the 4 KiB superblock.
pub mod sb {
    pub const OFF_CSUM: usize = 0x00;
    pub const OFF_FSID: usize = 0x20;
    pub const OFF_BYTENR: usize = 0x30;
    pub const OFF_FLAGS: usize = 0x38;
    pub const OFF_MAGIC: usize = 0x40;
    pub const OFF_GENERATION: usize = 0x48;
    pub const OFF_ROOT: usize = 0x50;
    pub const OFF_CHUNK_ROOT: usize = 0x58;
    pub const OFF_LOG_ROOT: usize = 0x60;
    pub const OFF_TOTAL_BYTES: usize = 0x70;
    pub const OFF_BYTES_USED: usize = 0x78;
    pub const OFF_ROOT_DIR_OBJECTID: usize = 0x80;
    pub const OFF_NUM_DEVICES: usize = 0x88;
    pub const OFF_SECTORSIZE: usize = 0x90;
    pub const OFF_NODESIZE: usize = 0x94;
    pub const OFF_LEAFSIZE: usize = 0x98;
    pub const OFF_STRIPESIZE: usize = 0x9c;
    pub const OFF_SYS_CHUNK_ARRAY_SIZE: usize = 0xa0;
    pub const OFF_CHUNK_ROOT_GENERATION: usize = 0xa4;
    pub const OFF_COMPAT_FLAGS: usize = 0xac;
    pub const OFF_COMPAT_RO_FLAGS: usize = 0xb4;
    pub const OFF_INCOMPAT_FLAGS: usize = 0xbc;
    pub const OFF_CSUM_TYPE: usize = 0xc4;
    pub const OFF_ROOT_LEVEL: usize = 0xc6;
    pub const OFF_CHUNK_ROOT_LEVEL: usize = 0xc7;
    pub const OFF_LOG_ROOT_LEVEL: usize = 0xc8;
    pub const OFF_DEV_ITEM: usize = 0xc9;
    pub const OFF_LABEL: usize = 0x12b;
    pub const OFF_CACHE_GENERATION: usize = 0x22b;
    pub const OFF_UUID_TREE_GENERATION: usize = 0x233;
    pub const OFF_METADATA_UUID: usize = 0x23b;
    pub const OFF_SYS_CHUNK_ARRAY: usize = 0x32b;
    pub const SYS_CHUNK_ARRAY_LEN: usize = 2048;
}

/// In-memory copy of the superblock: the raw 4 KiB plus typed accessors.
#[derive(Clone)]
pub struct Superblock {
    pub raw: Vec<u8>,
}

impl Superblock {
    pub fn parse(raw: Vec<u8>) -> Option<Self> {
        if raw.len() != SUPERBLOCK_SIZE || get_u64(&raw, sb::OFF_MAGIC) != SUPERBLOCK_MAGIC {
            return None;
        }
        Some(Self { raw })
    }

    pub fn fsid(&self) -> [u8; 16] {
        self.raw[sb::OFF_FSID..sb::OFF_FSID + 16]
            .try_into()
            .unwrap()
    }
    pub fn generation(&self) -> u64 {
        get_u64(&self.raw, sb::OFF_GENERATION)
    }
    pub fn root(&self) -> u64 {
        get_u64(&self.raw, sb::OFF_ROOT)
    }
    pub fn set_root(&mut self, v: u64) {
        put_u64(&mut self.raw, sb::OFF_ROOT, v)
    }
    pub fn chunk_root(&self) -> u64 {
        get_u64(&self.raw, sb::OFF_CHUNK_ROOT)
    }
    pub fn set_chunk_root(&mut self, v: u64) {
        put_u64(&mut self.raw, sb::OFF_CHUNK_ROOT, v)
    }
    pub fn total_bytes(&self) -> u64 {
        get_u64(&self.raw, sb::OFF_TOTAL_BYTES)
    }
    pub fn set_total_bytes(&mut self, v: u64) {
        put_u64(&mut self.raw, sb::OFF_TOTAL_BYTES, v)
    }
    pub fn bytes_used(&self) -> u64 {
        get_u64(&self.raw, sb::OFF_BYTES_USED)
    }
    pub fn set_bytes_used(&mut self, v: u64) {
        put_u64(&mut self.raw, sb::OFF_BYTES_USED, v)
    }
    pub fn sectorsize(&self) -> u32 {
        get_u32(&self.raw, sb::OFF_SECTORSIZE)
    }
    pub fn nodesize(&self) -> u32 {
        get_u32(&self.raw, sb::OFF_NODESIZE)
    }
    pub fn compat_ro_flags(&self) -> u64 {
        get_u64(&self.raw, sb::OFF_COMPAT_RO_FLAGS)
    }
    pub fn incompat_flags(&self) -> u64 {
        get_u64(&self.raw, sb::OFF_INCOMPAT_FLAGS)
    }
    pub fn csum_type(&self) -> u16 {
        get_u16(&self.raw, sb::OFF_CSUM_TYPE)
    }
    pub fn root_level(&self) -> u8 {
        self.raw[sb::OFF_ROOT_LEVEL]
    }
    pub fn set_root_level(&mut self, v: u8) {
        self.raw[sb::OFF_ROOT_LEVEL] = v
    }
    pub fn chunk_root_level(&self) -> u8 {
        self.raw[sb::OFF_CHUNK_ROOT_LEVEL]
    }
    pub fn set_chunk_root_level(&mut self, v: u8) {
        self.raw[sb::OFF_CHUNK_ROOT_LEVEL] = v
    }
    pub fn log_root(&self) -> u64 {
        get_u64(&self.raw, sb::OFF_LOG_ROOT)
    }
    pub fn set_log_root(&mut self, v: u64) {
        put_u64(&mut self.raw, sb::OFF_LOG_ROOT, v)
    }
    pub fn num_devices(&self) -> u64 {
        get_u64(&self.raw, sb::OFF_NUM_DEVICES)
    }
    pub fn dev_item(&self) -> Option<DevItem> {
        DevItem::parse(&self.raw[sb::OFF_DEV_ITEM..sb::OFF_DEV_ITEM + DEV_ITEM_LEN])
    }
    pub fn set_dev_item_total_bytes(&mut self, v: u64) {
        put_u64(&mut self.raw, sb::OFF_DEV_ITEM + 8, v)
    }
    pub fn set_dev_item_bytes_used(&mut self, v: u64) {
        put_u64(&mut self.raw, sb::OFF_DEV_ITEM + 16, v)
    }
    pub fn chunk_tree_uuid(&self) -> [u8; 16] {
        // stored inside dev_item? No: chunk tree uuid lives in tree-block
        // headers; the superblock does not carry it. We use the fsid-derived
        // uuid kept by the volume instead.
        self.fsid()
    }
    pub fn label(&self) -> String {
        let raw = &self.raw[sb::OFF_LABEL..sb::OFF_LABEL + 256];
        let end = raw.iter().position(|&c| c == 0).unwrap_or(256);
        String::from_utf8_lossy(&raw[..end]).into_owned()
    }
    pub fn sys_chunk_array(&self) -> &[u8] {
        let size = get_u32(&self.raw, sb::OFF_SYS_CHUNK_ARRAY_SIZE) as usize;
        let size = size.min(sb::SYS_CHUNK_ARRAY_LEN);
        &self.raw[sb::OFF_SYS_CHUNK_ARRAY..sb::OFF_SYS_CHUNK_ARRAY + size]
    }

    /// Recompute the superblock checksum.
    pub fn update_csum(&mut self) {
        let sum = crate::crc::checksum(&self.raw[CSUM_SIZE..]);
        self.raw[..CSUM_SIZE].fill(0);
        put_u32(&mut self.raw, 0, sum);
    }
}
