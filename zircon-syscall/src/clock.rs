use super::*;

impl Syscall<'_> {
    pub fn sys_clock_get(&self, _clock_id: u32, _time: UserOutPtr<u64>) -> ZxResult<usize> {
        unimplemented!()
    }
}
