use {super::*, bitflags::bitflags, zircon_object::vm::*};

impl Syscall<'_> {
    /// Create a stream from a VMO.    
    ///   
    /// Stream for reads and writes the data in an underlying VMO.  
    pub fn sys_stream_create(
        &self,
        options: u32,
        vmo_handle: HandleValue,
        seek: usize,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "stream.create: options={:#x?}, vmo_handle={:#x?}, seek={:#x?}",
            options, vmo_handle, seek
        );
        bitflags! {
            struct CreateOptions: u32 {
                #[allow(clippy::identity_op)]
                const MODE_READ     = 1 << 0;
                const MODE_WRITE    = 1 << 1;
            }
        }
        let options = CreateOptions::from_bits(options).ok_or(ZxError::INVALID_ARGS)?;
        let mut rights = Rights::DEFAULT_STREAM;
        let mut vmo_rights = Rights::empty();
        if options.contains(CreateOptions::MODE_READ) {
            rights |= Rights::READ;
            vmo_rights |= Rights::READ;
        }
        if options.contains(CreateOptions::MODE_WRITE) {
            rights |= Rights::WRITE;
            vmo_rights |= Rights::WRITE;
        }
        let proc = self.thread.proc();
        let vmo = proc.get_object_with_rights::<VmObject>(vmo_handle, vmo_rights)?;
        let stream = Stream::create(vmo, seek, options.bits());
        let handle = proc.add_handle(Handle::new(stream, rights));
        out.write(handle)?;
        Ok(())
    }

    /// Write data to a stream at the current seek offset.   
    pub fn sys_stream_writev(
        &self,
        handle_value: HandleValue,
        options: u32,
        vector: UserInPtr<IoVecIn>,
        vector_size: usize,
        mut actual_count_ptr: UserOutPtr<usize>,
    ) -> ZxResult {
        info!(
            "stream.write: stream={:#x?}, options={:#x?}, vector=({:#x?}; {:#x?})",
            handle_value, options, vector, vector_size,
        );
        bitflags! {
            struct WriteOptions: u32 {
                const APPEND = 1;
            }
        }
        let data = vector.read_iovecs(vector_size)?;
        let options = WriteOptions::from_bits(options).ok_or(ZxError::INVALID_ARGS)?;
        let proc = self.thread.proc();
        let stream = proc.get_object_with_rights::<Stream>(handle_value, Rights::WRITE)?;
        let mut actual_count = 0;
        for io_vec in data.iter() {
            actual_count +=
                stream.write(io_vec.as_slice()?, options.contains(WriteOptions::APPEND))?;
        }
        actual_count_ptr.write_if_not_null(actual_count)?;
        Ok(())
    }

    /// Write data to a stream at the given offset.   
    pub fn sys_stream_writev_at(
        &self,
        handle_value: HandleValue,
        options: u32,
        mut offset: usize,
        vector: UserInPtr<IoVecIn>,
        vector_size: usize,
        mut actual_count_ptr: UserOutPtr<usize>,
    ) -> ZxResult {
        info!(
            "stream.write_at: stream={:#x?}, options={:#x?}, offset={:#x?}, vector=({:#x?}; {:#x?})",
            handle_value, options, offset, vector, vector_size,
        );
        if options != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let data = vector.read_iovecs(vector_size)?;
        let proc = self.thread.proc();
        let stream = proc.get_object_with_rights::<Stream>(handle_value, Rights::WRITE)?;
        let mut actual_count = 0;
        for io_vec in data.iter() {
            actual_count += stream.write_at(io_vec.as_slice()?, offset)?;
            offset += actual_count;
        }
        actual_count_ptr.write_if_not_null(actual_count)?;
        Ok(())
    }

    /// Read data from a stream at the current seek offset.   
    pub fn sys_stream_readv(
        &self,
        handle_value: HandleValue,
        options: u32,
        vector: UserInPtr<IoVecOut>,
        vector_size: usize,
        mut actual_count_ptr: UserOutPtr<usize>,
    ) -> ZxResult {
        info!(
            "stream.read: stream={:#x?}, options={:#x?}, vector=({:#x?}; {:#x?})",
            handle_value, options, vector, vector_size,
        );
        if options != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut data = vector.read_iovecs(vector_size)?;
        let proc = self.thread.proc();
        let stream = proc.get_object_with_rights::<Stream>(handle_value, Rights::READ)?;
        let mut actual_count = 0usize;
        for io_vec in data.iter_mut() {
            actual_count += stream.read(io_vec.as_mut_slice()?)?;
        }
        actual_count_ptr.write_if_not_null(actual_count)?;
        Ok(())
    }

    /// Read data from a stream at the given offset.   
    pub fn sys_stream_readv_at(
        &self,
        handle_value: HandleValue,
        options: u32,
        mut offset: usize,
        vector: UserInPtr<IoVecOut>,
        vector_size: usize,
        mut actual_count_ptr: UserOutPtr<usize>,
    ) -> ZxResult {
        info!(
            "stream.read_at: stream={:#x?}, options={:#x?}, offset={:#x?}, vector=({:#x?}; {:#x?})",
            handle_value, options, offset, vector, vector_size,
        );
        if options != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut data = vector.read_iovecs(vector_size)?;
        let proc = self.thread.proc();
        let stream = proc.get_object_with_rights::<Stream>(handle_value, Rights::READ)?;
        let mut actual_count = 0usize;
        for io_vec in data.iter_mut() {
            actual_count += stream.read_at(io_vec.as_mut_slice()?, offset)?;
            offset += actual_count;
        }
        actual_count_ptr.write_if_not_null(actual_count)?;
        Ok(())
    }

    /// Modify the seek offset.  
    ///   
    /// Sets the seek offset of the stream to `offset` relative to `whence`.  
    pub fn sys_stream_seek(
        &self,
        handle_value: HandleValue,
        whence: usize,
        offset: isize,
        mut out_seek: UserOutPtr<usize>,
    ) -> ZxResult {
        info!(
            "stream.seek: stream={:#x?}, whence={:#x?}, offset={:#x?}",
            handle_value, whence, offset,
        );
        let proc = self.thread.proc();
        let (stream, rights) = proc.get_object_and_rights::<Stream>(handle_value)?;
        if !rights.contains(Rights::READ) && !rights.contains(Rights::WRITE) {
            return Err(ZxError::ACCESS_DENIED);
        }
        let whence = SeekOrigin::try_from(whence).map_err(|_| ZxError::INVALID_ARGS)?;
        let new_seek = stream.seek(whence, offset)?;
        out_seek.write_if_not_null(new_seek)?;
        Ok(())
    }
}
