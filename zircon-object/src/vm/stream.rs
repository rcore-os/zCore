use {
    super::*, crate::object::*, alloc::sync::Arc, bitflags::bitflags, kernel_hal::user_io_vec::*,
    numeric_enum_macro::numeric_enum, spin::Mutex,
};

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

numeric_enum! {
    #[repr(usize)]
    #[derive(Debug)]
    pub enum SeekOrigin {
        Start = 0,
        Current = 1,
        End = 2,
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
        let mut rights = Rights::DEFAULT_STREAM;
        if options.contains(StreamOptions::STREAM_MODE_READ) {
            rights |= Rights::READ;
            if !vmo_rights.contains(Rights::READ) {
                return Err(ZxError::ACCESS_DENIED);
            }
        }
        if options.contains(StreamOptions::STREAM_MODE_WRITE) {
            rights |= Rights::WRITE;
            if !vmo_rights.contains(Rights::WRITE) {
                return Err(ZxError::ACCESS_DENIED);
            }
        }
        let out = Arc::new(Stream {
            base: KObjectBase::with_signal(Signal::empty()), // it seems that stream don't care signals
            options,
            vmo,
            seek: Mutex::new(seek),
        });
        Ok((out, rights))
    }

    /// Read data from the stream at the current seek offset
    pub fn read(&self, data: &OutIoVec<u8>) -> ZxResult<usize> {
        let count = data.len();
        let content_size = self.vmo.content_size();
        let mut seek = self.seek.lock();
        if *seek >= content_size {
            return Ok(0);
        }
        let offset = *seek;
        let length = count.min(content_size - offset);
        *seek += length;
        let slice = data.as_mut_slice()?;
        self.vmo.read(offset, &mut slice[..length])?;
        Ok(length)
    }

    /// Read data from the stream at a given offset
    pub fn read_at(&self, data: &OutIoVec<u8>, offset: usize) -> ZxResult<usize> {
        let count = data.len();
        let content_size = self.vmo.content_size();
        if offset >= content_size {
            return Ok(0);
        }
        let length = count.min(content_size - offset);
        let slice = data.as_mut_slice()?;
        self.vmo.read(offset, &mut slice[..length])?;
        Ok(length)
    }

    /// write data to the stream at the current seek offset or append data at the end of content
    pub fn write(&self, data: &InIoVec<u8>, append: bool) -> ZxResult<usize> {
        let count = data.len();
        let mut content_size = self.vmo.content_size();
        let mut seek = self.seek.lock();
        if append {
            *seek = content_size;
        }
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
        let slice = data.as_slice()?;
        self.vmo.write(offset, &slice[..length])?;
        Ok(length)
    }

    /// Write data to the stream at a given offset
    pub fn write_at(&self, data: &InIoVec<u8>, offset: usize) -> ZxResult<usize> {
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
        let slice = data.as_slice()?;
        self.vmo.write(offset, &slice[..length])?;
        Ok(length)
    }

    /// Modify the current seek offset of the stream
    pub fn seek(&self, seek_origin: SeekOrigin, offset: isize) -> ZxResult<usize> {
        let mut seek = self.seek.lock();
        let origin: usize = match seek_origin {
            SeekOrigin::Start => 0,
            SeekOrigin::Current => *seek,
            SeekOrigin::End => self.vmo.content_size(),
        };
        if offset >= 0 {
            let (target, overflow) = origin.overflowing_add(offset as usize);
            if overflow {
                return Err(ZxError::INVALID_ARGS);
            }
            *seek = target;
        } else {
            let target = origin as isize + offset;
            if origin as isize >= 0 && target < 0 {
                return Err(ZxError::INVALID_ARGS);
            }
            *seek = target as usize;
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
    // The options passed to zx_stream_create().
    options: u32,
    padding1: u32,
    // The current seek offset.
    //
    // Used by zx_stream_readv and zx_stream_writev to determine where to read
    // and write the stream.
    seek: u64,
    // The current size of the stream.
    //
    // The number of bytes in the stream that store data. The stream itself
    // might have a larger capacity to avoid reallocating the underlying storage
    // as the stream grows or shrinks.
    // NOTE: in fact, this value is store in the VmObject associated and can be
    // get/set through 'object_[get/set]_property(vmo_handle, ...)'
    content_size: u64,
}
