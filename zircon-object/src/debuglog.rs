use {
    super::*, crate::object::*, alloc::sync::Arc, core::convert::TryInto, kernel_hal::serial_write,
    spin::Mutex,
};

static DLOG: Mutex<DlogBuffer> = Mutex::new(DlogBuffer::new());

#[cfg(not(test))]
const DLOG_SIZE: usize = 128usize * 1024usize;
#[cfg(test)]
const DLOG_SIZE: usize = 2usize * 1024usize;
const DLOG_MASK: usize = DLOG_SIZE - 1;
const DLOG_MIN_RECORD: usize = 32usize;

#[repr(C)]
struct DlogHeader {
    header: u32,
    datalen: u16,
    flags: u16,
    timestamp: u64,
    pid: u64,
    tid: u64,
}

pub struct DebugLog {
    base: KObjectBase,
    flags: u32,
}

impl_kobject!(DebugLog);

impl DebugLog {
    pub fn create(flags: u32) -> Arc<Self> {
        Arc::new(DebugLog {
            base: KObjectBase::new(),
            flags,
        })
    }

    pub fn write(&self, flags: u32, data: &str, tid: u64, pid: u64) -> ZxResult<usize> {
        let flags = flags | self.flags;
        DLOG.lock().write(flags, data.as_bytes(), tid, pid);
        serial_write(data);
        Ok(0)
    }
}

struct DlogBuffer {
    buf: [u8; DLOG_SIZE],
    head: usize,
    tail: usize,
}

impl DlogBuffer {
    pub const fn new() -> Self {
        DlogBuffer {
            buf: [0u8; DLOG_SIZE],
            head: 0usize,
            tail: 0usize,
        }
    }

    #[allow(unsafe_code)]
    pub fn write(&mut self, flags: u32, data: &[u8], tid: u64, pid: u64) {
        let wire_size = DLOG_MIN_RECORD + ((data.len() + 3) & !3);
        let header_flag = (((DLOG_MIN_RECORD + data.len()) as u32 & 0xFFFu32) << 12)
            | (wire_size as u32 & 0xFFFu32);
        let header = DlogHeader {
            header: header_flag,
            datalen: data.len() as u16,
            flags: flags as u16,
            timestamp: 0u64, // FIXME timer_now() should be used here
            pid,
            tid,
        };
        let serde_header: [u8; core::mem::size_of::<DlogHeader>()] =
            unsafe { core::mem::transmute(header) };
        let head = self.head;
        while (head - self.tail) > (DLOG_SIZE - wire_size) {
            let tail_index = self.tail & DLOG_MASK;
            let header: u32 =
                u32::from_ne_bytes(self.buf[tail_index..tail_index + 4].try_into().unwrap());
            self.tail += (header & 0xFFF) as usize;
        }
        let mut offset = head & DLOG_MASK;
        let fifo_size = DLOG_SIZE - offset;
        if fifo_size >= wire_size {
            self.copy_and_write(offset, &serde_header);
            self.copy_and_write(offset + DLOG_MIN_RECORD, data);
        } else if fifo_size < DLOG_MIN_RECORD {
            self.copy_and_write(offset, &serde_header[..fifo_size]);
            self.copy_and_write(0, &serde_header[fifo_size..]);
            self.copy_and_write(DLOG_MIN_RECORD - fifo_size, data);
        } else {
            self.copy_and_write(offset, &serde_header);
            offset += DLOG_MIN_RECORD;
            if offset < DLOG_SIZE {
                let fifo_size = DLOG_SIZE - offset;
                self.copy_and_write(offset, &data[..fifo_size]);
                self.copy_and_write(0, &data[fifo_size..]);
            } else {
                self.copy_and_write(0, data);
            }
        }
        self.head += wire_size;
    }

    fn copy_and_write(&mut self, start: usize, data: &[u8]) {
        let end = start + data.len();
        assert!(start < DLOG_SIZE);
        assert!(end <= DLOG_SIZE);
        assert!(start < end);
        self.buf[start..end].copy_from_slice(data);
    }

    #[cfg(test)]
    pub fn get_head(&self) -> usize {
        self.head & DLOG_MASK
    }

    #[cfg(test)]
    pub fn get_tail(&self) -> usize {
        self.tail & DLOG_MASK
    }

    #[cfg(test)]
    pub fn check(&self, position: usize, value: u8) -> bool {
        assert!(position < DLOG_SIZE);
        assert_eq!(self.buf[position], value);
        self.buf[position] == value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_cover1() {
        let mut buffer = DlogBuffer::new();
        buffer.write(0u32, &[127u8; 100], 0, 0);
        let head = buffer.get_head();
        assert_eq!(head, 132usize);
        let tail = buffer.get_tail();
        assert_eq!(tail, 0usize);
        buffer.write(0u32, &[255u8; 2000], 0, 0);
        let head = buffer.get_head();
        assert_eq!(head, 116usize);
        let tail = buffer.get_tail();
        assert_eq!(tail, 132usize);
    }

    #[test]
    fn buffer_cover2() {
        let mut buffer = DlogBuffer::new();
        buffer.write(0u32, &[127u8; 2000], 0, 0);
        for i in 32..2032 {
            assert!(buffer.check(i, 127u8));
        }
        let head = buffer.get_head();
        assert_eq!(head, 2032usize);
        let tail = buffer.get_tail();
        assert_eq!(tail, 0usize);
        buffer.write(0u32, &[255u8; 101], 0, 0);
        for i in 16..117 {
            assert!(buffer.check(i, 255u8));
        }
        for i in 117..2032 {
            assert!(buffer.check(i, 127u8));
        }
        let head = buffer.get_head();
        assert_eq!(head, 120usize);
        let tail = buffer.get_tail();
        assert_eq!(tail, 2032usize);
    }

    #[test]
    fn buffer_cover3() {
        let mut buffer = DlogBuffer::new();
        buffer.write(0u32, &[127u8; 1984], 0, 0);
        buffer.write(0xdead_beafu32, &[255u8; 101], 0, 0);
        for i in 0..101 {
            assert!(buffer.check(i, 255u8));
        }
        for i in 102..2016 {
            assert!(buffer.check(i, 127u8));
        }
        assert!(buffer.check(2022, 0xafu8));
        assert!(buffer.check(2023, 0xbeu8));
        let head = buffer.get_head();
        assert_eq!(head, 104usize);
        let tail = buffer.get_tail();
        assert_eq!(tail, 2016usize);
    }
}
