//! `pidfd_open`, `pidfd_send_signal`, `pidfd_getfd`

use super::*;
use linux_object::fs::{OpenFlags, PidFd, PIDFD_THREAD};
use linux_object::process::send_signal_to_process;
use linux_object::signal::{SigInfo, Signal};
use zircon_object::object::KoID;
use zircon_object::task::{Status, ROOT_JOB};

impl Syscall<'_> {
    /// Create a pollable file descriptor referring to `pid`.
    pub fn sys_pidfd_open(&self, pid: usize, flags: u32) -> SysResult {
        if pid == 0 || pid > i32::MAX as usize {
            return Err(LxError::EINVAL);
        }
        const NONBLOCK: u32 = OpenFlags::NON_BLOCK.bits() as u32;
        if flags & !(NONBLOCK | PIDFD_THREAD) != 0 {
            return Err(LxError::EINVAL);
        }
        if flags & PIDFD_THREAD != 0 {
            return Err(LxError::EINVAL);
        }

        let process = ROOT_JOB.find_process(pid as KoID).ok_or(LxError::ESRCH)?;

        let mut open_flags = OpenFlags::CLOEXEC;
        if flags & NONBLOCK != 0 {
            open_flags |= OpenFlags::NON_BLOCK;
        }
        let pidfd = PidFd::new(process, open_flags);
        let fd = self.linux_process().add_file(pidfd)?;
        Ok(fd.into())
    }

    /// Send `sig` to the process referred to by `pidfd`.
    pub fn sys_pidfd_send_signal(
        &self,
        pidfd: FileDesc,
        signum: usize,
        info: UserInPtr<SigInfo>,
        flags: u32,
    ) -> SysResult {
        if flags != 0 {
            return Err(LxError::EINVAL);
        }
        let signal = Signal::try_from(signum as u8).map_err(|_| LxError::EINVAL)?;
        let proc = self.linux_process();
        let pidfd = PidFd::from_file_like(proc.get_file_like(pidfd)?)?;
        let target = pidfd.target().clone();
        if matches!(target.status(), Status::Exited(_)) {
            return Err(LxError::ESRCH);
        }
        let _ = info;
        match signal {
            Signal::SIGKILL => {
                let retcode = (128 + Signal::SIGKILL as i32) as i64;
                target.exit(retcode);
            }
            sig => send_signal_to_process(target.id() as usize, sig)?,
        }
        Ok(0)
    }

    /// Duplicate `targetfd` from the process referred to by `pidfd`.
    pub fn sys_pidfd_getfd(&self, pidfd: FileDesc, targetfd: i32, flags: u32) -> SysResult {
        if flags != 0 || targetfd < 0 {
            return Err(LxError::EINVAL);
        }
        let caller = self.linux_process();
        let pidfd = PidFd::from_file_like(caller.get_file_like(pidfd)?)?;
        let target_proc = pidfd.target();
        if matches!(target_proc.status(), Status::Exited(_)) {
            return Err(LxError::ESRCH);
        }
        let file = target_proc
            .try_linux()
            .ok_or(LxError::ESRCH)?
            .get_file_like(targetfd.into())?
            .dup();
        let mut open_flags = file.flags();
        open_flags |= OpenFlags::CLOEXEC;
        file.set_flags(open_flags)?;
        let new_fd = caller.add_file(file)?;
        Ok(new_fd.into())
    }
}
