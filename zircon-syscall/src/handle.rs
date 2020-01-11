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

    pub fn sys_handle_close(&self, handle: HandleValue) -> ZxResult<usize> {
        info!("handle.close: handle={:?}", handle);
        if handle == INVALID_HANDLE {
            return Ok(0);
        }
        let proc = &self.thread.proc;
        proc.remove_handle(handle)?;
        Ok(0)
    }

    pub fn sys_handle_close_many(
        &self,
        handles: UserInPtr<HandleValue>,
        num_handles: usize,
    ) -> ZxResult<usize> {
        info!(
            "handle.close_many: handles=({:?}; {:?})",
            handles, num_handles,
        );
        let proc = &self.thread.proc;
        let handles = handles.read_array(num_handles)?;
        for handle in handles {
            if handle == INVALID_HANDLE {
                continue;
            }
            proc.remove_handle(handle)?;
        }
        Ok(0)
    }
}
