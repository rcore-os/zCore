use {super::*, crate::object::*, alloc::sync::Arc, bitflags::bitflags, core::slice, spin::Mutex};

/// A readable, writable, seekable interface to some underlying storage
///
/// ## SYNOPSIS
///
/// A stream is an interface for reading and writing data to some underlying
/// storage, typically a VMO.
pub struct Stream {
    base: KObjectBase,
    options: StreamOptions,
    vmo: Arc<VmObject>,
    seek: Mutex<usize>,
}

impl_kobject!(Stream);

bitflags! {
    #[derive(Default)]
    pub struct StreamOptions: u32 {
        #[allow(clippy::identity_op)]
        // These can be passed to stream_create()
        const STREAM_MODE_READ           = 1;
        const STREAM_MODE_WRITE          = 1 << 1;
        const STREAM_CREATE_MASK         = Self::STREAM_MODE_READ.bits | Self::STREAM_MODE_WRITE.bits;

        // These can be passed to stream_writev()
        const STREAM_APPEND              = 1;
    }
}

pub enum SeekOrigin {
    SeekOriginStart,
    SeekOriginCurrent,
    SeekOriginEnd,
    InvalidValue,
}

impl From<usize> for SeekOrigin {
    fn from(n: usize) -> Self {
        match n {
            0 => SeekOrigin::SeekOriginStart,
            1 => SeekOrigin::SeekOriginCurrent,
            2 => SeekOrigin::SeekOriginEnd,
            _ => SeekOrigin::InvalidValue,
        }
    }
}

impl Stream {
    /// Create a stream from a VMO
    pub fn create(
        options: u32,
        vmo: Arc<VmObject>,
        vmo_rights: Rights,
        seek: usize,
    ) -> ZxResult<(Arc<Self>, Rights)> {
        let options = StreamOptions::from_bits(options).ok_or(ZxError::INVALID_ARGS)?;
        if !(options - StreamOptions::STREAM_CREATE_MASK).is_empty() {
            return Err(ZxError::INVALID_ARGS);
        }
        let start_signals = Signal::empty();
        let mut rights = Rights::DEFAULT_STREAM;
        if options.contains(StreamOptions::STREAM_MODE_READ) {
            rights |= Rights::READ;
            if !vmo_rights.contains(Rights::READ) {
                return Err(ZxError::ACCESS_DENIED);
            }
            // start_signals |= Signals::READABLE;
        }
        if options.contains(StreamOptions::STREAM_MODE_WRITE) {
            rights |= Rights::WRITE;
            if !vmo_rights.contains(Rights::WRITE) {
                return Err(ZxError::ACCESS_DENIED);
            }
            // start_signals |= Signals::WRITABLE;
        }
        let out = Arc::new(Stream {
            base: KObjectBase::with_signal(start_signals),
            options,
            vmo,
            seek: Mutex::new(seek),
        });
        Ok((out, rights))
    }

    /// Read data from the stream at the current seek offset
    #[allow(unsafe_code)]
    pub fn read(&self, data: &mut [u8]) -> ZxResult<usize> {
        let count = data.len();
        let content_size = self.vmo.content_size();
        let mut seek = self.seek.lock();
        if *seek >= content_size {
            return Ok(0);
        }
        let offset = *seek;
        let length = count.min(content_size - offset);
        *seek += length;
        // unsafe code can be avoided by adding more interface to VmObject and HAL
        let io_vec = unsafe { slice::from_raw_parts_mut(data.as_mut_ptr(), length) };
        self.vmo.read(offset, io_vec)?;
        Ok(length)
    }

    /// Read data from the stream at a given offset
    #[allow(unsafe_code)]
    pub fn read_at(&self, data: &mut [u8], offset: usize) -> ZxResult<usize> {
        let count = data.len();
        let content_size = self.vmo.content_size();
        if offset >= content_size {
            return Ok(0);
        }
        let length = count.min(content_size - offset);
        // unsafe code can be avoided by adding more interface to VmObject and HAL
        let io_vec = unsafe { slice::from_raw_parts_mut(data.as_mut_ptr(), length) };
        self.vmo.read(offset, io_vec)?;
        Ok(length)
    }

    /// write data to the stream at the current seek offset or append data at the end of content
    pub fn write(&self, options: StreamOptions, data: &[u8]) -> ZxResult<usize> {
        if options.contains(StreamOptions::STREAM_APPEND) {
            self.write_append(data)
        } else {
            self.write_seek(data)
        }
    }

    #[allow(unsafe_code)]
    fn write_seek(&self, data: &[u8]) -> ZxResult<usize> {
        let count = data.len();
        let mut content_size = self.vmo.content_size();
        let mut seek = self.seek.lock();
        let (target_size, overflow) = seek.overflowing_add(count);
        if overflow {
            return Err(ZxError::FILE_BIG);
        }
        if target_size > content_size {
            content_size = self.vmo.set_content_size_and_resize(target_size, *seek)?;
        }
        if *seek >= content_size {
            return Err(ZxError::NO_SPACE);
        }
        let offset = *seek;
        let length = count.min(content_size - offset);
        *seek += length;
        // unsafe code can be avoided by adding more interface to VmObject and HAL
        let io_vec = unsafe { slice::from_raw_parts(data.as_ptr(), length) };
        self.vmo.write(offset, io_vec)?;
        Ok(length)
    }

    #[allow(unsafe_code)]
    fn write_append(&self, data: &[u8]) -> ZxResult<usize> {
        let count = data.len();
        let content_size = self.vmo.content_size();
        let mut seek = self.seek.lock();
        let (target_size, overflow) = content_size.overflowing_add(count);
        if overflow {
            return Err(ZxError::FILE_BIG);
        }
        let new_content_size = self
            .vmo
            .set_content_size_and_resize(target_size, content_size)?;
        if new_content_size <= content_size {
            return Err(ZxError::NO_SPACE);
        }
        let offset = content_size;
        let length = count.min(new_content_size - offset);
        *seek = content_size + length;
        // unsafe code can be avoided by adding more interface to VmObject and HAL
        let io_vec = unsafe { slice::from_raw_parts(data.as_ptr(), length) };
        self.vmo.write(offset, io_vec)?;
        Ok(length)
    }

    /// Write data to the stream at a given offset
    #[allow(unsafe_code)]
    pub fn write_at(&self, data: &[u8], offset: usize) -> ZxResult<usize> {
        let count = data.len();
        let mut content_size = self.vmo.content_size();
        let (target_size, overflow) = offset.overflowing_add(count);
        if overflow {
            return Err(ZxError::FILE_BIG);
        }
        if target_size > content_size {
            content_size = self.vmo.set_content_size_and_resize(target_size, offset)?;
        }
        if offset >= content_size {
            return Err(ZxError::NO_SPACE);
        }
        let length = count.min(content_size - offset);
        self.vmo.write(offset, data)?;
        // unsafe code can be avoided by adding more interface to VmObject and HAL
        let io_vec = unsafe { slice::from_raw_parts(data.as_ptr(), length) };
        self.vmo.write(offset, io_vec)?;
        Ok(length)
    }

    /// Modify the current seek offset of the stream
    pub fn seek(&self, seek_origin: SeekOrigin, offset: isize) -> ZxResult<usize> {
        let mut seek = self.seek.lock();
        match seek_origin {
            SeekOrigin::SeekOriginStart => {
                if offset < 0 {
                    return Err(ZxError::INVALID_ARGS);
                }
                *seek = offset as usize;
            }
            SeekOrigin::SeekOriginCurrent => {
                if offset >= 0 {
                    let (target, overflow) = seek.overflowing_add(offset as usize);
                    if overflow {
                        return Err(ZxError::INVALID_ARGS);
                    }
                    *seek = target;
                } else {
                    let target = *seek as isize + offset;
                    if *seek as isize > 0 && target < 0 {
                        return Err(ZxError::INVALID_ARGS);
                    }
                    *seek = target as usize;
                }
            }
            SeekOrigin::SeekOriginEnd => {
                let content_size = self.vmo.content_size();
                if offset >= 0 {
                    let (target, overflow) = content_size.overflowing_add(offset as usize);
                    if overflow {
                        return Err(ZxError::INVALID_ARGS);
                    }
                    *seek = target;
                } else {
                    let target = content_size as isize + offset;
                    if content_size as isize > 0 && target < 0 {
                        return Err(ZxError::INVALID_ARGS);
                    }
                    *seek = target as usize;
                }
            }
            _ => unreachable!(),
        }
        Ok(*seek)
    }

    /// Get information of the socket.
    pub fn get_info(&self) -> StreamInfo {
        let seek = self.seek.lock();
        StreamInfo {
            options: self.options.bits,
            padding1: 0,
            seek: *seek as u64,
            content_size: self.vmo.content_size() as u64,
        }
    }
}

#[repr(C)]
#[derive(Default)]
pub struct StreamInfo {
    options: u32,
    padding1: u32,
    seek: u64,
    content_size: u64,
}
