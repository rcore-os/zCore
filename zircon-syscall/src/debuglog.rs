use {super::*, zircon_object::resource::ResourceKind};

impl Syscall {
    pub fn sys_debuglog_create(
        &self,
        rsrc: HandleValue,
        options: usize,
        target: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        info!(
            "debuglog_create: resource_handle={:?}, options={:?}",
            rsrc, options,
        );
        let proc = &self.thread.proc;
        proc.validate_resource(rsrc, ResourceKind::ROOT)?;
        target.write(1u32)?;
        Ok(ZxError::OK as usize)
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
