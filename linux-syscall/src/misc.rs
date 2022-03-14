use super::*;
use bitflags::bitflags;
use linux_object::time::*;

impl Syscall<'_> {
    /// set architecture-specific thread state
    /// for x86_64 currently.
    ///
    /// code selects a subfunction and passes argument addr to it;
    /// addr is interpreted as an usize for both the "set" operations
    /// and the "get" operations.
    ///
    /// currently code only support `ARCH_SET_FS`.
    ///
    /// TODO: ARCH_SET_CPUID, ARCH_GET_CPUID, ARCH_GET_FS, ARCH_SET_GS, ARCH_GET_GS.
    #[cfg(target_arch = "x86_64")]
    pub fn sys_arch_prctl(&mut self, code: i32, addr: usize) -> SysResult {
        const ARCH_SET_FS: i32 = 0x1002;
        match code {
            ARCH_SET_FS => {
                info!("sys_arch_prctl: set FSBASE to {:#x}", addr);
                self.thread.with_context(|ctx| {
                    ctx.set_field(kernel_hal::context::UserContextField::ThreadPointer, addr)
                })?;
                Ok(0)
            }
            _ => Err(LxError::EINVAL),
        }
    }

    /// get name and about current kernel.
    ///
    /// returns system information in the structure
    /// pointed to by `buf`.
    ///
    /// the `SysResult` is an alias for `LxError`
    /// which defined in `linux-object/src/error.rs`.
    ///
    /// currently support arch: x86_64, aarch64, riscv64
    pub fn sys_uname(&self, buf: UserOutPtr<u8>) -> SysResult {
        info!("uname: buf={:?}", buf);

        let release = alloc::string::String::from(concat!(env!("CARGO_PKG_VERSION"), "-zcore"));
        #[cfg(not(target_os = "none"))]
        let release = release + "-libos";

        let vdso_const = kernel_hal::vdso::vdso_constants();

        let arch = if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else if cfg!(target_arch = "riscv64") {
            "riscv64"
        } else {
            "unknown"
        };

        let strings = [
            "Linux",                            // sysname
            "zcore",                            // nodename
            release.as_str(),                   // release
            vdso_const.version_string.as_str(), // version
            arch,                               // machine
            "rcore-os",                         // domainname
        ];

        for (i, &s) in strings.iter().enumerate() {
            const OFFSET: usize = 65;
            buf.add(i * OFFSET).write_cstring(s)?;
        }
        Ok(0)
    }

    /// provides a simple way of getting overall system statistics.
    ///
    /// returns system information in the structure
    /// pointed to by sys_info.
    ///
    /// such as:
    ///
    /// - `uptime: u64` - Seconds since boot
    ///
    /// - `loads: [u64; 3]` - 1, 5, and 15 minute load averages
    ///
    /// - `totalram: u64` - Total usable main memory size
    ///
    /// - `freeram: u64` - Available memory size
    ///
    /// - `sharedram: u64` - Amount of shared memory
    ///
    /// - `bufferram: u64` - Memory used by buffers
    ///
    /// - `totalswap: u64` - Total swa Total swap space sizep space size
    ///
    /// - `freeswap: u64` - swap space still available
    ///
    /// - `procs: u16` - Number of current processes
    ///
    /// - `totalhigh: u64` - Total high memory size
    ///
    /// - `freehigh: u64` - Available high memory size
    ///
    /// - `mem_unit: u32` - Memory unit size in bytes
    ///
    /// the `SysResult` is an alias for `LxError`
    /// which defined in `linux-object/src/error.rs`.
    pub fn sys_sysinfo(&mut self, mut sys_info: UserOutPtr<SysInfo>) -> SysResult {
        let sysinfo = SysInfo::default();
        sys_info.write(sysinfo)?;
        Ok(0)
    }

    /// provides a method for waiting until a certain condition becomes true.
    ///
    /// It is typically used as a
    /// blocking construct in the context of shared-memory
    /// synchronization.
    ///
    /// When using futexes, the majority of the
    /// synchronization operations are performed in user space.
    ///
    /// A user-
    /// space program employs the futex() system call only when it is
    /// likely that the program has to block for a longer time until the
    /// condition becomes true.  
    ///
    /// Other futex() operations can be used to
    /// wake any processes or threads waiting for a particular condition.
    ///
    /// - `uaddr` - points to the futex word.
    /// - `op` -  the operation to perform on the futex
    /// - `val` -  a value whose meaning and purpose depends on op
    /// - `timeout` - not support now
    ///
    /// the `SysResult` is an alias for `LxError`
    /// which defined in `linux-object/src/error.rs`.
    ///
    /// TODO: support timeout
    pub async fn sys_futex(
        &self,
        uaddr: usize,
        op: u32,
        val: i32,
        timeout_addr: usize,
    ) -> SysResult {
        let op = FutexFlags::from_bits_truncate(op);
        let timeout = if op.contains(FutexFlags::WAKE) {
            self.into_inout_userptr::<TimeSpec>(0).unwrap()
        } else {
            let timeout_result = self.into_inout_userptr::<TimeSpec>(timeout_addr);
            match timeout_result {
                Ok(t) => t,
                Err(_e) => return Err(LxError::EACCES),
            }
        };
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

    /// Combines and extends the functionality of setrlimit() and getrlimit().
    ///
    /// Each `resource` has an associated soft and hard limit,
    /// as defined by the `rlimit` structure
    /// which defined in `linux-object/src/process.rs`.
    ///
    /// The soft limit is the value that the kernel enforces
    /// for the corresponding resource.
    ///  
    /// The hard limit acts as a ceiling
    /// for the soft limit: an unprivileged process may set only its soft limit
    /// to a value in the range from 0 up to the hard limit,
    /// and (irreversibly) lower its hard limit.
    ///
    /// the `SysResult` is an alias for `LxError`
    /// which defined in `linux-object/src/error.rs`.
    ///
    /// currently resource support: `RLIMIT_STACK`, `RLIMIT_NOFILE`, `RLIMIT_RSS`.
    ///
    /// TODO: RLIMIT_AS, RLIMIT_CORE, RLIMIT_CPU,
    /// RLIMIT_DATA, RLIMIT_FSIZE, RLIMIT_LOCKS,
    /// RLIMIT_MEMLOCK, RLIMIT_MSGQUEUE, RLIMIT_NICE, RLIMIT_NPROC,
    /// RLIMIT_RTPRIO, RLIMIT_RTTIME, RLIMIT_SIGPENDING.
    pub fn sys_prlimit64(
        &mut self,
        pid: usize,
        resource: usize,
        new_limit: UserInPtr<RLimit>,
        mut old_limit: UserOutPtr<RLimit>,
    ) -> SysResult {
        info!(
            "prlimit64: pid: {}, resource: {}, new_limit: {:x?}, old_limit: {:x?}",
            pid, resource, new_limit, old_limit
        );
        let proc = self.linux_process();
        match resource {
            RLIMIT_STACK => {
                old_limit.write_if_not_null(RLimit {
                    cur: USER_STACK_SIZE as u64,
                    max: USER_STACK_SIZE as u64,
                })?;
                Ok(0)
            }
            RLIMIT_NOFILE => {
                let new_limit = new_limit.read_if_not_null()?;
                old_limit.write_if_not_null(proc.file_limit(new_limit))?;
                Ok(0)
            }
            RLIMIT_RSS | RLIMIT_AS => {
                old_limit.write_if_not_null(RLimit {
                    cur: 1024 * 1024 * 1024,
                    max: 1024 * 1024 * 1024,
                })?;
                Ok(0)
            }
            _ => Err(LxError::ENOSYS),
        }
    }

    #[allow(unsafe_code)]
    /// fills the buffer pointed to by `buf` with up to `buflen` random bytes.
    /// - `buf` - buffer that needed to fill
    /// - `buflen` - length of buffer
    /// - `flag` - a bit mask that can contain zero or more of the following values ORed together:
    ///   - GRND_RANDOM
    ///   - GRND_NONBLOCK
    /// - returns the number of bytes that were copied to the buffer buf.
    pub fn sys_getrandom(&mut self, mut buf: UserOutPtr<u8>, len: usize, flag: u32) -> SysResult {
        info!("getrandom: buf: {:?}, len: {:?}, flag {:?}", buf, len, flag);
        let mut buffer = vec![0u8; len];
        kernel_hal::rand::fill_random(&mut buffer);
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

const USER_STACK_SIZE: usize = 8 * 1024 * 1024; // 8 MB, the default config of Linux

const RLIMIT_STACK: usize = 3;
const RLIMIT_RSS: usize = 5;
const RLIMIT_NOFILE: usize = 7;
const RLIMIT_AS: usize = 9;

/// sysinfo() return information sturct
#[repr(C)]
#[derive(Debug, Default)]
pub struct SysInfo {
    /// Seconds since boot
    uptime: u64,
    /// 1, 5, and 15 minute load averages
    loads: [u64; 3],
    /// Total usable main memory size
    totalram: u64,
    /// Available memory size
    freeram: u64,
    /// Amount of shared memory
    sharedram: u64,
    /// Memory used by buffers
    bufferram: u64,
    /// Total swa Total swap space sizep space size
    totalswap: u64,
    /// swap space still available
    freeswap: u64,
    /// Number of current processes
    procs: u16,
    /// Total high memory size
    totalhigh: u64,
    /// Available high memory size
    freehigh: u64,
    /// Memory unit size in bytes
    mem_unit: u32,
}
