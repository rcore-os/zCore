use super::time::TimeSpec;
use super::*;
use bitflags::bitflags;

impl Syscall<'_> {
    #[cfg(target_arch = "x86_64")]
    pub fn sys_arch_prctl(&mut self, code: i32, addr: usize) -> SysResult {
        const ARCH_SET_FS: i32 = 0x1002;
        match code {
            ARCH_SET_FS => {
                info!("sys_arch_prctl: set FSBASE to {:#x}", addr);
                self.context.general.fsbase = addr;
                Ok(0)
            }
            _ => Err(LxError::EINVAL),
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

    pub async fn sys_futex(
        &self,
        uaddr: usize,
        op: u32,
        val: i32,
        timeout: UserInPtr<TimeSpec>,
    ) -> SysResult {
        bitflags! {
            struct FutexFlags: u32 {
                const WAIT      = 0;
                const WAKE      = 1;
                const PRIVATE   = 0x80;
            }
        }
        let op = FutexFlags::from_bits_truncate(op);
        info!(
            "futex: uaddr: {:#x}, op: {:?}, val: {}, timeout_ptr: {:?}",
            uaddr, op, val, timeout
        );
        if op.contains(FutexFlags::PRIVATE) {
            warn!("process-shared futex is unimplemented");
        }
        let futex = self.linux_process().get_futex(uaddr);
        match op.bits & 0xf {
            0 => {
                // FIXME: support timeout
                let _timeout = timeout.read_if_not_null()?;
                match futex.wait(val).await {
                    Ok(_) => Ok(0),
                    Err(ZxError::BAD_STATE) => Err(LxError::EAGAIN),
                    Err(e) => Err(e.into()),
                }
            }
            1 => {
                let woken_up_count = futex.wake(val as usize);
                Ok(woken_up_count)
            }
            _ => {
                warn!("unsupported futex operation: {:?}", op);
                Err(LxError::ENOSYS)
            }
        }
    }
}
