use super::*;
use core::time::Duration;
use kernel_hal::timer::timer_now;
use linux_object::time::*;
use zircon_object::task::ThreadState;

impl Syscall<'_> {
    #[cfg(target_arch = "x86_64")]
    /// set architecture-specific thread state
    /// for x86_64 currently
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

    /// get name and information about current kernel
    ///
    /// All six `utsname` strings come from [`linux_object::uname`], the same
    /// source `/proc/version` and `/proc/sys/kernel/*` read, so every interface
    /// reports one consistent identity. The release string is Linux-formatted
    /// ("0.4.2-eclipse"): glibc and the Go runtime parse it at startup and
    /// refuse to run when it looks older than their build-time minimum — the
    /// crate version previously reported here ("0.1.0-…") failed exactly that.
    pub fn sys_uname(&self, buf: UserOutPtr<u8>) -> SysResult {
        info!("uname: buf={:?}", buf);

        use linux_object::uname;

        let version = uname::os_version();
        let hostname = uname::hostname();
        let domainname = uname::domainname();

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
            uname::OS_TYPE,      // sysname
            hostname.as_str(),   // nodename
            uname::OS_RELEASE,   // release
            version.as_str(),    // version
            arch,                // machine
            domainname.as_str(), // domainname
        ];

        for (i, &s) in strings.iter().enumerate() {
            const OFFSET: usize = 65;
            buf.add(i * OFFSET).write_cstring(s)?;
        }
        Ok(0)
    }

    /// set the system hostname
    /// (see [linux man sethostname(2)](https://www.man7.org/linux/man-pages/man2/sethostname.2.html)).
    ///
    /// Alpine's boot scripts (`busybox hostname -F /etc/hostname`) and the
    /// `hostname` utility depend on this; the new name is immediately visible
    /// through `uname`, `gethostname` and `/proc/sys/kernel/hostname`.
    pub fn sys_sethostname(&mut self, base: UserInPtr<u8>, len: usize) -> SysResult {
        info!("sethostname: base={:?}, len={}", base, len);
        if len > linux_object::uname::HOST_NAME_MAX {
            return Err(LxError::EINVAL);
        }
        let bytes = base.read_array(len)?;
        let name = alloc::string::String::from_utf8_lossy(&bytes);
        linux_object::uname::set_hostname(&name);
        Ok(0)
    }

    /// set the system NIS domain name
    /// (see [linux man setdomainname(2)](https://www.man7.org/linux/man-pages/man2/setdomainname.2.html)).
    pub fn sys_setdomainname(&mut self, base: UserInPtr<u8>, len: usize) -> SysResult {
        info!("setdomainname: base={:?}, len={}", base, len);
        if len > linux_object::uname::HOST_NAME_MAX {
            return Err(LxError::EINVAL);
        }
        let bytes = base.read_array(len)?;
        let name = alloc::string::String::from_utf8_lossy(&bytes);
        linux_object::uname::set_domainname(&name);
        Ok(0)
    }

    /// get/set the process execution domain
    /// (see [linux man personality(2)](https://www.man7.org/linux/man-pages/man2/personality.2.html)).
    ///
    /// The persona is a real per-process value: `0xffffffff` queries without
    /// changing it, anything else is stored and the previous persona returned.
    /// Everything runs as `PER_LINUX`; modifier bits such as
    /// `ADDR_NO_RANDOMIZE` (what `setarch -R` sets — trivially satisfied here,
    /// mappings are not randomized) are kept so the caller reads back what it
    /// configured and inheritance across fork/exec behaves like Linux.
    pub fn sys_personality(&self, persona: usize) -> SysResult {
        const QUERY: u32 = 0xffff_ffff;
        let proc = self.linux_process();
        if persona as u32 == QUERY {
            return Ok(proc.personality() as usize);
        }
        info!("personality: persona={:#x}", persona);
        Ok(proc.set_personality(persona as u32) as usize)
    }

    /// determine CPU and NUMA node on which the calling thread is running
    /// (see [linux man getcpu(2)](https://www.man7.org/linux/man-pages/man2/getcpu.2.html)).
    ///
    /// musl's `sched_getcpu` is a direct wrapper over this syscall, and thread
    /// pools / allocators (jemalloc arenas, Go's scheduler instrumentation) use
    /// it for CPU-local sharding. There is a single NUMA node here.
    pub fn sys_getcpu(
        &self,
        mut cpu: UserOutPtr<u32>,
        mut node: UserOutPtr<u32>,
        _tcache: usize,
    ) -> SysResult {
        cpu.write_if_not_null(kernel_hal::cpu::cpu_id() as u32)?;
        node.write_if_not_null(0)?;
        Ok(0)
    }

    /// get I/O scheduling class and priority
    /// (see [linux man ioprio_get(2)](https://www.man7.org/linux/man-pages/man2/ioprio_set.2.html)).
    ///
    /// There is no I/O scheduler to carry the value, so every task reports the
    /// Linux default: best-effort class, priority 4 (what a task with nice 0
    /// gets). `ionice` and `tar --ioprio` run happily against this.
    pub fn sys_ioprio_get(&self, which: usize, who: usize) -> SysResult {
        info!("ioprio_get: which={}, who={}", which, who);
        // IOPRIO_WHO_PROCESS / _PGRP / _USER
        if !(1..=3).contains(&which) {
            return Err(LxError::EINVAL);
        }
        const IOPRIO_CLASS_BE: usize = 2;
        const IOPRIO_CLASS_SHIFT: usize = 13;
        Ok((IOPRIO_CLASS_BE << IOPRIO_CLASS_SHIFT) | 4)
    }

    /// set I/O scheduling class and priority
    /// (see [linux man ioprio_set(2)](https://www.man7.org/linux/man-pages/man2/ioprio_set.2.html)).
    ///
    /// Accepted and not acted upon (there is no I/O scheduler); arguments are
    /// still validated so misuse fails loudly like on Linux.
    pub fn sys_ioprio_set(&self, which: usize, who: usize, ioprio: usize) -> SysResult {
        info!(
            "ioprio_set: which={}, who={}, ioprio={:#x}",
            which, who, ioprio
        );
        if !(1..=3).contains(&which) {
            return Err(LxError::EINVAL);
        }
        const IOPRIO_CLASS_SHIFT: usize = 13;
        // Classes: 0 = none (inherit), 1 = RT, 2 = BE, 3 = IDLE.
        if ioprio >> IOPRIO_CLASS_SHIFT > 3 {
            return Err(LxError::EINVAL);
        }
        Ok(0)
    }

    /// get capabilities of a process
    /// (see [linux man capget(2)](https://www.man7.org/linux/man-pages/man2/capget.2.html)).
    ///
    /// Implements the header version-negotiation protocol from
    /// `linux/capability.h`: an unrecognised version gets the preferred one
    /// (V3) written back plus `EINVAL`, which is how libcap probes. Every
    /// process running as root reports the full capability set; anything else
    /// reports empty sets. Tools like `ping` and container runtimes check
    /// themselves with this before attempting privileged operations.
    pub fn sys_capget(
        &self,
        mut header: UserInOutPtr<CapUserHeader>,
        mut data: UserOutPtr<CapUserData>,
    ) -> SysResult {
        let hdr = header.read()?;
        info!("capget: version={:#x}, pid={}", hdr.version, hdr.pid);
        let elems = match cap_version_elems(hdr.version) {
            Some(n) => n,
            None => {
                header.write(CapUserHeader {
                    version: LINUX_CAPABILITY_VERSION_3,
                    pid: hdr.pid,
                })?;
                return Err(LxError::EINVAL);
            }
        };
        if hdr.pid < 0 {
            return Err(LxError::EINVAL);
        }
        if data.is_null() {
            return Ok(0);
        }
        // CAP_LAST_CAP is 40 on Linux 5.15: bits 0..=40 are valid.
        const CAP_FULL_SET: u64 = (1 << 41) - 1;
        let caps = if self.linux_process().euid() == 0 {
            CAP_FULL_SET
        } else {
            0
        };
        let mut out = [CapUserData::default(); 2];
        out[0].effective = caps as u32;
        out[0].permitted = caps as u32;
        out[1].effective = (caps >> 32) as u32;
        out[1].permitted = (caps >> 32) as u32;
        data.write_array(&out[..elems])?;
        Ok(0)
    }

    /// set capabilities of a process
    /// (see [linux man capset(2)](https://www.man7.org/linux/man-pages/man2/capset.2.html)).
    ///
    /// Version/pid validation matches [`sys_capget`](Self::sys_capget); the new
    /// sets are then accepted without being stored — every root process already
    /// holds the full set here and there is no privilege machinery for a
    /// reduced set to constrain. Capability-dropping daemons proceed as if it
    /// worked, which on this single-user kernel it effectively has.
    pub fn sys_capset(
        &self,
        mut header: UserInOutPtr<CapUserHeader>,
        data: UserInPtr<CapUserData>,
    ) -> SysResult {
        let hdr = header.read()?;
        info!("capset: version={:#x}, pid={}", hdr.version, hdr.pid);
        let elems = match cap_version_elems(hdr.version) {
            Some(n) => n,
            None => {
                header.write(CapUserHeader {
                    version: LINUX_CAPABILITY_VERSION_3,
                    pid: hdr.pid,
                })?;
                return Err(LxError::EINVAL);
            }
        };
        if hdr.pid < 0 {
            return Err(LxError::EINVAL);
        }
        let _sets = data.read_array(elems)?;
        Ok(0)
    }

    /// Read and/or clear kernel message ring buffer; set console_loglevel
    pub fn sys_syslog(&self, type_: i32, mut buf: UserOutPtr<u8>, len: i32) -> SysResult {
        info!("syslog: type={}, buf={:?}, len={}", type_, buf, len);
        // syslog(2) action codes
        const SYSLOG_ACTION_CLOSE: i32 = 0;
        const SYSLOG_ACTION_OPEN: i32 = 1;
        const SYSLOG_ACTION_READ: i32 = 2; // read & clear (we treat as READ_ALL)
        const SYSLOG_ACTION_READ_ALL: i32 = 3;
        const SYSLOG_ACTION_READ_CLEAR: i32 = 4;
        const SYSLOG_ACTION_CLEAR: i32 = 5;
        const SYSLOG_ACTION_CONSOLE_OFF: i32 = 6;
        const SYSLOG_ACTION_CONSOLE_ON: i32 = 7;
        const SYSLOG_ACTION_CONSOLE_LEVEL: i32 = 8;
        const SYSLOG_ACTION_SIZE_UNREAD: i32 = 9;
        const SYSLOG_ACTION_SIZE_BUFFER: i32 = 10;

        match type_ {
            SYSLOG_ACTION_CLOSE
            | SYSLOG_ACTION_OPEN
            | SYSLOG_ACTION_CLEAR
            | SYSLOG_ACTION_CONSOLE_OFF
            | SYSLOG_ACTION_CONSOLE_ON
            | SYSLOG_ACTION_CONSOLE_LEVEL => Ok(0),

            SYSLOG_ACTION_SIZE_BUFFER => Ok(kernel_hal::console::klog_buf_size()),

            SYSLOG_ACTION_SIZE_UNREAD => Ok(kernel_hal::console::klog_buf_size()),

            SYSLOG_ACTION_READ | SYSLOG_ACTION_READ_ALL | SYSLOG_ACTION_READ_CLEAR => {
                // A negative `len` would sign-extend to a huge `usize`, letting
                // the read write past the (smaller) user buffer.
                if len < 0 {
                    return Err(LxError::EINVAL);
                }
                let cap = (len as usize).min(kernel_hal::console::klog_buf_size().max(1));
                let mut tmp = vec![0u8; cap];
                let n = kernel_hal::console::klog_read(&mut tmp);
                if n > 0 {
                    buf.write_array(&tmp[..n])?;
                }
                Ok(n)
            }

            _ => Ok(0),
        }
    }

    /// provides a simple way of getting overall system statistics
    pub fn sys_sysinfo(&mut self, mut sys_info: UserOutPtr<SysInfo>) -> SysResult {
        // `uptime` was the headline: returning the zeroed default made
        // `uptime`/`top` always report "up 0 min". Fill the fields we can
        // source cheaply so userspace tools show real numbers.
        let (used, total) = kernel_hal::mem::memory_usage();
        let (procs, _running) = linux_object::loadavg::count_processes();
        let sysinfo = SysInfo {
            // Seconds since boot, from the monotonic timer (same source as
            // /proc/uptime).
            uptime: timer_now().as_secs(),
            // Approximate 1/5/15-minute run-queue load averages, in the
            // fixed point sysinfo expects (value << 16).
            loads: linux_object::loadavg::loadavg_sysinfo(),
            totalram: total as u64,
            freeram: total.saturating_sub(used) as u64,
            procs: procs as u16,
            mem_unit: 1,
            ..SysInfo::default()
        };
        sys_info.write(sysinfo)?;
        Ok(0)
    }

    /// `membarrier` issues memory barriers across the threads of the system
    /// (see [membarrier(2)](https://man7.org/linux/man-pages/man2/membarrier.2.html)).
    ///
    /// We advertise and accept the query, the global/private *expedited*
    /// barrier commands and their SYNC_CORE variants. Registration commands are
    /// accepted as no-ops (expedited membarrier is always permitted here).
    ///
    /// The barrier itself is a local sequentially-consistent fence. We do not
    /// broadcast a cross-CPU IPI: on this kernel the IPI receive path always
    /// flushes the whole TLB, so forcing that on every `membarrier` (a call the
    /// runtimes that use it may issue often) would be far too costly. This is a
    /// deliberate simplification — adequate for the userspaces that call it,
    /// short of the full system-wide ordering guarantee.
    pub fn sys_membarrier(&self, cmd: i32, flags: u32, _cpu_id: i32) -> SysResult {
        use core::sync::atomic::{fence, Ordering};

        const QUERY: i32 = 0;
        const GLOBAL: i32 = 1 << 0;
        const GLOBAL_EXPEDITED: i32 = 1 << 1;
        const REGISTER_GLOBAL_EXPEDITED: i32 = 1 << 2;
        const PRIVATE_EXPEDITED: i32 = 1 << 3;
        const REGISTER_PRIVATE_EXPEDITED: i32 = 1 << 4;
        const PRIVATE_EXPEDITED_SYNC_CORE: i32 = 1 << 5;
        const REGISTER_PRIVATE_EXPEDITED_SYNC_CORE: i32 = 1 << 6;

        info!("membarrier: cmd={:#x}, flags={:#x}", cmd, flags);

        // The only defined flag (MEMBARRIER_CMD_FLAG_CPU) applies to the RSEQ
        // command, which we don't advertise; reject any flag.
        if flags != 0 {
            return Err(LxError::EINVAL);
        }

        let supported = GLOBAL
            | GLOBAL_EXPEDITED
            | REGISTER_GLOBAL_EXPEDITED
            | PRIVATE_EXPEDITED
            | REGISTER_PRIVATE_EXPEDITED
            | PRIVATE_EXPEDITED_SYNC_CORE
            | REGISTER_PRIVATE_EXPEDITED_SYNC_CORE;

        match cmd {
            QUERY => Ok(supported as usize),
            GLOBAL | GLOBAL_EXPEDITED | PRIVATE_EXPEDITED | PRIVATE_EXPEDITED_SYNC_CORE => {
                fence(Ordering::SeqCst);
                Ok(0)
            }
            REGISTER_GLOBAL_EXPEDITED
            | REGISTER_PRIVATE_EXPEDITED
            | REGISTER_PRIVATE_EXPEDITED_SYNC_CORE => Ok(0),
            _ => Err(LxError::EINVAL),
        }
    }

    /// provides a method for waiting until a certain condition becomes true.
    /// - `uaddr` - points to the futex word.
    /// - `op` -  the operation to perform on the futex
    /// - `val` -  a value whose meaning and purpose depends on op
    /// - `val2` - provides a timeout for the attempt or acts as val2 when op is REQUEUE
    /// - `uaddr2` - when op is REQUEUE, points to the target futex
    /// - `val3` - expected futex value for CMP_REQUEUE; bitset mask for *_BITSET
    pub async fn sys_futex(
        &self,
        uaddr: usize,
        op: u32,
        val: u32,
        val2: usize,
        uaddr2: usize,
        val3: u32,
    ) -> SysResult {
        const FUTEX_WAIT: u32 = 0;
        const FUTEX_WAKE: u32 = 1;
        const FUTEX_REQUEUE: u32 = 3;
        const FUTEX_CMP_REQUEUE: u32 = 4;
        const FUTEX_WAIT_BITSET: u32 = 9;
        const FUTEX_WAKE_BITSET: u32 = 10;
        const FUTEX_PRIVATE_FLAG: u32 = 0x80;
        const FUTEX_CLOCK_REALTIME: u32 = 0x100;

        debug!(
            "Futex uaddr: {:#x}, op: {:x}, val: {}, val2(timeout_addr): {:x}",
            uaddr, op, val, val2,
        );
        if op & FUTEX_PRIVATE_FLAG == 0 {
            // Futexes are per-process objects here, which is correct for
            // private futexes and a usable approximation for shared ones
            // within a single process (e.g. musl pthread_join passes priv=0).
            debug!("process-shared futex is treated as process-private");
        }
        // NOTE: do NOT parse `op` as bitflags — command values are an enum
        // (WAIT_BITSET=9 would alias WAKE=1 when bits are truncated).
        let cmd = op & !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);
        let futex = self
            .linux_process()
            .get_futex(uaddr)
            .ok_or(LxError::EINVAL)?;
        match cmd {
            FUTEX_WAIT | FUTEX_WAIT_BITSET => {
                // Fast-path EAGAIN: the userspace cmpxchg often loses by the
                // time we get here (the canonical contended-mutex case in
                // musl/glibc). Short-circuit before allocating the Waiter
                // future and engaging the blocking machinery — the slow path
                // re-checks under the queue lock so this is purely an
                // optimization.
                if !futex.value_eq(val as i32) {
                    return Err(LxError::EAGAIN);
                }
                // FUTEX_WAIT_BITSET with a mask is approximated as match-any;
                // both musl and glibc only use FUTEX_BITSET_MATCH_ANY here.
                let future = futex.wait(val as _);
                let timeout_addr: UserInPtr<TimeSpec> = val2.into();
                let res = if let Some(timeout) = timeout_addr.read_if_not_null()? {
                    // FUTEX_WAIT takes a relative timeout; FUTEX_WAIT_BITSET
                    // takes an absolute one on the clock selected by
                    // FUTEX_CLOCK_REALTIME. Convert absolute deadlines to the
                    // kernel's monotonic deadline base.
                    let deadline = if cmd == FUTEX_WAIT_BITSET {
                        let now = if op & FUTEX_CLOCK_REALTIME != 0 {
                            Duration::from(TimeSpec::now())
                        } else {
                            Duration::from(TimeSpec::now_monotonic())
                        };
                        timer_now() + Duration::from(timeout).saturating_sub(now)
                    } else {
                        timer_now() + Duration::from(timeout)
                    };
                    self.thread
                        .blocking_run(future, ThreadState::BlockedFutex, deadline, None)
                        .await
                } else {
                    future.await
                };
                match res {
                    Ok(_) => Ok(0),
                    Err(e) => Err(e.into()),
                }
            }
            FUTEX_WAKE | FUTEX_WAKE_BITSET => Ok(futex.wake(val as _)),
            FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => {
                let requeue_futex = self
                    .linux_process()
                    .get_futex(uaddr2)
                    .ok_or(LxError::EINVAL)?;
                // FUTEX_CMP_REQUEUE checks *uaddr against val3 first.
                let res = futex.requeue(
                    val3 as i32,
                    val as _,
                    val2,
                    &requeue_futex,
                    None,
                    cmd == FUTEX_CMP_REQUEUE,
                );
                match res {
                    Ok(_) => Ok(0),
                    Err(e) => Err(e.into()),
                }
            }
            _ => {
                warn!("unsupported futex operation: {:#x} (cmd {})", op, cmd);
                Err(LxError::ENOSYS)
            }
        }
    }

    /// Combines and extends the functionality of setrlimit() and getrlimit()
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

    /// `getrlimit` gets resource limits.
    pub fn sys_getrlimit(&mut self, resource: usize, rlim: UserOutPtr<RLimit>) -> SysResult {
        info!("getrlimit: resource={}, rlim={:?}", resource, rlim);
        self.sys_prlimit64(0, resource, 0.into(), rlim)
    }

    /// `setrlimit` sets resource limits.
    pub fn sys_setrlimit(&mut self, resource: usize, rlim: UserInPtr<RLimit>) -> SysResult {
        info!("setrlimit: resource={}, rlim={:?}", resource, rlim);
        self.sys_prlimit64(0, resource, rlim, 0.into())
    }

    #[allow(unsafe_code)]
    /// fills the buffer pointed to by `buf` with up to `buflen` random bytes.
    /// - `buf` - buffer that needed to fill
    /// - `buflen` - length of buffer
    /// - `flag` - a bit mask that can contain zero or more of the following values ORed together:
    ///   - GRND_RANDOM
    ///   - GRND_NONBLOCK
    ///
    /// - returns the number of bytes that were copied to the buffer buf.
    ///
    /// reboot() reboots the system, or enables/disables the reboot keystroke.
    pub fn sys_reboot(
        &mut self,
        magic1: u32,
        magic2: u32,
        cmd: u32,
        _arg: UserInPtr<u8>,
    ) -> SysResult {
        warn!(
            "reboot: magic1={:#x}, magic2={:#x}, cmd={:#x}",
            magic1, magic2, cmd
        );
        if magic1 != 0xfee1dead
            || (magic2 != 0x28121969
                && magic2 != 0x05121996
                && magic2 != 0x16041998
                && magic2 != 0x20112000)
        {
            warn!("reboot: invalid magic!");
            return Err(LxError::EINVAL);
        }
        match cmd {
            0x4321fedc => {
                // LINUX_REBOOT_CMD_POWER_OFF
                warn!("reboot: poweroff...");
                kernel_hal::cpu::reset();
            }
            0x89abcdef => {
                // LINUX_REBOOT_CMD_CAD_ON
                Ok(0)
            }
            0x00000000 => {
                // LINUX_REBOOT_CMD_CAD_OFF
                Ok(0)
            }
            0xcdef0123 => {
                // LINUX_REBOOT_CMD_HALT
                warn!("reboot: halt...");
                kernel_hal::cpu::reset();
            }
            0x456789ab => {
                // LINUX_REBOOT_CMD_SW_SUSPEND
                warn!("reboot: sw_suspend unimplemented");
                Err(LxError::EINVAL)
            }
            0x01234567 => {
                // LINUX_REBOOT_CMD_RESTART
                warn!("reboot: restarting...");
                kernel_hal::cpu::reset();
            }
            _ => {
                warn!("reboot: unknown command {:#x}", cmd);
                Err(LxError::EINVAL)
            }
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
    pub fn sys_getrandom(&mut self, buf: UserOutPtr<u8>, len: usize, flag: u32) -> SysResult {
        info!("getrandom: buf: {:?}, len: {:?}, flag {:?}", buf, len, flag);
        let mut written = 0;
        let mut chunk = [0u8; 1024];
        while written < len {
            let left = len - written;
            let current_len = left.min(chunk.len());
            kernel_hal::rand::fill_random(&mut chunk[..current_len]);
            buf.add(written).write_array(&chunk[..current_len])?;
            written += current_len;
        }
        Ok(len)
    }
}

const USER_STACK_SIZE: usize = 8 * 1024 * 1024; // 8 MB, the default config of Linux

const RLIMIT_STACK: usize = 3;
const RLIMIT_RSS: usize = 5;
const RLIMIT_NOFILE: usize = 7;
const RLIMIT_AS: usize = 9;

/// `struct __user_cap_header_struct` from `linux/capability.h`, the in/out
/// header both `capget(2)` and `capset(2)` start with.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CapUserHeader {
    /// One of the `_LINUX_CAPABILITY_VERSION_{1,2,3}` magics; rewritten to the
    /// preferred version when the caller sends an unknown one (the libcap
    /// probing protocol).
    pub version: u32,
    /// Target process; 0 means the calling process.
    pub pid: i32,
}

/// `struct __user_cap_data_struct`: one 32-bit slice of the three capability
/// sets. Version 1 uses one element, versions 2/3 use two (64 bits).
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct CapUserData {
    /// Capabilities the process may currently exercise.
    pub effective: u32,
    /// Capabilities the process is permitted to make effective.
    pub permitted: u32,
    /// Capabilities preserved across `execve`.
    pub inheritable: u32,
}

const LINUX_CAPABILITY_VERSION_1: u32 = 0x1998_0330;
const LINUX_CAPABILITY_VERSION_2: u32 = 0x2007_1026;
const LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;

/// Number of `CapUserData` elements a capability ABI version transfers, or
/// `None` for an unrecognised version (which triggers the write-back probe
/// reply). Pure, so the negotiation table is unit-testable.
fn cap_version_elems(version: u32) -> Option<usize> {
    match version {
        LINUX_CAPABILITY_VERSION_1 => Some(1),
        LINUX_CAPABILITY_VERSION_2 | LINUX_CAPABILITY_VERSION_3 => Some(2),
        _ => None,
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    #[test]
    fn known_versions_map_to_element_counts() {
        assert_eq!(cap_version_elems(LINUX_CAPABILITY_VERSION_1), Some(1));
        assert_eq!(cap_version_elems(LINUX_CAPABILITY_VERSION_2), Some(2));
        assert_eq!(cap_version_elems(LINUX_CAPABILITY_VERSION_3), Some(2));
    }

    #[test]
    fn unknown_versions_are_refused() {
        // 0 is what libcap sends to probe; garbage must also be refused.
        for v in [0u32, 1, 0x2008_0523, u32::MAX] {
            assert_eq!(cap_version_elems(v), None, "version {:#x}", v);
        }
    }
}

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
