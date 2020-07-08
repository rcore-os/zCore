use {super::*, zircon_object::vm::*};

impl Syscall<'_> {
    pub fn sys_stream_create(
        &self,
        options: u32,
        vmo_handle: HandleValue,
        seek: usize,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "stream.create: options={:#x?}, vmo_handle = {:#x?}, seek = {:#x?}",
            options, vmo_handle, seek
        );
        let proc = self.thread.proc();
        let (vmo, vmo_rights) = proc.get_object_and_rights::<VmObject>(vmo_handle)?;
        let (stream, rights) = Stream::create(options, vmo, vmo_rights, seek)?;
        let proc = self.thread.proc();
        let handle = proc.add_handle(Handle::new(stream, rights));
        out.write(handle)?;
        Ok(())
    }

    pub fn sys_stream_writev(
        &self,
        handle_value: HandleValue,
        options: u32,
        user_bytes: UserInPtr<InIoVec<u8>>,
        count: usize,
        mut actual_count_ptr: UserOutPtr<usize>,
    ) -> ZxResult {
        info!(
            "stream.write: stream={:#x?}, options={:#x?}, buffer={:#x?}, size={:#x?}",
            handle_value, options, user_bytes, count,
        );
        let options = StreamOptions::from_bits(options).ok_or(ZxError::INVALID_ARGS)?;
        if !(options - StreamOptions::STREAM_APPEND).is_empty() {
            return Err(ZxError::INVALID_ARGS);
        }
        if user_bytes.is_null() {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        let stream = proc.get_object_with_rights::<Stream>(handle_value, Rights::WRITE)?;
        let data = user_bytes.read_array(count)?;
        check_total_capacity(&data)?;
        let mut actual_count = 0;
        for io_vec in &data {
            if io_vec.is_null() {
                return Err(ZxError::NOT_FOUND);
            }
            actual_count += stream.write(io_vec, options.contains(StreamOptions::STREAM_APPEND))?;
        }
        actual_count_ptr.write_if_not_null(actual_count)?;
        Ok(())
    }

    pub fn sys_stream_writev_at(
        &self,
        handle_value: HandleValue,
        options: u32,
        offset: usize,
        user_bytes: UserInPtr<InIoVec<u8>>,
        count: usize,
        mut actual_count_ptr: UserOutPtr<usize>,
    ) -> ZxResult {
        info!(
            "stream.write_at: stream={:#x?}, options={:#x?}, offset = {:#x?}, buffer={:#x?}, size={:#x?}",
            handle_value, options, offset, user_bytes, count,
        );
        if options != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        if user_bytes.is_null() {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        let stream = proc.get_object_with_rights::<Stream>(handle_value, Rights::WRITE)?;
        let data = user_bytes.read_array(count)?;
        check_total_capacity(&data)?;
        let mut actual_count = 0;
        let mut off = offset;
        for io_vec in &data {
            if io_vec.is_null() {
                return Err(ZxError::NOT_FOUND);
            }
            actual_count += stream.write_at(io_vec, off)?;
            off += actual_count;
        }
        actual_count_ptr.write_if_not_null(actual_count)?;
        Ok(())
    }

    pub fn sys_stream_readv(
        &self,
        handle_value: HandleValue,
        options: u32,
        user_bytes: UserInOutPtr<OutIoVec<u8>>,
        count: usize,
        mut actual_count_ptr: UserOutPtr<usize>,
    ) -> ZxResult {
        info!(
            "stream.read: stream={:#x?}, options={:#x?}, buffer={:#x?}, size={:#x?}",
            handle_value, options, user_bytes, count,
        );
        if options != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        if user_bytes.is_null() {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut data = user_bytes.read_array(count)?;
        let proc = self.thread.proc();
        let stream = proc.get_object_with_rights::<Stream>(handle_value, Rights::READ)?;
        check_total_capacity(&data)?;
        let mut actual_count = 0usize;
        for io_vec in &mut data {
            if io_vec.is_null() {
                return Err(ZxError::NOT_FOUND);
            }
            actual_count += stream.read(io_vec)?;
        }
        actual_count_ptr.write_if_not_null(actual_count)?;
        Ok(())
    }

    pub fn sys_stream_readv_at(
        &self,
        handle_value: HandleValue,
        options: u32,
        offset: usize,
        user_bytes: UserInOutPtr<OutIoVec<u8>>,
        count: usize,
        mut actual_count_ptr: UserOutPtr<usize>,
    ) -> ZxResult {
        info!(
            "stream.read_at: stream={:#x?}, options={:#x?}, offset = {:#x?}, buffer={:#x?}, size={:#x?}",
            handle_value, options, offset, user_bytes, count,
        );
        if options != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        if user_bytes.is_null() {
            return Err(ZxError::INVALID_ARGS);
        }
        let mut data = user_bytes.read_array(count)?;
        let proc = self.thread.proc();
        let stream = proc.get_object_with_rights::<Stream>(handle_value, Rights::READ)?;
        check_total_capacity(&data)?;
        let mut actual_count = 0usize;
        let mut off = offset;
        for io_vec in &mut data {
            if io_vec.is_null() {
                return Err(ZxError::NOT_FOUND);
            }
            actual_count += stream.read_at(io_vec, off)?;
            off += actual_count;
        }
        actual_count_ptr.write_if_not_null(actual_count)?;
        Ok(())
    }

    pub fn sys_stream_seek(
        &self,
        handle_value: HandleValue,
        seek_origin: usize,
        offset: isize,
        mut out_seek: UserOutPtr<usize>,
    ) -> ZxResult {
        info!(
            "stream.seek: stream={:#x?}, seek_origin={:#x?}, offset = {:#x?}",
            handle_value, seek_origin, offset,
        );
        let proc = self.thread.proc();
        let (stream, rights) = proc.get_object_and_rights::<Stream>(handle_value)?;
        if !rights.contains(Rights::READ) && !rights.contains(Rights::WRITE) {
            return Err(ZxError::ACCESS_DENIED);
        }
        let seek_origin = SeekOrigin::try_from(seek_origin).or(Err(ZxError::INVALID_ARGS))?;
        let new_seek = stream.seek(seek_origin, offset)?;
        out_seek.write_if_not_null(new_seek)?;
        Ok(())
    }
}

fn check_total_capacity<T, P: Policy>(data: &[IoVec<T, P>]) -> ZxResult {
    let mut total_count = 0usize;
    for io_vec in data {
        let (result, overflow) = total_count.overflowing_add(io_vec.len());
        if overflow {
            return Err(ZxError::INVALID_ARGS);
        }
        total_count = result;
    }
    Ok(())
}
