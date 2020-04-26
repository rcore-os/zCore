use {super::*, alloc::vec::Vec, zircon_object::ipc::Fifo};

impl Syscall<'_> {
    pub fn sys_fifo_create(
        &self,
        elem_count: usize,
        elem_size: usize,
        options: u32,
        mut out0: UserOutPtr<HandleValue>,
        mut out1: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "fifo.create: count={:#x}, item_size={:#x}, options={:#x}",
            elem_count, elem_size, options,
        );
        if options != 0 {
            return Err(ZxError::OUT_OF_RANGE);
            // return Err(ZxError::INVALID_ARGS); // MARK?
        }
        if !elem_count.is_power_of_two() || elem_size == 0 || elem_count * elem_size > 4096 {
            return Err(ZxError::OUT_OF_RANGE);
        }
        let (end0, end1) = Fifo::create(elem_count, elem_size);
        let proc = self.thread.proc();
        let handle0 = proc.add_handle(Handle::new(end0, Rights::DEFAULT_FIFO));
        let handle1 = proc.add_handle(Handle::new(end1, Rights::DEFAULT_FIFO));
        out0.write(handle0)?;
        out1.write(handle1)?;
        Ok(())
    }

    pub fn sys_fifo_write(
        &self,
        handle_value: HandleValue,
        elem_size: usize,
        user_bytes: UserInPtr<u8>,
        count: usize,
        mut actual_count: UserOutPtr<usize>,
    ) -> ZxResult {
        let proc = self.thread.proc();
        let fifo = proc.get_object_with_rights::<Fifo>(handle_value, Rights::WRITE)?;
        if count == 0 {
            return Err(ZxError::OUT_OF_RANGE);
        }
        let data = user_bytes.read_array(count * elem_size)?;
        let mut actual: usize = 0;
        fifo.write(elem_size, data, count, &mut actual)?;
        actual_count.write_if_not_null(actual / elem_size)?;
        Ok(())
    }

    pub fn sys_fifo_read(
        &self,
        handle_value: HandleValue,
        elem_size: usize,
        mut user_bytes: UserOutPtr<u8>,
        count: usize,
        mut actual_count: UserOutPtr<usize>,
    ) -> ZxResult {
        let proc = self.thread.proc();
        let fifo = proc.get_object_with_rights::<Fifo>(handle_value, Rights::WRITE)?;
        if count == 0 {
            return Err(ZxError::OUT_OF_RANGE);
        }
        let mut actual: usize = 0;
        let elem_vec: Vec<u8> = fifo.read(elem_size, count, &mut actual)?;
        actual_count.write_if_not_null(actual / elem_size)?;
        user_bytes.write_array(elem_vec.as_slice())?;
        Ok(())
    }
}
