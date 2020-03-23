use {super::*, core::sync::atomic::*};

impl Syscall<'_> {
    pub async fn sys_futex_wait(
        &self,
        value_ptr: UserInPtr<AtomicI32>,
        current_value: i32,
        new_futex_owner: HandleValue,
        deadline: u64,
    ) -> ZxResult<usize> {
        info!(
            "futex.wait: value_ptr={:?}, current_value={:#x}, new_futex_owner={:#x}, deadline={:#x}",
            value_ptr, current_value, new_futex_owner, deadline
        );
        let value = value_ptr.as_ref()?;
        if value.load(Ordering::SeqCst) != current_value {
            return Err(ZxError::BAD_STATE);
        }
        let proc = self.thread.proc();
        let futex = proc.get_futex(value);
        let new_owner = if new_futex_owner == INVALID_HANDLE {
            None
        } else {
            Some(proc.get_object::<Thread>(new_futex_owner)?)
        };
        futex.set_owner(new_owner)?;
        futex.wait_async(current_value, self.thread.clone()).await?;
        Ok(0)
    }

    pub fn sys_futex_requeue(
        &self,
        value_ptr: UserInPtr<AtomicI32>,
        wake_count: u32,
        current_value: i32,
        requeue_ptr: UserInPtr<AtomicI32>,
        requeue_count: u32,
        new_requeue_owner: HandleValue,
    ) -> ZxResult<usize> {
        info!(
            "futex.requeue: value_ptr={:#x}, wake_count={:#x}, current_value={:#x}, requeue_ptr={:#x}, requeue_count={:#x}, new_requeue_owner={:#x}",
            value_ptr.as_ptr() as usize, wake_count, current_value, requeue_ptr.as_ptr() as usize, requeue_count, new_requeue_owner
        );
        let value = value_ptr.as_ref()?;
        let requeue = requeue_ptr.as_ref()?;
        if value_ptr.as_ptr() == requeue_ptr.as_ptr() {
            return Err(ZxError::INVALID_ARGS);
        }
        if value.load(Ordering::SeqCst) != current_value {
            return Err(ZxError::BAD_STATE);
        }
        let proc = self.thread.proc();
        let new_owner = if new_requeue_owner == INVALID_HANDLE {
            None
        } else {
            Some(proc.get_object::<Thread>(new_requeue_owner)?)
        };
        let wake_futex = proc.get_futex(value);
        let requeue_futex = proc.get_futex(requeue);
        wake_futex.set_owner(None)?;
        requeue_futex.set_owner(new_owner)?;
        wake_futex.wake_and_requeue(wake_count as usize, requeue_futex, requeue_count as usize)?;
        Ok(0)
    }

    pub fn sys_futex_wake(&self, value_ptr: UserInPtr<AtomicI32>, count: u32) -> ZxResult<usize> {
        info!("futex.wake: value_ptr={:?}, count={:#x}", value_ptr, count);
        let value = value_ptr.as_ref()?;
        let proc = self.thread.proc();
        let futex = proc.get_futex(value);
        futex.wake(count as usize);
        futex.set_owner(None)?;
        Ok(0)
    }

    pub fn sys_futex_wake_single_owner(&self, value_ptr: UserInPtr<AtomicI32>) -> ZxResult<usize> {
        info!("futex.wake_single_owner: value_ptr={:?}", value_ptr);
        let value = value_ptr.as_ref()?;
        let proc = self.thread.proc();
        proc.get_futex(value).wake_single_owner();
        Ok(0)
    }
}
