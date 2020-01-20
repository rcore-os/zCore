use {super::*, zircon_object::debuglog::DebugLog, zircon_object::resource::ResourceKind};

const FLAG_READABLE: u32 = 0x4000_0000u32;

impl Syscall {
    pub fn sys_debuglog_create(
        &self,
        rsrc: HandleValue,
        options: u32,
        target: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        info!(
            "debuglog_create: resource_handle={:?}, options={:?}",
            rsrc, options,
        );
        let proc = &self.thread.proc;
        proc.validate_resource(rsrc, ResourceKind::ROOT)?;
        let dlog = DebugLog::create(options).unwrap();
        let dlog_right = if options & FLAG_READABLE == 0 {
            Rights::DEFAULT_LOG_WRITABLE
        } else {
            Rights::DEFAULT_LOG_READABLE
        };
        let dlog_handle = self.thread.proc.add_handle(Handle::new(dlog, dlog_right));
        target.write(dlog_handle)?;
        Ok(0)
    }

    pub fn sys_debuglog_write(
        &self,
        _handle_value: HandleValue,
        _options: u32,
        buf: UserInPtr<u8>,
        len: usize,
    ) -> ZxResult<usize> {
        self.sys_debug_write(buf, len)
    }
}
