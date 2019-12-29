use super::*;

impl Syscall {
    pub fn sys_process_exit(&self, code: i64) -> ZxResult<usize> {
        panic!("process exit: code={:?}", code);
    }
}
