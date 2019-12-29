use super::*;

impl Syscall {
    pub fn sys_process_exit(&self) -> ZxResult<usize> {
        panic!("process exit");
    }
}
