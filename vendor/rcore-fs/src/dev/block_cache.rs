//! A naive LRU cache layer for `BlockDevice`
use super::*;
use alloc::{vec, vec::Vec};
use spin::{Mutex, MutexGuard};

pub struct BlockCache<T: BlockDevice> {
    device: T,
    bufs: Vec<Mutex<Buf>>,
    lru: Mutex<LRU>,
}

struct Buf {
    status: BufStatus,
    data: Vec<u8>,
}

enum BufStatus {
    /// buffer is unused
    Unused,
    /// buffer has been read from disk
    Valid(BlockId),
    /// buffer needs to be written to disk
    Dirty(BlockId),
}

impl<T: BlockDevice> BlockCache<T> {
    pub fn new(device: T, capacity: usize) -> Self {
        let mut bufs = Vec::new();
        bufs.resize_with(capacity, || {
            Mutex::new(Buf {
                status: BufStatus::Unused,
                data: vec![0; 1 << T::BLOCK_SIZE_LOG2 as usize],
            })
        });
        let lru = Mutex::new(LRU::new(capacity));
        BlockCache { device, bufs, lru }
    }

    /// Get a buffer for `block_id` with any status
    fn get_buf(&self, block_id: BlockId) -> MutexGuard<Buf> {
        let (i, buf) = self._get_buf(block_id);
        self.lru.lock().visit(i);
        buf
    }

    fn _get_buf(&self, block_id: BlockId) -> (usize, MutexGuard<Buf>) {
        for (i, buf) in self.bufs.iter().enumerate() {
            if let Some(lock) = buf.try_lock() {
                match lock.status {
                    BufStatus::Valid(id) if id == block_id => return (i, lock),
                    BufStatus::Dirty(id) if id == block_id => return (i, lock),
                    _ => {}
                }
            }
        }
        self.get_unused()
    }

    /// Get an unused buffer
    fn get_unused(&self) -> (usize, MutexGuard<Buf>) {
        for (i, buf) in self.bufs.iter().enumerate() {
            if let Some(lock) = buf.try_lock() {
                if let BufStatus::Unused = lock.status {
                    return (i, lock);
                }
            }
        }
        let victim_id = self.lru.lock().victim();
        let mut victim = self.bufs[victim_id].lock();
        self.write_back(&mut victim).expect("failed to write back");
        victim.status = BufStatus::Unused;
        (victim_id, victim)
    }

    /// Write back data if buffer is dirty
    fn write_back(&self, buf: &mut Buf) -> Result<()> {
        if let BufStatus::Dirty(block_id) = buf.status {
            self.device.write_at(block_id, &buf.data)?;
            buf.status = BufStatus::Valid(block_id);
        }
        Ok(())
    }
}

impl<T: BlockDevice> Drop for BlockCache<T> {
    fn drop(&mut self) {
        BlockDevice::sync(self).expect("failed to sync");
    }
}

impl<T: BlockDevice> BlockDevice for BlockCache<T> {
    const BLOCK_SIZE_LOG2: u8 = T::BLOCK_SIZE_LOG2;

    fn read_at(&self, block_id: BlockId, buffer: &mut [u8]) -> Result<()> {
        let mut buf = self.get_buf(block_id);
        if let BufStatus::Unused = buf.status {
            // read from device
            self.device.read_at(block_id, &mut buf.data)?;
            buf.status = BufStatus::Valid(block_id);
        }
        let len = 1 << Self::BLOCK_SIZE_LOG2 as usize;
        buffer[..len].copy_from_slice(&buf.data);
        Ok(())
    }

    fn write_at(&self, block_id: BlockId, buffer: &[u8]) -> Result<()> {
        let mut buf = self.get_buf(block_id);
        buf.status = BufStatus::Dirty(block_id);
        let len = 1 << Self::BLOCK_SIZE_LOG2 as usize;
        buf.data.copy_from_slice(&buffer[..len]);
        Ok(())
    }

    fn sync(&self) -> Result<()> {
        for buf in self.bufs.iter() {
            self.write_back(&mut buf.lock())?;
        }
        self.device.sync()?;
        Ok(())
    }
}

/// Doubly circular linked list LRU manager
#[allow(clippy::upper_case_acronyms)]
struct LRU {
    prev: Vec<usize>,
    next: Vec<usize>,
}

impl LRU {
    fn new(size: usize) -> Self {
        LRU {
            prev: (size - 1..size).chain(0..size - 1).collect(),
            next: (1..size).chain(0..1).collect(),
        }
    }
    /// Visit element `id`, move it to head.
    fn visit(&mut self, id: usize) {
        if id == 0 || id >= self.prev.len() {
            return;
        }
        self._list_remove(id);
        self._list_insert_head(id);
    }
    /// Get a victim at tail.
    fn victim(&self) -> usize {
        self.prev[0]
    }
    fn _list_remove(&mut self, id: usize) {
        let prev = self.prev[id];
        let next = self.next[id];
        self.prev[next] = prev;
        self.next[prev] = next;
    }
    fn _list_insert_head(&mut self, id: usize) {
        let head = self.next[0];
        self.prev[id] = 0;
        self.next[id] = head;
        self.next[0] = id;
        self.prev[head] = id;
    }
}
