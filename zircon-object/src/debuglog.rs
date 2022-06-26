//! Objects for Kernel Debuglog.
use {
    super::*,
    crate::object::*,
    alloc::{sync::Arc, vec::Vec},
    lazy_static::lazy_static,
    lock::Mutex,
};

lazy_static! {
    static ref DLOG: Mutex<DlogBuffer> = Mutex::new(DlogBuffer {
        buf: Vec::with_capacity(0x1000),
    });
}

/// Debuglog - Kernel debuglog
///
/// ## SYNOPSIS
///
/// Debuglog objects allow userspace to read and write to kernel debug logs.
pub struct DebugLog {
    base: KObjectBase,
    flags: u32,
    read_offset: Mutex<usize>,
}

struct DlogBuffer {
    /// Append only buffer
    buf: Vec<u8>,
}

impl_kobject!(DebugLog);

impl DebugLog {
    /// Create a new `DebugLog`.
    pub fn create(flags: u32) -> Arc<Self> {
        Arc::new(DebugLog {
            base: KObjectBase::new(),
            flags,
            read_offset: Default::default(),
        })
    }

    /// Read a log, return the actual read size.
    pub fn read(&self, buf: &mut [u8]) -> usize {
        let mut offset = self.read_offset.lock();
        let len = DLOG.lock().read_at(*offset, buf);
        *offset += len;
        len
    }

    /// Write a log.
    pub fn write(&self, severity: Severity, flags: u32, tid: u64, pid: u64, data: &str) {
        DLOG.lock()
            .write(severity, flags | self.flags, tid, pid, data.as_bytes());
    }
}

#[repr(C)]
#[derive(Debug)]
struct DlogHeader {
    rollout: u32,
    datalen: u16,
    severity: Severity,
    flags: u8,
    timestamp: u64,
    pid: u64,
    tid: u64,
}

/// Log entry severity. Used for coarse filtering of log messages.
#[allow(missing_docs)]
#[repr(u8)]
#[derive(Debug)]
pub enum Severity {
    Trace = 0x10,
    Debug = 0x20,
    Info = 0x30,
    Warning = 0x40,
    Error = 0x50,
    Fatal = 0x60,
}

const HEADER_SIZE: usize = core::mem::size_of::<DlogHeader>();
/// Max length of Dlog read buffer.
pub const DLOG_MAX_LEN: usize = 256;

#[allow(unsafe_code)]
impl DlogBuffer {
    /// Read one record at offset.
    #[allow(clippy::cast_ptr_alignment)]
    fn read_at(&mut self, offset: usize, buf: &mut [u8]) -> usize {
        assert!(buf.len() >= DLOG_MAX_LEN);
        if offset == self.buf.len() {
            return 0;
        }
        let header_buf = &self.buf[offset..offset + HEADER_SIZE];
        buf[..HEADER_SIZE].copy_from_slice(header_buf);
        let header = unsafe { &*(header_buf.as_ptr() as *const DlogHeader) };
        let len = (header.rollout & 0xFFF) as usize;
        buf[HEADER_SIZE..len].copy_from_slice(&self.buf[offset + HEADER_SIZE..offset + len]);
        len
    }

    fn write(&mut self, severity: Severity, flags: u32, tid: u64, pid: u64, data: &[u8]) {
        let wire_size = HEADER_SIZE + align_up_4(data.len());
        let size = HEADER_SIZE + data.len();
        let header = DlogHeader {
            rollout: ((size as u32) << 12) | (wire_size as u32),
            datalen: data.len() as u16,
            severity,
            flags: flags as u8,
            timestamp: kernel_hal::timer::timer_now().as_nanos() as u64,
            pid,
            tid,
        };
        let header_buf: [u8; HEADER_SIZE] = unsafe { core::mem::transmute(header) };
        self.buf.extend(header_buf.iter());
        self.buf.extend(data);
        self.buf.extend(&[0u8; 4][..wire_size - size]);
    }
}

fn align_up_4(x: usize) -> usize {
    (x + 3) & !3
}
