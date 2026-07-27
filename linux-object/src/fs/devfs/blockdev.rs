use alloc::sync::Arc;
use core::any::Any;
use rcore_fs::vfs::{make_rdev, FileType, FsError, INode, Metadata, PollStatus, Result, Timespec};
use rcore_fs_devfs::DevFS;
use zcore_drivers::{scheme::BlockScheme, scheme::Scheme, DeviceError};

/// Linux `BLKGETSIZE64` — total size in bytes (`linux/fs.h`).
const BLKGETSIZE64: u32 = 0x8008_1272;
/// Linux `BLKGETSIZE` — size in 512-byte sectors (`linux/fs.h`).
const BLKGETSIZE: u32 = 0x0000_1260;
/// Linux `BLKFLSBUF` — flush block device buffers (`_IO(0x12,97)`).
const BLKFLSBUF: u32 = 0x0000_1261;

/// Block device INode.
pub struct BlockDev {
    index: usize,
    block: Arc<dyn BlockScheme>,
    inode_id: usize,
    name: alloc::string::String,
}

impl BlockDev {
    pub fn new(index: usize, block: Arc<dyn BlockScheme>, name: alloc::string::String) -> Self {
        Self {
            index,
            block,
            inode_id: DevFS::new_inode_id(),
            name,
        }
    }

    pub fn block_scheme(&self) -> Arc<dyn BlockScheme> {
        self.block.clone()
    }
}

impl INode for BlockDev {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        const BS: usize = 512;
        #[repr(align(4096))]
        struct AlignedBuf([u8; 512]);
        let mut temp_buf = AlignedBuf([0u8; 512]);
        let mut done = 0usize;

        // Partial leading sector.
        let head_off = offset % BS;
        if head_off != 0 && done < buf.len() {
            let take = buf.len().min(BS - head_off);
            self.block
                .read_block(offset / BS, &mut temp_buf.0)
                .map_err(convert_error)?;
            buf[..take].copy_from_slice(&temp_buf.0[head_off..head_off + take]);
            done += take;
        }

        // Whole-sector middle: a single multi-sector transfer.
        let mid = ((buf.len() - done) / BS) * BS;
        if mid > 0 {
            self.block
                .read_block((offset + done) / BS, &mut buf[done..done + mid])
                .map_err(convert_error)?;
            done += mid;
        }

        // Partial trailing sector.
        if done < buf.len() {
            let take = buf.len() - done;
            self.block
                .read_block((offset + done) / BS, &mut temp_buf.0)
                .map_err(convert_error)?;
            buf[done..].copy_from_slice(&temp_buf.0[..take]);
            done += take;
        }

        Ok(done)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize> {
        const BS: usize = 512;
        #[repr(align(4096))]
        struct AlignedBuf([u8; 512]);
        let mut temp_buf = AlignedBuf([0u8; 512]);
        let mut done = 0usize;

        // Partial leading sector: read-modify-write.
        let head_off = offset % BS;
        if head_off != 0 && done < buf.len() {
            let take = buf.len().min(BS - head_off);
            let block_id = offset / BS;
            self.block
                .read_block(block_id, &mut temp_buf.0)
                .map_err(convert_error)?;
            temp_buf.0[head_off..head_off + take].copy_from_slice(&buf[..take]);
            self.block
                .write_block(block_id, &temp_buf.0)
                .map_err(convert_error)?;
            done += take;
        }

        // Whole-sector middle: a single multi-sector transfer.
        let mid = ((buf.len() - done) / BS) * BS;
        if mid > 0 {
            self.block
                .write_block((offset + done) / BS, &buf[done..done + mid])
                .map_err(convert_error)?;
            done += mid;
        }

        // Partial trailing sector: read-modify-write.
        if done < buf.len() {
            let take = buf.len() - done;
            let block_id = (offset + done) / BS;
            self.block
                .read_block(block_id, &mut temp_buf.0)
                .map_err(convert_error)?;
            temp_buf.0[..take].copy_from_slice(&buf[done..]);
            self.block
                .write_block(block_id, &temp_buf.0)
                .map_err(convert_error)?;
            done += take;
        }

        Ok(done)
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: true,
            error: false,
        })
    }

    fn metadata(&self) -> Result<Metadata> {
        let blocks = self.block.block_count();
        let size = blocks.saturating_mul(512);
        Ok(Metadata {
            dev: 1,
            inode: self.inode_id,
            size,
            blk_size: 512,
            blocks,
            atime: Timespec { sec: 0, nsec: 0 },
            mtime: Timespec { sec: 0, nsec: 0 },
            ctime: Timespec { sec: 0, nsec: 0 },
            type_: FileType::BlockDevice,
            mode: 0o660, // owner & group read/write
            nlinks: 1,
            uid: 0,
            gid: 0,
            rdev: make_rdev(3, self.index),
        })
    }

    fn io_control(&self, cmd: u32, data: usize) -> Result<usize> {
        let sectors = self.block.block_count() as u64;
        match cmd {
            BLKGETSIZE64 => {
                if data == 0 {
                    return Err(FsError::InvalidParam);
                }
                let size = sectors.saturating_mul(512);
                let mut out_ptr = kernel_hal::user::UserOutPtr::<u64>::from(data);
                out_ptr.write(size).map_err(|_| FsError::InvalidParam)?;
                Ok(0)
            }
            BLKGETSIZE => {
                if data == 0 {
                    return Err(FsError::InvalidParam);
                }
                let legacy = sectors as usize;
                let mut out_ptr = kernel_hal::user::UserOutPtr::<usize>::from(data);
                out_ptr.write(legacy).map_err(|_| FsError::InvalidParam)?;
                Ok(0)
            }
            0x0000_125f => {
                // BLKRRPART
                crate::fs::rescan_partitions(&self.name, &self.block, self.index)
                    .map_err(|_| FsError::DeviceError)?;
                Ok(0)
            }
            BLKFLSBUF => {
                // Eclipse escribe directamente al dispositivo (sin caché de
                // bloques), así que basta con delegar el flush del driver y
                // devolver éxito en lugar de ENOSYS. Herramientas como el
                // instalador lo invocan para asegurar la persistencia.
                let _ = self.block.flush();
                Ok(0)
            }
            _ => Err(FsError::NotSupported),
        }
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn sync_all(&self) -> Result<()> {
        Ok(())
    }

    fn sync_data(&self) -> Result<()> {
        Ok(())
    }
}

fn convert_error(e: DeviceError) -> FsError {
    match e {
        DeviceError::NotSupported => FsError::NotSupported,
        DeviceError::NotReady => FsError::Busy,
        DeviceError::InvalidParam => FsError::InvalidParam,
        DeviceError::BufferTooSmall
        | DeviceError::DmaError
        | DeviceError::IoError
        | DeviceError::AlreadyExists
        | DeviceError::NoResources => FsError::DeviceError,
    }
}

/// A wrapper block device that represents a partition on a physical block device.
pub struct PartitionBlock {
    parent: Arc<dyn BlockScheme>,
    name: alloc::string::String,
    start_block: usize,
    block_count: usize,
}

impl PartitionBlock {
    pub fn new(
        parent: Arc<dyn BlockScheme>,
        name: alloc::string::String,
        start_block: usize,
        block_count: usize,
    ) -> Self {
        Self {
            parent,
            name,
            start_block,
            block_count,
        }
    }
}

impl Scheme for PartitionBlock {
    fn name(&self) -> &str {
        &self.name
    }
    fn handle_irq(&self, irq_num: usize) {
        self.parent.handle_irq(irq_num);
    }
}

impl BlockScheme for PartitionBlock {
    fn read_block(&self, block_id: usize, buf: &mut [u8]) -> zcore_drivers::DeviceResult {
        let nsectors = buf.len() / 512;
        if block_id + nsectors > self.block_count {
            return Err(zcore_drivers::DeviceError::InvalidParam);
        }
        self.parent.read_block(self.start_block + block_id, buf)
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) -> zcore_drivers::DeviceResult {
        let nsectors = buf.len() / 512;
        if block_id + nsectors > self.block_count {
            return Err(zcore_drivers::DeviceError::InvalidParam);
        }
        self.parent.write_block(self.start_block + block_id, buf)
    }

    fn flush(&self) -> zcore_drivers::DeviceResult {
        self.parent.flush()
    }

    fn block_count(&self) -> usize {
        self.block_count
    }
}

/// Scans a block device for partition tables (MBR/GPT) and returns a vector
/// of partitions as (start_sector, size_sectors) pairs.
pub fn scan_partitions(block: &Arc<dyn BlockScheme>) -> alloc::vec::Vec<(usize, usize)> {
    let mut partitions = alloc::vec::Vec::new();
    #[repr(align(4096))]
    struct AlignedBuf([u8; 512]);
    let mut mbr_buffer = AlignedBuf([0u8; 512]);
    if block.read_block(0, &mut mbr_buffer.0).is_err() {
        return partitions;
    }
    let boot_signature = u16::from_le_bytes([mbr_buffer.0[510], mbr_buffer.0[511]]);
    if boot_signature != 0xAA55 {
        return partitions;
    }

    // Check for GPT protective MBR or GPT header signature
    let is_gpt_mbr = mbr_buffer.0[450] == 0xEE;
    let mut is_gpt_header = false;
    let mut gpt_header_buf = AlignedBuf([0u8; 512]);
    if block.read_block(1, &mut gpt_header_buf.0).is_ok() && &gpt_header_buf.0[0..8] == b"EFI PART"
    {
        is_gpt_header = true;
    }

    if is_gpt_mbr || is_gpt_header {
        // GPT: read entries from sectors 2..=5 (up to 16 partitions)
        let mut gpt_buffer = AlignedBuf([0u8; 512]);
        for sector_id in 2..=5 {
            if block.read_block(sector_id, &mut gpt_buffer.0).is_err() {
                break;
            }
            for i in 0..4 {
                let offset = i * 128;
                let partition_entry = &gpt_buffer.0[offset..offset + 128];
                if !partition_entry.iter().all(|&b| b == 0) {
                    let start_sector = u64::from_le_bytes([
                        partition_entry[32],
                        partition_entry[33],
                        partition_entry[34],
                        partition_entry[35],
                        partition_entry[36],
                        partition_entry[37],
                        partition_entry[38],
                        partition_entry[39],
                    ]) as usize;
                    let end_sector = u64::from_le_bytes([
                        partition_entry[40],
                        partition_entry[41],
                        partition_entry[42],
                        partition_entry[43],
                        partition_entry[44],
                        partition_entry[45],
                        partition_entry[46],
                        partition_entry[47],
                    ]) as usize;
                    if end_sector >= start_sector && start_sector > 0 {
                        let size_sectors = end_sector - start_sector + 1;
                        partitions.push((start_sector, size_sectors));
                    }
                }
            }
        }
    } else {
        // MBR: parse partitions from sector 0
        for i in 0..4 {
            let offset = 446 + (i * 16);
            let partition_entry = &mbr_buffer.0[offset..offset + 16];
            let part_type = partition_entry[4];
            if part_type != 0 {
                let start_sector = u32::from_le_bytes([
                    partition_entry[8],
                    partition_entry[9],
                    partition_entry[10],
                    partition_entry[11],
                ]) as usize;
                let size_sectors = u32::from_le_bytes([
                    partition_entry[12],
                    partition_entry[13],
                    partition_entry[14],
                    partition_entry[15],
                ]) as usize;
                if size_sectors > 0 && start_sector > 0 {
                    partitions.push((start_sector, size_sectors));
                }
            }
        }
    }
    partitions
}
