use {
    super::*,
    crate::object::*,
    alloc::sync::Arc,
    core::sync::atomic::{AtomicU32, Ordering},
    kernel_hal::sync::Mutex,
    numeric_enum_macro::numeric_enum,
};

/// A readable, writable, seekable interface to some underlying storage
///
/// ## SYNOPSIS
///
/// A stream is an interface for reading and writing data to some underlying
/// storage, typically a VMO.
pub struct Stream {
    base: KObjectBase,
    options: AtomicU32,
    vmo: Arc<VmObject>,
    seek: Mutex<usize>,
}

bitflags::bitflags! {
    pub struct StreamOptions: u32 {
        const MODE_READ = 1 << 0;
        const MODE_WRITE = 1 << 1;
        const MODE_APPEND = 1 << 2;
    }
}

impl_kobject!(Stream);

numeric_enum! {
    #[repr(usize)]
    #[derive(Debug)]
    /// Enumeration of possible methods to modify the seek within an Stream.
    pub enum SeekOrigin {
        /// Set the seek offset relative to the start of the stream.
        Start = 0,
        /// Set the seek offset relative to the current seek offset of the stream.
        Current = 1,
        /// Set the seek offset relative to the end of the stream, as defined by the content size of the stream.
        End = 2,
    }
}

impl Stream {
    /// Create a stream from a VMO
    pub fn create(vmo: Arc<VmObject>, seek: usize, options: u32) -> Arc<Self> {
        Arc::new(Stream {
            base: KObjectBase::default(),
            options: AtomicU32::new(options),
            vmo,
            seek: Mutex::new(seek),
        })
    }

    /// Read data from the stream at the current seek offset
    pub fn read(&self, data: &mut [u8]) -> ZxResult<usize> {
        let mut seek = self.seek.lock();
        let length = self.read_at(data, *seek)?;
        *seek += length;
        Ok(length)
    }

    /// Read data from the stream at a given offset
    pub fn read_at(&self, data: &mut [u8], offset: usize) -> ZxResult<usize> {
        let count = data.len();
        let content_size = self.vmo.content_size();
        if offset >= content_size {
            return Ok(0);
        }
        let length = count.min(content_size - offset);
        self.vmo.read(offset, &mut data[..length])?;
        Ok(length)
    }

    /// write data to the stream at the current seek offset or append data at the end of content
    pub fn write(&self, data: &[u8], append: bool) -> ZxResult<usize> {
        let mut seek = self.seek.lock();
        if data.is_empty() {
            return Ok(0);
        }
        let offset = if append || self.append_mode() {
            None
        } else {
            Some(*seek)
        };
        let (offset, length) = self.vmo.write_stream(offset, data)?;
        *seek = offset + length;
        Ok(length)
    }

    /// Validate the size calculation before touching any user buffers.
    pub fn check_write_size(&self, count: usize, append: bool, offset: Option<usize>) -> ZxResult {
        if count == 0 {
            return Ok(());
        }
        let append = offset.is_none() && (append || self.append_mode());
        let offset = match offset {
            Some(offset) => offset,
            None if append => self.vmo.content_size(),
            None => *self.seek.lock(),
        };
        offset.checked_add(count).ok_or(if append {
            ZxError::OUT_OF_RANGE
        } else {
            ZxError::FILE_BIG
        })?;
        Ok(())
    }

    /// Write data to the stream at a given offset
    pub fn write_at(&self, data: &[u8], offset: usize) -> ZxResult<usize> {
        self.vmo
            .write_stream(Some(offset), data)
            .map(|(_, length)| length)
    }

    /// Modify the current seek offset of the stream
    pub fn seek(&self, whence: SeekOrigin, offset: isize) -> ZxResult<usize> {
        let mut seek = self.seek.lock();
        let origin: usize = match whence {
            SeekOrigin::Start => 0,
            SeekOrigin::Current => *seek,
            SeekOrigin::End => self.vmo.content_size(),
        };
        *seek = if offset >= 0 {
            origin
                .checked_add(offset as usize)
                .ok_or(ZxError::INVALID_ARGS)?
        } else {
            origin
                .checked_sub(offset.unsigned_abs())
                .ok_or(ZxError::INVALID_ARGS)?
        };
        Ok(*seek)
    }

    /// Return whether writes without an explicit option append to the VMO.
    pub fn append_mode(&self) -> bool {
        self.options.load(Ordering::Relaxed) & StreamOptions::MODE_APPEND.bits() != 0
    }

    /// Change the persistent append mode of this stream.
    pub fn set_append_mode(&self, append: bool) {
        if append {
            self.options
                .fetch_or(StreamOptions::MODE_APPEND.bits(), Ordering::Relaxed);
        } else {
            self.options
                .fetch_and(!StreamOptions::MODE_APPEND.bits(), Ordering::Relaxed);
        }
    }

    /// Get information of the socket.
    pub fn get_info(&self) -> StreamInfo {
        let seek = self.seek.lock();
        StreamInfo {
            options: self.options.load(Ordering::Relaxed),
            padding1: 0,
            seek: *seek as u64,
            content_size: self.vmo.content_size() as u64,
        }
    }
}

/// Information of a Stream
#[repr(C)]
#[derive(Default)]
pub struct StreamInfo {
    /// The options passed to `Stream::create()`.
    options: u32,
    padding1: u32,
    /// The current seek offset.
    ///
    /// Used by stream_readv and stream_writev to determine where to read
    /// and write the stream.
    seek: u64,
    /// The current size of the stream.
    ///
    /// The number of bytes in the stream that store data. The stream itself
    /// might have a larger capacity to avoid reallocating the underlying storage
    /// as the stream grows or shrinks.
    /// NOTE: in fact, this value is store in the VmObject associated and can be
    /// get/set through 'object_[get/set]_property(vmo_handle, ...)'
    content_size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[async_std::test]
    async fn separate_streams_append_without_overwriting() {
        let vmo = VmObject::new_paged(1);
        vmo.set_content_size(0).unwrap();
        let mut writers = alloc::vec::Vec::new();
        for id in 0..4u8 {
            let stream = Stream::create(vmo.clone(), 0, StreamOptions::MODE_APPEND.bits());
            writers.push(async_std::task::spawn_blocking(move || {
                for _ in 0..256 {
                    assert_eq!(stream.write(&[id], false).unwrap(), 1);
                }
            }));
        }
        for writer in writers {
            writer.await;
        }
        assert_eq!(vmo.content_size(), 1024);
        let mut data = [0u8; 1024];
        vmo.read(0, &mut data).unwrap();
        for id in 0..4u8 {
            assert_eq!(data.iter().filter(|byte| **byte == id).count(), 256);
        }
    }
}
