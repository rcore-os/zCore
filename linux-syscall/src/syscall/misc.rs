use super::*;

impl Syscall {
    #[cfg(target_arch = "x86_64")]
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

    pub fn sys_uname(&self, buf: UserOutPtr<u8>) -> SysResult {
        info!("uname: buf={:?}", buf);

        let strings = ["rCore", "orz", "0.1.0", "1", "machine", "domain"];
        for (i, &s) in strings.iter().enumerate() {
            const OFFSET: usize = 65;
            buf.add(i * OFFSET).write_cstring(s)?;
        }
        Ok(0)
    }
}
