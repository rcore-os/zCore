use {
    super::*,
    crate::hal::{serial_write, timer_now},
    crate::object::*,
    alloc::sync::Arc,
    serde::{Deserialize, Serialize},
    spin::Mutex,
};

static DLOG: Mutex<DlogBuffer> = Mutex::new(DlogBuffer::new());

#[cfg(not(test))]
const DLOG_SIZE: usize = 128usize * 1024usize;
#[cfg(test)]
const DLOG_SIZE: usize = 2usize * 1024usize;
const DLOG_MASK: usize = DLOG_SIZE - 1;
const DLOG_MIN_RECORD: usize = 32usize;

#[derive(Serialize, Deserialize)]
#[repr(C)]
struct DlogHeader {
    pub header: u32,
    pub datalen: u16,
    pub flags: u16,
    pub timestamp: u64,
    pub pid: u64,
    pub tid: u64,
}

pub struct DebugLog {
    base: KObjectBase,
    #[allow(dead_code)]
    flags: u32,
}

impl_kobject!(DebugLog);

impl DebugLog {
    pub fn create(flags: u32) -> ZxResult<Arc<Self>> {
        let dlog = Arc::new(DebugLog {
            base: KObjectBase::new(),
            flags,
        });
        Ok(dlog)
    }

    pub fn write(&self, flags: u32, data: &str) -> ZxResult<usize> {
        let flags = flags | self.flags;
        DLOG.lock().write(flags, data.as_bytes());
        serial_write(data);
        serial_write("\n");
        Ok(0)
    }
}

#[allow(dead_code)]
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

    pub fn write(&mut self, flags: u32, data: &[u8]) {
        let wire_size = DLOG_MIN_RECORD + ((data.len() + 3) & !3);
        let header = (((DLOG_MIN_RECORD + data.len()) as u32 & 0xFFFu32) << 12)
            | (wire_size as u32 & 0xFFFu32);
        let serde_header = bincode::serialize(&DlogHeader {
            header,
            datalen: data.len() as u16,
            flags: flags as u16,
            timestamp: timer_now().as_nanos() as u64,
            pid: 0u64,
            tid: 0u64,
        })
        .unwrap();
        let head = self.head;
        while (head - self.tail) > (DLOG_SIZE - wire_size) {
            let tail_index = self.tail & DLOG_MASK;
            let header: u32 =
                bincode::deserialize::<u32>(&self.buf[tail_index..tail_index + 4]).unwrap();
            self.tail += (header & 0xFFF) as usize;
        }
        let mut offset = head & DLOG_MASK;
        let fifo_size = DLOG_SIZE - offset;
        if fifo_size >= wire_size {
            self.copy_and_write(offset, offset + DLOG_MIN_RECORD, serde_header.as_slice());
            self.copy_and_write(
                offset + DLOG_MIN_RECORD,
                offset + DLOG_MIN_RECORD + data.len(),
                data,
            );
        } else if fifo_size < DLOG_MIN_RECORD {
            self.copy_and_write(offset, DLOG_SIZE, &serde_header[..fifo_size]);
            self.copy_and_write(0, DLOG_MIN_RECORD - fifo_size, &serde_header[fifo_size..]);
            self.copy_and_write(
                DLOG_MIN_RECORD - fifo_size,
                DLOG_MIN_RECORD - fifo_size + data.len(),
                data,
            );
        } else {
            self.copy_and_write(offset, offset + DLOG_MIN_RECORD, serde_header.as_slice());
            offset += DLOG_MIN_RECORD;
            if offset < DLOG_SIZE {
                let fifo_size = DLOG_SIZE - offset;
                self.copy_and_write(offset, DLOG_SIZE, &data[..fifo_size]);
                self.copy_and_write(0, data.len() - fifo_size, &data[fifo_size..]);
            } else {
                self.copy_and_write(0, data.len(), data);
            }
        }
        self.head += wire_size;
    }

    fn copy_and_write(&mut self, start: usize, end: usize, data: &[u8]) {
        assert!(start < DLOG_SIZE);
        assert!(end <= DLOG_SIZE);
        assert!(start < end);
        assert_eq!(end - start, data.len());
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
    pub fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
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
        DLOG.lock().clear();
        let mut buffer = DLOG.lock();
        buffer.write(0u32, &[127u8; 100]);
        let head = buffer.get_head();
        assert_eq!(head, 132usize);
        let tail = buffer.get_tail();
        assert_eq!(tail, 0usize);
        buffer.write(0u32, &[255u8; 2000]);
        let head = buffer.get_head();
        assert_eq!(head, 116usize);
        let tail = buffer.get_tail();
        assert_eq!(tail, 132usize);
    }

    #[test]
    fn buffer_cover2() {
        DLOG.lock().clear();
        let mut buffer = DLOG.lock();
        buffer.write(0u32, &[127u8; 2000]);
        for i in 32..2032 {
            assert!(buffer.check(i, 127u8));
        }
        let head = buffer.get_head();
        assert_eq!(head, 2032usize);
        let tail = buffer.get_tail();
        assert_eq!(tail, 0usize);
        buffer.write(0u32, &[255u8; 101]);
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
        DLOG.lock().clear();
        let mut buffer = DLOG.lock();
        buffer.write(0u32, &[127u8; 1984]);
        buffer.write(0xdead_beafu32, &[255u8; 101]);
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
