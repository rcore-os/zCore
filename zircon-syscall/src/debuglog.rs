use {
    super::*,
    zircon_object::{debuglog::DebugLog, resource::ResourceKind},
};

const FLAG_READABLE: u32 = 0x4000_0000u32;

impl Syscall {
    pub fn sys_debuglog_create(
        &self,
        rsrc: HandleValue,
        options: u32,
        mut target: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        info!(
            "debuglog_create: resource_handle={:?}, options={:?}",
            rsrc, options,
        );
        let proc = self.thread.proc();
        proc.validate_resource(rsrc, ResourceKind::ROOT)?;
        let dlog = DebugLog::create(options);
        let dlog_right = if options & FLAG_READABLE == 0 {
            Rights::DEFAULT_LOG_WRITABLE
        } else {
            Rights::DEFAULT_LOG_READABLE
        };
        let dlog_handle = self.thread.proc().add_handle(Handle::new(dlog, dlog_right));
        target.write(dlog_handle)?;
        Ok(0)
    }

    pub fn sys_debuglog_write(
        &self,
        handle_value: HandleValue,
        flags: u32,
        buf: UserInPtr<u8>,
        len: usize,
    ) -> ZxResult<usize> {
        let datalen = len.max(224);
        let data = buf.read_string(datalen as usize)?;
        self.thread
            .proc()
            .get_object_with_rights::<DebugLog>(handle_value, Rights::WRITE)?
            .write(flags, &data)
    }
}
