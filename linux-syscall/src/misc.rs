use super::time::TimeSpec;
use super::*;
use bitflags::bitflags;

impl Syscall<'_> {
    #[cfg(target_arch = "x86_64")]
    /// set architecture-specific thread state
    /// for x86_64 currently
    pub fn sys_arch_prctl(&mut self, code: i32, addr: usize) -> SysResult {
        const ARCH_SET_FS: i32 = 0x1002;
        match code {
            ARCH_SET_FS => {
                info!("sys_arch_prctl: set FSBASE to {:#x}", addr);
                self.regs.fsbase = addr;
                Ok(0)
            }
            _ => Err(LxError::EINVAL),
        }
    }

    /// get name and information about current kernel
    pub fn sys_uname(&self, buf: UserOutPtr<u8>) -> SysResult {
        info!("uname: buf={:?}", buf);

        let strings = ["Linux", "orz", "0.1.0", "1", "machine", "domain"];
        for (i, &s) in strings.iter().enumerate() {
            const OFFSET: usize = 65;
            buf.add(i * OFFSET).write_cstring(s)?;
        }
        Ok(0)
    }

    /// provides a method for waiting until a certain condition becomes true.
    /// - `uaddr` - points to the futex word.
    /// - `op` -  the operation to perform on the futex
    /// - `val` -  a value whose meaning and purpose depends on op
    /// - `timeout` - not support now
    /// TODO: support timeout
    pub async fn sys_futex(
        &self,
        uaddr: usize,
        op: u32,
        val: i32,
        timeout: UserInPtr<TimeSpec>,
    ) -> SysResult {
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

    #[allow(unsafe_code)]
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    /// fills the buffer pointed to by `buf` with up to `buflen` random bytes.
    /// - `buf` - buffer that needed to fill
    /// - `buflen` - length of buffer
    /// - `flag` - a bit mask that can contain zero or more of the following values ORed together:
    ///   - GRND_RANDOM
    ///   - GRND_NONBLOCK
    /// - returns the number of bytes that were copied to the buffer buf.
    pub fn sys_getrandom(&mut self, mut buf: UserOutPtr<u8>, len: usize, _flag: u32) -> SysResult {
        // info!("getrandom: buf: {:?}, len: {:?}, falg {:?}", buf, len, flag);
        let mut buffer = vec![0u8; len];
        kernel_hal::fill_random(&mut buffer);
        buf.write_array(&buffer[..len])?;
        Ok(len)
    }
}

bitflags! {
    /// for op argument in futex()
    struct FutexFlags: u32 {
        /// tests that the value at the futex word pointed
        /// to by the address uaddr still contains the expected value val,
        /// and if so, then sleeps waiting for a FUTEX_WAKE operation on the futex word.
        const WAIT      = 0;
        /// wakes at most val of the waiters that are waiting on the futex word at the address uaddr.
        const WAKE      = 1;
        /// can be employed with all futex operations, tells the kernel that the futex is process-private and not shared with another process
        const PRIVATE   = 0x80;
    }
}
