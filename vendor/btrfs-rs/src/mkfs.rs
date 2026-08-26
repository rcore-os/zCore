//! Filesystem image builder (mkfs).
//!
//! Produces a fresh btrfs filesystem mountable by Linux, mirroring the item
//! layout of `mkfs.btrfs -O ^free-space-tree` (no free-space cache/tree,
//! crc32c, SINGLE profiles, nodesize 16 KiB, sectorsize 4 KiB, incompat
//! flags MIXED_BACKREF|EXTENDED_IREF|SKINNY_METADATA|NO_HOLES).

use alloc::vec::Vec;

use crate::crc;
use crate::device::BlockDevice;
use crate::structs::*;
use crate::{Error, Result};

pub const SECTORSIZE: u32 = 4096;
pub const NODESIZE: u32 = 16384;

const GEN: u64 = 1;
const SYS_CHUNK_START: u64 = 0x10_0000; // 1 MiB
const SYS_CHUNK_LEN: u64 = 0x40_0000; // 4 MiB
const META_CHUNK_START: u64 = SYS_CHUNK_START + SYS_CHUNK_LEN;
const CHUNK_ALIGN: u64 = 0x10_0000;

/// Minimum device size we accept (1 MiB reserved + 4 MiB system + 8 MiB
/// metadata + 4 MiB data).
pub const MIN_DEVICE_SIZE: u64 = 17 * 1024 * 1024;

#[derive(Clone)]
pub struct MkfsOptions {
    pub label: alloc::string::String,
    pub fsid: [u8; 16],
    pub chunk_uuid: [u8; 16],
    pub dev_uuid: [u8; 16],
    pub subvol_uuid: [u8; 16],
    /// Wall-clock time for inode timestamps (secs, nanos).
    pub now: (u64, u32),
}

struct Layout {
    total_bytes: u64,
    meta_len: u64,
    /// (start, len) of every DATA chunk (logical == physical at mkfs time).
    data_chunks: Vec<(u64, u64)>,
    // Tree block addresses.
    chunk_leaf: u64,
    root_leaf: u64,
    extent_leaf: u64,
    dev_leaf: u64,
    fs_leaf: u64,
    csum_leaf: u64,
    uuid_leaf: u64,
    dreloc_leaf: u64,
}

fn plan(total_bytes: u64) -> Result<Layout> {
    if total_bytes < MIN_DEVICE_SIZE {
        return Err(Error::Invalid);
    }
    let total = total_bytes / CHUNK_ALIGN * CHUNK_ALIGN;
    let meta_len = (total / 8).clamp(8 * 1024 * 1024, 32 * 1024 * 1024) / CHUNK_ALIGN * CHUNK_ALIGN;
    let data_start = META_CHUNK_START + meta_len;
    // Data chunks fill the rest, leaving holes for superblock mirrors.
    let mut data_chunks = Vec::new();
    let mut pos = data_start;
    let mut breaks: Vec<u64> = SUPERBLOCK_OFFSETS
        .iter()
        .copied()
        .filter(|&off| off >= data_start && off < total)
        .collect();
    breaks.push(total);
    breaks.sort_unstable();
    for brk in breaks {
        let end = brk / CHUNK_ALIGN * CHUNK_ALIGN;
        if end > pos && end - pos >= 4 * 1024 * 1024 {
            data_chunks.push((pos, end - pos));
        }
        // Skip 1 MiB past the break (covers the 4 KiB superblock).
        pos = pos.max(brk / CHUNK_ALIGN * CHUNK_ALIGN + CHUNK_ALIGN);
    }
    let ns = NODESIZE as u64;
    Ok(Layout {
        total_bytes,
        meta_len,
        data_chunks,
        chunk_leaf: SYS_CHUNK_START,
        root_leaf: META_CHUNK_START,
        extent_leaf: META_CHUNK_START + ns,
        dev_leaf: META_CHUNK_START + 2 * ns,
        fs_leaf: META_CHUNK_START + 3 * ns,
        csum_leaf: META_CHUNK_START + 4 * ns,
        uuid_leaf: META_CHUNK_START + 5 * ns,
        dreloc_leaf: META_CHUNK_START + 6 * ns,
    })
}

fn build_leaf(
    opts: &MkfsOptions,
    owner: u64,
    bytenr: u64,
    items: &[(Key, Vec<u8>)],
) -> Result<Vec<u8>> {
    let nodesize = NODESIZE as usize;
    let mut b = alloc::vec![0u8; nodesize];
    b[header::OFF_FSID..header::OFF_FSID + 16].copy_from_slice(&opts.fsid);
    b[header::OFF_CHUNK_TREE_UUID..header::OFF_CHUNK_TREE_UUID + 16]
        .copy_from_slice(&opts.chunk_uuid);
    header::set_bytenr(&mut b, bytenr);
    header::set_flags(&mut b, HEADER_FLAG_WRITTEN | BACKREF_REV_MIXED);
    header::set_generation(&mut b, GEN);
    header::set_owner(&mut b, owner);
    header::set_level(&mut b, 0);
    header::set_nritems(&mut b, items.len() as u32);
    let mut data_off = nodesize - HEADER_SIZE;
    for (slot, (key, data)) in items.iter().enumerate() {
        data_off = data_off.checked_sub(data.len()).ok_or(Error::NoSpace)?;
        leaf::set_key(&mut b, slot, key);
        leaf::set_data_off(&mut b, slot, data_off);
        leaf::set_data_size(&mut b, slot, data.len());
        b[HEADER_SIZE + data_off..HEADER_SIZE + data_off + data.len()].copy_from_slice(data);
    }
    if items.len() * ITEM_SIZE > data_off {
        return Err(Error::NoSpace);
    }
    header::update_csum(&mut b);
    Ok(b)
}

fn root_item(bytenr: u64, root_dirid: u64, uuid: [u8; 16], now: (u64, u32)) -> Vec<u8> {
    let inode = InodeItem {
        generation: GEN,
        transid: 0,
        size: 0,
        nbytes: NODESIZE as u64,
        block_group: 0,
        nlink: 1,
        uid: 0,
        gid: 0,
        mode: S_IFDIR | 0o755,
        rdev: 0,
        flags: 0,
        sequence: 0,
        atime: now,
        ctime: now,
        mtime: now,
        otime: now,
    };
    let item = RootItem {
        inode,
        generation: GEN,
        root_dirid,
        bytenr,
        bytes_used: NODESIZE as u64,
        refs: 1,
        level: 0,
        uuid,
    };
    item.encode().to_vec()
}

fn dir_inode_item(now: (u64, u32), nbytes: u64) -> Vec<u8> {
    InodeItem {
        generation: GEN,
        transid: 0,
        size: 0,
        nbytes,
        block_group: 0,
        nlink: 1,
        uid: 0,
        gid: 0,
        mode: S_IFDIR | 0o755,
        rdev: 0,
        flags: 0,
        sequence: 0,
        atime: now,
        ctime: now,
        mtime: now,
        otime: now,
    }
    .encode()
    .to_vec()
}

fn metadata_item(owner: u64) -> Vec<u8> {
    let mut d = alloc::vec![0u8; EXTENT_ITEM_LEN + 9];
    put_u64(&mut d, 0, 1); // refs
    put_u64(&mut d, 8, GEN);
    put_u64(&mut d, 16, EXTENT_FLAG_TREE_BLOCK);
    d[24] = TREE_BLOCK_REF_KEY;
    put_u64(&mut d, 25, owner);
    d
}

/// Format the device. The filesystem can then be mounted (and populated)
/// with [`crate::Btrfs::mount`].
pub fn format(dev: &dyn BlockDevice, opts: &MkfsOptions) -> Result<()> {
    let layout = plan(dev.size())?;
    let ns = NODESIZE as u64;

    // Chunks: (logical/physical start, len, flags).
    let mut chunks: Vec<(u64, u64, u64)> = alloc::vec![
        (SYS_CHUNK_START, SYS_CHUNK_LEN, BLOCK_GROUP_SYSTEM),
        (META_CHUNK_START, layout.meta_len, BLOCK_GROUP_METADATA),
    ];
    for &(start, len) in &layout.data_chunks {
        chunks.push((start, len, BLOCK_GROUP_DATA));
    }
    let chunk_items: Vec<(Key, Vec<u8>)> = {
        let mut items: Vec<(Key, Vec<u8>)> = Vec::new();
        let dev_item = DevItem {
            devid: 1,
            total_bytes: layout.total_bytes,
            bytes_used: chunks.iter().map(|c| c.1).sum(),
            uuid: opts.dev_uuid,
            fsid: opts.fsid,
        };
        items.push((
            Key::new(DEV_ITEMS_OBJECTID, DEV_ITEM_KEY, 1),
            dev_item.encode(SECTORSIZE).to_vec(),
        ));
        for &(start, len, flags) in &chunks {
            let chunk = ChunkItem {
                length: len,
                owner: EXTENT_TREE,
                stripe_len: 65536,
                type_: flags,
                io_align: 65536,
                io_width: 65536,
                sector_size: SECTORSIZE,
                sub_stripes: 1,
                stripes: alloc::vec![Stripe {
                    devid: 1,
                    offset: start,
                }],
            };
            items.push((
                Key::new(FIRST_CHUNK_TREE_OBJECTID, CHUNK_ITEM_KEY, start),
                chunk.encode(&opts.dev_uuid),
            ));
        }
        items
    };

    // Root tree.
    let now = opts.now;
    let root_items: Vec<(Key, Vec<u8>)> = alloc::vec![
        (
            Key::new(EXTENT_TREE, ROOT_ITEM_KEY, 0),
            root_item(layout.extent_leaf, 0, [0; 16], now),
        ),
        (
            Key::new(DEV_TREE, ROOT_ITEM_KEY, 0),
            root_item(layout.dev_leaf, 0, [0; 16], now),
        ),
        (
            Key::new(FS_TREE, INODE_REF_KEY, ROOT_TREE_DIR),
            encode_inode_ref(0, b"default"),
        ),
        (
            Key::new(FS_TREE, ROOT_ITEM_KEY, 0),
            root_item(layout.fs_leaf, FIRST_FREE_OBJECTID, opts.subvol_uuid, now),
        ),
        (
            Key::new(ROOT_TREE_DIR, INODE_ITEM_KEY, 0),
            dir_inode_item(now, ns),
        ),
        (
            Key::new(ROOT_TREE_DIR, INODE_REF_KEY, ROOT_TREE_DIR),
            encode_inode_ref(0, b".."),
        ),
        (
            Key::new(ROOT_TREE_DIR, DIR_ITEM_KEY, crc::name_hash(b"default")),
            DirEntryRaw {
                location: Key::new(FS_TREE, ROOT_ITEM_KEY, u64::MAX),
                transid: 0,
                dir_type: FT_DIR,
                name: b"default".to_vec(),
                data: Vec::new(),
            }
            .encode(),
        ),
        (
            Key::new(CSUM_TREE, ROOT_ITEM_KEY, 0),
            root_item(layout.csum_leaf, 0, [0; 16], now),
        ),
        (
            Key::new(UUID_TREE, ROOT_ITEM_KEY, 0),
            root_item(layout.uuid_leaf, 0, [0; 16], now),
        ),
        (
            Key::new(DATA_RELOC_TREE, ROOT_ITEM_KEY, 0),
            root_item(layout.dreloc_leaf, FIRST_FREE_OBJECTID, [0; 16], now),
        ),
    ];

    // Extent tree: metadata items + block-group items, sorted by address.
    let meta_blocks: [(u64, u64); 8] = [
        (layout.chunk_leaf, CHUNK_TREE),
        (layout.root_leaf, ROOT_TREE),
        (layout.extent_leaf, EXTENT_TREE),
        (layout.dev_leaf, DEV_TREE),
        (layout.fs_leaf, FS_TREE),
        (layout.csum_leaf, CSUM_TREE),
        (layout.uuid_leaf, UUID_TREE),
        (layout.dreloc_leaf, DATA_RELOC_TREE),
    ];
    let mut extent_items: Vec<(Key, Vec<u8>)> = Vec::new();
    for &(bytenr, owner) in &meta_blocks {
        extent_items.push((Key::new(bytenr, METADATA_ITEM_KEY, 0), metadata_item(owner)));
    }
    for &(start, len, flags) in &chunks {
        let used = match flags {
            BLOCK_GROUP_SYSTEM => ns,
            BLOCK_GROUP_METADATA => 7 * ns,
            _ => 0,
        };
        extent_items.push((
            Key::new(start, BLOCK_GROUP_ITEM_KEY, len),
            BlockGroupItem { used, flags }.encode().to_vec(),
        ));
    }
    extent_items.sort_by(|a, b| a.0.cmp(&b.0));

    // Dev tree.
    let mut dev_items: Vec<(Key, Vec<u8>)> = Vec::new();
    for &(start, len, _) in &chunks {
        let ext = DevExtent {
            chunk_offset: start,
            length: len,
        };
        dev_items.push((
            Key::new(1, DEV_EXTENT_KEY, start),
            ext.encode(&opts.chunk_uuid).to_vec(),
        ));
    }

    // FS tree (root directory inode).
    let fs_items: Vec<(Key, Vec<u8>)> = alloc::vec![
        (
            Key::new(FIRST_FREE_OBJECTID, INODE_ITEM_KEY, 0),
            dir_inode_item(now, ns),
        ),
        (
            Key::new(FIRST_FREE_OBJECTID, INODE_REF_KEY, FIRST_FREE_OBJECTID),
            encode_inode_ref(0, b".."),
        ),
    ];

    // UUID tree: subvol uuid → id 5.
    let uuid_items: Vec<(Key, Vec<u8>)> = {
        let objectid = get_u64(&opts.subvol_uuid, 0);
        let offset = get_u64(&opts.subvol_uuid, 8);
        let mut val = alloc::vec![0u8; 8];
        put_u64(&mut val, 0, FS_TREE);
        alloc::vec![(Key::new(objectid, 251 /* UUID_KEY_SUBVOL */, offset), val)]
    };

    // Data reloc tree: like an empty fs tree but nbytes 0.
    let dreloc_items: Vec<(Key, Vec<u8>)> = alloc::vec![
        (
            Key::new(FIRST_FREE_OBJECTID, INODE_ITEM_KEY, 0),
            dir_inode_item(now, 0),
        ),
        (
            Key::new(FIRST_FREE_OBJECTID, INODE_REF_KEY, FIRST_FREE_OBJECTID),
            encode_inode_ref(0, b".."),
        ),
    ];

    // Zero the reserved first MiB (except where the superblock will go).
    let zeros = alloc::vec![0u8; 65536];
    let mut off = 0u64;
    while off < SYS_CHUNK_START {
        dev.write_at(off, &zeros)?;
        off += zeros.len() as u64;
    }

    dev.write_at(
        layout.chunk_leaf,
        &build_leaf(opts, CHUNK_TREE, layout.chunk_leaf, &chunk_items)?,
    )?;
    dev.write_at(
        layout.root_leaf,
        &build_leaf(opts, ROOT_TREE, layout.root_leaf, &root_items)?,
    )?;
    dev.write_at(
        layout.extent_leaf,
        &build_leaf(opts, EXTENT_TREE, layout.extent_leaf, &extent_items)?,
    )?;
    dev.write_at(
        layout.dev_leaf,
        &build_leaf(opts, DEV_TREE, layout.dev_leaf, &dev_items)?,
    )?;
    dev.write_at(
        layout.fs_leaf,
        &build_leaf(opts, FS_TREE, layout.fs_leaf, &fs_items)?,
    )?;
    dev.write_at(
        layout.csum_leaf,
        &build_leaf(opts, CSUM_TREE, layout.csum_leaf, &[])?,
    )?;
    dev.write_at(
        layout.uuid_leaf,
        &build_leaf(opts, UUID_TREE, layout.uuid_leaf, &uuid_items)?,
    )?;
    dev.write_at(
        layout.dreloc_leaf,
        &build_leaf(opts, DATA_RELOC_TREE, layout.dreloc_leaf, &dreloc_items)?,
    )?;

    // Superblock.
    let mut raw = alloc::vec![0u8; SUPERBLOCK_SIZE];
    raw[sb::OFF_FSID..sb::OFF_FSID + 16].copy_from_slice(&opts.fsid);
    put_u64(&mut raw, sb::OFF_FLAGS, 0x1);
    put_u64(&mut raw, sb::OFF_MAGIC, SUPERBLOCK_MAGIC);
    put_u64(&mut raw, sb::OFF_GENERATION, GEN);
    put_u64(&mut raw, sb::OFF_ROOT, layout.root_leaf);
    put_u64(&mut raw, sb::OFF_CHUNK_ROOT, layout.chunk_leaf);
    put_u64(&mut raw, sb::OFF_TOTAL_BYTES, layout.total_bytes);
    put_u64(&mut raw, sb::OFF_BYTES_USED, 8 * ns);
    put_u64(&mut raw, sb::OFF_ROOT_DIR_OBJECTID, ROOT_TREE_DIR);
    put_u64(&mut raw, sb::OFF_NUM_DEVICES, 1);
    put_u32(&mut raw, sb::OFF_SECTORSIZE, SECTORSIZE);
    put_u32(&mut raw, sb::OFF_NODESIZE, NODESIZE);
    put_u32(&mut raw, sb::OFF_LEAFSIZE, NODESIZE);
    put_u32(&mut raw, sb::OFF_STRIPESIZE, SECTORSIZE);
    put_u64(&mut raw, sb::OFF_CHUNK_ROOT_GENERATION, GEN);
    put_u64(
        &mut raw,
        sb::OFF_INCOMPAT_FLAGS,
        INCOMPAT_MIXED_BACKREF
            | INCOMPAT_EXTENDED_IREF
            | INCOMPAT_SKINNY_METADATA
            | INCOMPAT_NO_HOLES,
    );
    // csum_type = 0 (crc32c); root/chunk_root levels = 0.
    let dev_item = DevItem {
        devid: 1,
        total_bytes: layout.total_bytes,
        bytes_used: chunks.iter().map(|c| c.1).sum(),
        uuid: opts.dev_uuid,
        fsid: opts.fsid,
    };
    raw[sb::OFF_DEV_ITEM..sb::OFF_DEV_ITEM + DEV_ITEM_LEN]
        .copy_from_slice(&dev_item.encode(SECTORSIZE));
    let label = opts.label.as_bytes();
    let n = label.len().min(255);
    raw[sb::OFF_LABEL..sb::OFF_LABEL + n].copy_from_slice(&label[..n]);
    put_u64(&mut raw, sb::OFF_CACHE_GENERATION, u64::MAX);
    raw[sb::OFF_METADATA_UUID..sb::OFF_METADATA_UUID + 16].fill(0);
    // sys_chunk_array: the SYSTEM chunk.
    let sys_chunk = ChunkItem {
        length: SYS_CHUNK_LEN,
        owner: EXTENT_TREE,
        stripe_len: 65536,
        type_: BLOCK_GROUP_SYSTEM,
        io_align: 65536,
        io_width: 65536,
        sector_size: SECTORSIZE,
        sub_stripes: 1,
        stripes: alloc::vec![Stripe {
            devid: 1,
            offset: SYS_CHUNK_START,
        }],
    };
    let sys_data = sys_chunk.encode(&opts.dev_uuid);
    Key::new(FIRST_CHUNK_TREE_OBJECTID, CHUNK_ITEM_KEY, SYS_CHUNK_START)
        .write(&mut raw, sb::OFF_SYS_CHUNK_ARRAY);
    raw[sb::OFF_SYS_CHUNK_ARRAY + KEY_SIZE..sb::OFF_SYS_CHUNK_ARRAY + KEY_SIZE + sys_data.len()]
        .copy_from_slice(&sys_data);
    put_u32(
        &mut raw,
        sb::OFF_SYS_CHUNK_ARRAY_SIZE,
        (KEY_SIZE + sys_data.len()) as u32,
    );

    for &off in SUPERBLOCK_OFFSETS.iter() {
        if off + SUPERBLOCK_SIZE as u64 <= layout.total_bytes {
            put_u64(&mut raw, sb::OFF_BYTENR, off);
            let sum = crc::checksum(&raw[CSUM_SIZE..]);
            raw[..CSUM_SIZE].fill(0);
            put_u32(&mut raw, 0, sum);
            dev.write_at(off, &raw)?;
        }
    }
    dev.sync()
}
