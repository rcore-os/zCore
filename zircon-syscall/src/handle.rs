use {super::*, core::convert::TryFrom};

impl Syscall<'_> {
    pub fn sys_handle_duplicate(
        &self,
        handle_value: HandleValue,
        rights: u32,
        mut new_handle_value: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        let rights = Rights::try_from(rights)?;
        info!("handle.dup: handle={:?}, rights={:?}", handle_value, rights);
        let proc = self.thread.proc();
        let new_value = proc.dup_handle_operating_rights(handle_value, |handle_rights| {
            if !handle_rights.contains(Rights::DUPLICATE) {
                return Err(ZxError::ACCESS_DENIED);
            }
            if !rights.contains(Rights::SAME_RIGHTS) {
                // `rights` must be strictly lesser than of the source handle
                if !(handle_rights.contains(rights) && handle_rights != rights) {
                    return Err(ZxError::INVALID_ARGS);
                }
                Ok(rights)
            } else {
                Ok(handle_rights)
            }
        })?;
        new_handle_value.write(new_value)?;
        Ok(0)
    }

    pub fn sys_handle_close(&self, handle: HandleValue) -> ZxResult<usize> {
        info!("handle.close: handle={:?}", handle);
        if handle == INVALID_HANDLE {
            return Ok(0);
        }
        let proc = self.thread.proc();
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
        let proc = self.thread.proc();
        let handles = handles.read_array(num_handles)?;
        for handle in handles {
            if handle == INVALID_HANDLE {
                continue;
            }
            proc.remove_handle(handle)?;
        }
        Ok(0)
    }

    pub fn sys_handle_replace(
        &self,
        handle_value: HandleValue,
        rights: u32,
        mut out: UserOutPtr<HandleValue>,
    ) -> ZxResult<usize> {
        let rights = Rights::try_from(rights)?;
        info!(
            "handle.replace: handle={:?}, rights={:?}",
            handle_value, rights
        );
        let proc = self.thread.proc();
        let new_value = proc.dup_handle_operating_rights(handle_value, |handle_rights| {
            if !rights.contains(Rights::SAME_RIGHTS) {
                // `rights` must be strictly lesser than of the source handle
                if !(handle_rights.contains(rights) && handle_rights != rights) {
                    return Err(ZxError::INVALID_ARGS);
                }
            }
            Ok(rights)
        })?;
        proc.remove_handle(handle_value)?;
        out.write(new_value)?;
        Ok(0)
    }
}
