//! Syscalls of Inter-Process Communication
#![allow(dead_code)]

use bitflags::*;
use numeric_enum_macro::numeric_enum;

pub use linux_object::ipc::*;

use super::*;

impl Syscall<'_> {
    /// returns the semaphore set identifier associated with the argument key
    pub fn sys_semget(&self, key: usize, nsems: usize, flags: usize) -> SysResult {
        info!("semget: key: {} nsems: {} flags: {:#x}", key, nsems, flags);

        /// The maximum semaphores per semaphore set
        const SEMMSL: usize = 256;

        if nsems > SEMMSL {
            return Err(LxError::EINVAL);
        }

        let sem_array = SemArray::get_or_create(key as u32, nsems, flags)?;
        let id = self.linux_process().semaphores_add(sem_array);
        Ok(id)
    }

    /// semaphore operations
    ///
    /// performs operations on selected semaphores in the set indicated by semid
    pub async fn sys_semop(&self, id: usize, ops: UserInPtr<SemBuf>, num_ops: usize) -> SysResult {
        info!("semop: id: {}", id);
        let ops = ops.read_array(num_ops)?;

        let sem_array = self
            .linux_process()
            .semaphores_get(id)
            .ok_or(LxError::EINVAL)?;
        sem_array.otime();
        for &SemBuf { num, op, flags } in ops.iter() {
            let flags = SemFlags::from_bits(flags).ok_or(LxError::EINVAL)?;
            if flags.contains(SemFlags::IPC_NOWAIT) {
                unimplemented!("Semaphore: semop.IPC_NOWAIT");
            }
            let sem = &sem_array[num as usize];

            let _result = match op {
                1 => sem.release(),
                -1 => sem.acquire().await?,
                _ => unimplemented!("Semaphore: semop.(Not 1/-1)"),
            };
            sem.set_pid(self.zircon_process().id() as usize);
            if flags.contains(SemFlags::SEM_UNDO) {
                self.linux_process().semaphores_add_undo(id, num, op);
            }
        }
        Ok(0)
    }

    /// semaphore control operations
    ///
    /// performs the control operation specified by cmd on the semaphore set identified by semid,
    /// or on the semnum-th semaphore of that set.
    pub fn sys_semctl(&self, id: usize, num: usize, cmd: usize, arg: usize) -> SysResult {
        info!(
            "semctl: id: {}, num: {}, cmd: {} arg: {:#x}",
            id, num, cmd, arg
        );
        let sem_array = self
            .linux_process()
            .semaphores_get(id)
            .ok_or(LxError::EINVAL)?;

        let cmd = match SemctlCmds::try_from(cmd) {
            Ok(t) => t,
            Err(_) => {
                error!("invalid semctl cmd: {}", cmd);
                return Err(LxError::EINVAL);
            }
        };
        match cmd {
            SemctlCmds::IPC_RMID => {
                sem_array.remove();
                self.linux_process().semaphores_remove(id);
                Ok(0)
            }
            SemctlCmds::IPC_SET => {
                // arg is struct semid_ds
                let ptr = UserInPtr::from(arg);
                let ds: SemidDs = ptr.read()?;
                // update IpcPerm
                sem_array.set(&ds);
                sem_array.ctime();
                Ok(0)
            }
            SemctlCmds::IPC_STAT => {
                // arg is struct semid_ds
                let mut ptr = UserOutPtr::from(arg);
                ptr.write(*sem_array.semid_ds.lock())?;
                Ok(0)
            }
            _ => {
                let sem = &sem_array[num as usize];
                match cmd {
                    SemctlCmds::GETPID => Ok(sem.get_pid()),
                    SemctlCmds::GETVAL => Ok(sem.get() as usize),
                    SemctlCmds::GETNCNT => Ok(sem.get_ncnt()),
                    SemctlCmds::GETZCNT => Ok(0),
                    SemctlCmds::SETVAL => {
                        sem.set(arg as isize);
                        sem.set_pid(self.zircon_process().id() as usize);
                        sem_array.ctime();
                        Ok(0)
                    }
                    _ => unimplemented!("Semaphore Semctl cmd: {:?}", cmd),
                }
            }
        }
    }
}

numeric_enum! {
    #[repr(usize)]
    #[derive(Debug, Eq, PartialEq)]
    #[allow(non_camel_case_types)]
    /// for the third argument of semctl(), specified the control operation
    pub enum SemctlCmds {
        /// Immediately remove the semaphore set, awakening all processes blocked
        IPC_RMID = 0,
        /// Write the values of some members of the semid_ds structure pointed to by arg
        IPC_SET = 1,
        /// Copy information from the kernel data structure associated with
        /// semid into the semid_ds structure pointed to by arg.buf.
        IPC_STAT = 2,
        /// Get the value of sempid
        GETPID = 11,
        /// Get the value of semval
        GETVAL = 12,
        /// Get semval for all semaphores of the set into arg.array
        GETALL = 13,
        /// Get the value of semncnt
        GETNCNT = 14,
        /// Get the value of semzcnt
        GETZCNT = 15,
        /// Set the value of semval to arg.val
        SETVAL = 16,
        /// Set semval for all semaphores of the set using arg.array
        SETALL = 17,
    }
}

/// An operation to be performed on a single semaphore
///
/// Ref: [http://man7.org/linux/man-pages/man2/semop.2.html]
#[repr(C)]
pub struct SemBuf {
    num: u16,
    op: i16,
    flags: i16,
}

/// for the fourth argument of semctl()
///
/// unused currently
pub union SemctlUnion {
    /// Value for SETVAL
    val: isize,
    /// Buffer for IPC_STAT, IPC_SET: type semid_ds
    buf: usize,
    /// Array for GETALL, SETALL
    array: usize,
}

bitflags! {
    pub struct SemFlags: i16 {
        /// For SemOP
        const IPC_NOWAIT = 0x800;
        /// it will be automatically undone when the process terminates.
        const SEM_UNDO = 0x1000;
    }
}
