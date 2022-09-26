//! Simulating a fast disk with a physics core.
//! Synchronization is done through atomic_load_acquireIPIs and shared memory.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::intrinsics::{atomic_load_acquire, atomic_store_release};
use core::sync::atomic::{AtomicBool, Ordering};
use kernel_hal::{
    cpu::cpu_id,
    interrupt::{send_ipi, wait_for_interrupt},
    timer::timer_now,
    IpiReason, LazyInit, MpscQueue,
};

type SubmitQueue = MpscQueue<'static, Entry>;

static SQ: LazyInit<Arc<SubmitQueue>> = LazyInit::new();

/// Submiting(client) side of the mock disk
pub struct MockBlock {
    sq: Arc<SubmitQueue>,
}

impl MockBlock {
    /// Wait until the mock disk is inited
    pub fn new() -> Self {
        while !MOCK_DISK_READY.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }
        Self { sq: SQ.clone() }
    }

    fn submit_entry(&self, start: EntryType, op: OpCode, buf: &[u8], finish: *mut usize) {
        let idx = loop {
            if let Some(idx) = self.sq.alloc_entry() {
                break idx;
            }
            core::hint::spin_loop();
        };
        let mut entry = self.sq.entry_at(idx);
        entry.start = start;
        entry.op = op;
        entry.buf_ptr = buf.as_ptr() as _;
        entry.buf_size = buf.len();
        entry.cpuid = cpu_id() as _;
        entry.finish = finish;
        self.sq.commit_entry(idx);
        trace!("entry submit : {:#x?} @ {}", entry, idx);
    }
}

impl Default for MockBlock {
    fn default() -> Self {
        Self::new()
    }
}

use rcore_fs::dev::{Device, Result};

#[allow(unsafe_code)]
impl Device for MockBlock {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        let mut finish: Box<usize> = Box::new(0);
        let ptr = finish.as_mut() as *mut usize;
        self.submit_entry(EntryType::Offset(offset), OpCode::Read, buf, ptr);
        while unsafe { atomic_load_acquire(ptr) } == 0 {
            wait_for_interrupt();
        }
        assert_eq!(*finish, BLKSIZE);
        Ok(buf.len())
    }
    #[allow(unsafe_code)]
    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize> {
        let mut finish: Box<usize> = Box::new(0);
        let ptr = finish.as_mut() as *mut usize;
        self.submit_entry(EntryType::Offset(offset), OpCode::Write, buf, ptr);
        while unsafe { atomic_load_acquire(ptr) } == 0 {
            wait_for_interrupt();
        }
        assert_eq!(*finish, BLKSIZE);
        Ok(buf.len())
    }
    fn sync(&self) -> Result<()> {
        Ok(())
    }
}

struct Mocking {
    data: MemBuf,
    sq: Arc<SubmitQueue>,
    /// SAFETY:
    ///
    /// this atomic var is static
    map: BTreeMap<u128, Entry>,
}

impl Mocking {
    pub fn new(data: &'static mut [u8], sq: Arc<SubmitQueue>) -> Self {
        Self {
            data: MemBuf(data),
            sq,
            map: BTreeMap::new(),
        }
    }

    fn handle_submits(&mut self) {
        for (idx, entry) in self.sq.consume_entrys().iter_mut() {
            trace!("entry received : {:#x?} @ {}", entry, idx);
            let stime = timer_now().as_nanos();
            self.data.handle_entry(entry);
            let etime = timer_now().as_nanos();
            let finish_time = etime + 10 * (etime - stime);
            self.map.insert(finish_time, *entry);
        }
    }

    #[allow(unsafe_code)]
    fn handle_finished(&mut self) {
        let now = timer_now().as_nanos();
        while let Some((key, ..)) = self.map.first_key_value() {
            if *key > now {
                break;
            }
            let (_, entry) = self.map.pop_first().unwrap();
            unsafe { atomic_store_release(entry.finish, BLKSIZE) };
            let reason = IpiReason::MockBlock { block_info: 0 };
            send_ipi(entry.cpuid, reason.into()).unwrap();
        }
    }
}

const BLKSIZE: usize = 512;
const CORE_NUM: usize = 4;
const QUEUE_SIZE: usize = 0x100 * CORE_NUM;
static MOCK_DISK_READY: AtomicBool = AtomicBool::new(false);
const ENTRY: Entry = Entry::new();
static mut QUEUE_BUF: [Entry; QUEUE_SIZE] = [ENTRY; QUEUE_SIZE];

/// Start simulating
#[allow(unsafe_code)]
pub fn mocking(initrd: &'static mut [u8]) -> ! {
    SQ.init_by(Arc::new(SubmitQueue::new(unsafe { &mut QUEUE_BUF })));
    let mut mock = Mocking::new(initrd, SQ.clone());
    MOCK_DISK_READY.store(true, Ordering::Release);
    loop {
        mock.handle_submits();
        mock.handle_finished();
    }
}

#[repr(usize)]
#[derive(Debug, Copy, Clone)]
#[allow(dead_code)]
enum OpCode {
    Read,
    Write,
    Flush,
}

#[derive(Debug, Copy, Clone)]
enum EntryType {
    Block(usize),
    Offset(usize),
}

#[repr(C)]
#[derive(Copy, Clone)]
struct Entry {
    start: EntryType,
    op: OpCode,
    buf_ptr: usize,
    buf_size: usize,
    cpuid: usize,
    /// Safety:
    ///
    /// This access was portected by atomic operations
    finish: *mut usize,
}

use core::fmt;

impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Entry")
            .field("type", &self.start)
            .field("op", &self.op)
            .field("buf_ptr", &self.buf_ptr)
            .field("buf_len", &self.buf_size)
            .field("cpuid", &self.cpuid)
            .field("finish", &self.finish)
            .finish()
    }
}

impl Entry {
    const fn new() -> Self {
        Self {
            start: EntryType::Block(0),
            op: OpCode::Read,
            buf_ptr: 0,
            buf_size: 0,
            cpuid: 0,
            finish: 0 as *mut usize,
        }
    }
}

struct MemBuf(&'static mut [u8]);

impl MemBuf {
    #[allow(unsafe_code)]
    pub fn handle_entry(&mut self, entry: &mut Entry) {
        match entry.op {
            OpCode::Read => {
                let buf =
                    unsafe { alloc::slice::from_raw_parts_mut(entry.buf_ptr as _, entry.buf_size) };
                match entry.start {
                    EntryType::Block(block_id) => self.read_block(block_id, buf),
                    EntryType::Offset(offset) => self.read_at(offset, buf),
                }
            }
            OpCode::Write => {
                let buf =
                    unsafe { alloc::slice::from_raw_parts(entry.buf_ptr as _, entry.buf_size) };
                match entry.start {
                    EntryType::Block(block_id) => self.write_block(block_id, buf),
                    EntryType::Offset(offset) => self.write_at(offset, buf),
                }
            }
            _ => { /* IGNORED */ }
        }
    }

    fn read_block(&self, block_id: usize, buf: &mut [u8]) {
        let start = block_id * BLKSIZE;
        let end = start + BLKSIZE;
        assert!(buf.len() == BLKSIZE);
        assert!(end <= self.0.len());
        buf.copy_from_slice(&(self.0)[start..end]);
    }

    fn write_block(&mut self, block_id: usize, buf: &[u8]) {
        let start = block_id * BLKSIZE;
        let end = start + BLKSIZE;
        assert!(buf.len() == BLKSIZE);
        assert!(end <= self.0.len());
        (self.0)[start..end].copy_from_slice(buf);
    }

    fn read_at(&self, offset: usize, buf: &mut [u8]) {
        let start = offset;
        let end = start + buf.len();
        assert!(end <= self.0.len());
        buf.copy_from_slice(&(self.0)[start..end]);
    }

    fn write_at(&mut self, offset: usize, buf: &[u8]) {
        let start = offset;
        let end = start + buf.len();
        assert!(end <= self.0.len());
        (self.0)[start..end].copy_from_slice(buf);
    }
}
