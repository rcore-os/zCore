use {
    super::*,
    core::{sync::atomic::*, time::Duration},
};

impl Syscall<'_> {
    pub async fn sys_futex_wait(
        &self,
        value_ptr: UserInPtr<AtomicI32>,
        current_value: i32,
        new_futex_owner: HandleValue,
        deadline: i64,
    ) -> ZxResult {
        info!(
            "futex.wait: value_ptr={:#x?}, current_value={:#x}, new_futex_owner={:#x}, deadline={:#x}",
            value_ptr, current_value, new_futex_owner, deadline
        );
        let value = value_ptr.as_ref()?;
        let proc = self.thread.proc();
        let futex = proc.get_futex(value);
        let new_owner = if new_futex_owner == INVALID_HANDLE {
            None
        } else {
            Some(proc.get_object::<Thread>(new_futex_owner)?)
        };
        let deadline = if deadline == i64::max_value() {
            None
        } else {
            Some(Duration::from_nanos(deadline.max(0) as u64))
        };
        futex
            .wait_with_owner(
                current_value,
                Some(self.thread.clone()),
                new_owner,
                deadline,
            )
            .await?;
        Ok(())
    }

    pub fn sys_futex_requeue(
        &self,
        value_ptr: UserInPtr<AtomicI32>,
        wake_count: u32,
        current_value: i32,
        requeue_ptr: UserInPtr<AtomicI32>,
        requeue_count: u32,
        new_requeue_owner: HandleValue,
    ) -> ZxResult {
        info!(
            "futex.requeue: value_ptr={:?}, wake_count={:#x}, current_value={:#x}, requeue_ptr={:?}, requeue_count={:#x}, new_requeue_owner={:?}",
            value_ptr, wake_count, current_value, requeue_ptr, requeue_count, new_requeue_owner
        );
        let value = value_ptr.as_ref()?;
        let requeue = requeue_ptr.as_ref()?;
        if value_ptr.as_ptr() == requeue_ptr.as_ptr() {
            return Err(ZxError::INVALID_ARGS);
        }
        let proc = self.thread.proc();
        let new_requeue_owner = if new_requeue_owner == INVALID_HANDLE {
            None
        } else {
            Some(proc.get_object::<Thread>(new_requeue_owner)?)
        };
        let wake_futex = proc.get_futex(value);
        let requeue_futex = proc.get_futex(requeue);
        wake_futex.requeue(
            current_value,
            wake_count as usize,
            requeue_count as usize,
            &requeue_futex,
            new_requeue_owner,
        )?;
        Ok(())
    }

    pub fn sys_futex_wake(&self, value_ptr: UserInPtr<AtomicI32>, count: u32) -> ZxResult {
        info!("futex.wake: value_ptr={:?}, count={:#x}", value_ptr, count);
        let value = value_ptr.as_ref()?;
        let proc = self.thread.proc();
        let futex = proc.get_futex(value);
        futex.wake(count as usize);
        Ok(())
    }

    pub fn sys_futex_wake_single_owner(&self, value_ptr: UserInPtr<AtomicI32>) -> ZxResult {
        info!("futex.wake_single_owner: value_ptr={:?}", value_ptr);
        let value = value_ptr.as_ref()?;
        let proc = self.thread.proc();
        proc.get_futex(value).wake_single_owner();
        Ok(())
    }
}
