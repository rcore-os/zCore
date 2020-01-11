use super::*;
use core::convert::TryFrom;

impl Syscall {
    pub fn sys_handle_duplicate(
        &self,
        handle_value: HandleValue,
        rights: u32,
        new_handle_value: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        let rights = Rights::try_from(rights)?;
        info!("handle.dup: handle={:?}, rights={:?}", handle_value, rights);
        let proc = &self.thread.proc;
        let new_value = proc.dup_handle(handle_value, rights)?;
        new_handle_value.write(new_value)?;
        Ok(0)
    }
}
