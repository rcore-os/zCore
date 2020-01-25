use super::*;

impl Syscall {
    pub fn sys_arch_prctl(&self, code: i32, addr: usize) -> SysResult {
        const ARCH_SET_FS: i32 = 0x1002;
        match code {
            ARCH_SET_FS => {
                info!("sys_arch_prctl: set FSBASE to {:#x}", addr);
                hal::set_user_fsbase(addr);
                Ok(0)
            }
            _ => Err(SysError::EINVAL),
        }
    }
}
