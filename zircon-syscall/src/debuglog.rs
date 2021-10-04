use {
    super::*,
    zircon_object::{debuglog::*, dev::*},
};

impl Syscall<'_> {
    /// Create a kernel managed debuglog reader or writer.
    pub fn sys_debuglog_create(
        &self,
        rsrc: HandleValue,
        options: u32,
        mut target: UserOutPtr<HandleValue>,
    ) -> ZxResult {
        info!(
            "debuglog.create: resource_handle={:#x?}, options={:#x?}",
            rsrc, options,
        );
        let proc = self.thread.proc();
        if rsrc != 0 {
            proc.get_object::<Resource>(rsrc)?
                .validate(ResourceKind::ROOT)?;
        }
        let dlog = DebugLog::create(options);
        const FLAG_READABLE: u32 = 0x4000_0000u32;
        let dlog_right = if options & FLAG_READABLE == 0 {
            Rights::DEFAULT_DEBUGLOG
        } else {
            Rights::DEFAULT_DEBUGLOG | Rights::READ
        };
        let dlog_handle = proc.add_handle(Handle::new(dlog, dlog_right));
        target.write(dlog_handle)?;
        Ok(())
    }

    /// Write log entry to debuglog.
    pub fn sys_debuglog_write(
        &self,
        handle_value: HandleValue,
        options: u32,
        buf: UserInPtr<u8>,
        len: usize,
    ) -> ZxResult {
        info!(
            "debuglog.write: handle={:#x?}, options={:#x?}, buf=({:#x?}; {:#x?})",
            handle_value, options, buf, len,
        );
        const LOG_FLAGS_MASK: u32 = 0x10;
        if options & !LOG_FLAGS_MASK != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let datalen = len.min(224);
        let data = buf.read_string(datalen as usize)?;
        let proc = self.thread.proc();
        let dlog = proc.get_object_with_rights::<DebugLog>(handle_value, Rights::WRITE)?;
        dlog.write(Severity::Info, options, self.thread.id(), proc.id(), &data);
        // print to kernel console
        kernel_hal::console::console_write(&data);
        if data.as_bytes().last() != Some(&b'\n') {
            kernel_hal::console::console_write("\n");
        }
        Ok(())
    }

    #[allow(unsafe_code)]
    /// Read log entries from debuglog.
    pub fn sys_debuglog_read(
        &self,
        handle_value: HandleValue,
        options: u32,
        mut buf: UserOutPtr<u8>,
        len: usize,
    ) -> ZxResult {
        info!(
            "debuglog.read: handle={:#x?}, options={:#x?}, buf=({:#x?}; {:#x?})",
            handle_value, options, buf, len,
        );
        if options != 0 {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        let mut buffer = [0; DLOG_MAX_LEN];
        let dlog = proc.get_object_with_rights::<DebugLog>(handle_value, Rights::READ)?;
        let actual_len = dlog.read(&mut buffer).min(len);
        if actual_len == 0 {
            return Err(ZxError::SHOULD_WAIT);
        }
        buf.write_array(&buffer[..actual_len])?;
        // special case: return actual_len as status
        Err(unsafe { core::mem::transmute(actual_len as u32) })
    }
}
