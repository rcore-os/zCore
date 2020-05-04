use {
    super::*,
    zircon_object::{debuglog::DebugLog, resource::*},
};

const FLAG_READABLE: u32 = 0x4000_0000u32;

impl Syscall<'_> {
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
        let dlog_right = if options & FLAG_READABLE == 0 {
            Rights::DEFAULT_DEBUGLOG
        } else {
            Rights::DEFAULT_DEBUGLOG | Rights::READ
        };
        let dlog_handle = proc.add_handle(Handle::new(dlog, dlog_right));
        target.write(dlog_handle)?;
        Ok(())
    }

    pub fn sys_debuglog_write(
        &self,
        handle_value: HandleValue,
        flags: u32,
        buf: UserInPtr<u8>,
        len: usize,
    ) -> ZxResult {
        info!(
            "debuglog.write: handle={:#x?}, flags={:#x?}, buf=({:#x?}; {:#x?})",
            handle_value, flags, buf, len,
        );
        let datalen = len.min(224);
        let data = buf.read_string(datalen as usize)?;
        let thread = &self.thread;
        let proc = thread.proc();
        let tid = thread.id();
        let pid = proc.id();
        proc.get_object_with_rights::<DebugLog>(handle_value, Rights::WRITE)?
            .write(flags, &data, tid, pid)?;
        Ok(())
    }
}
