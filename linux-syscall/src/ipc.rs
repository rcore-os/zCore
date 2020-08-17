#![allow(dead_code)]

use bitflags::*;

pub use linux_object::ipc::*;

use super::*;

impl Syscall<'_> {
    ///  returns the semaphore set identifier associated with the argument key
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
    pub async fn sys_semop(&self, id: usize, ops: UserInPtr<SemBuf>, num_ops: usize) -> SysResult {
        info!("semop: id: {}", id);
        let ops = ops.read_array(num_ops)?;

        let sem_array = self
            .linux_process()
            .semaphores_get(id)
            .ok_or(LxError::EINVAL)?;
        sem_array.otime();
        for &SemBuf { num, op, flags } in ops.iter() {
            let flags = SemFlags::from_bits_truncate(flags);
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

    /// performs the control operation specified by cmd on the semaphore set identified by semid
    pub fn sys_semctl(&self, id: usize, num: usize, cmd: usize, arg: usize) -> SysResult {
        info!(
            "semctl: id: {}, num: {}, cmd: {} arg: {:#x}",
            id, num, cmd, arg
        );
        let sem_array = self
            .linux_process()
            .semaphores_get(id)
            .ok_or(LxError::EINVAL)?;
        const IPC_RMID: usize = 0;
        const IPC_SET: usize = 1;
        const IPC_STAT: usize = 2;
        const GETPID: usize = 11;
        const GETVAL: usize = 12;
        const GETALL: usize = 13;
        const GETNCNT: usize = 14;
        const GETZCNT: usize = 15;
        const SETVAL: usize = 16;
        const SETALL: usize = 17;

        match cmd {
            IPC_RMID => {
                sem_array.remove();
                self.linux_process().semaphores_remove(id);
                Ok(0)
            }
            IPC_SET => {
                // arg is struct semid_ds
                let ptr = UserInPtr::from(arg);
                let ds: SemidDs = ptr.read()?;
                // update IpcPerm
                sem_array.set(&ds);
                sem_array.ctime();
                Ok(0)
            }
            IPC_STAT => {
                // arg is struct semid_ds
                let mut ptr = UserOutPtr::from(arg);
                ptr.write(*sem_array.semid_ds.lock())?;
                Ok(0)
            }
            _ => {
                let sem = &sem_array[num as usize];
                match cmd {
                    GETPID => Ok(sem.get_pid()),
                    GETVAL => Ok(sem.get() as usize),
                    GETNCNT => Ok(sem.get_ncnt()),
                    GETZCNT => Ok(0),
                    SETVAL => {
                        sem.set(arg as isize);
                        sem.set_pid(self.zircon_process().id() as usize);
                        sem_array.ctime();
                        Ok(0)
                    }
                    _ => unimplemented!("Semaphore Semctl cmd: {}", cmd),
                }
            }
        }
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

pub union SemctlUnion {
    val: isize,
    buf: usize,   // semid_ds*, unimplemented
    array: usize, // short*, unimplemented
} // unused

bitflags! {
    pub struct SemFlags: i16 {
        /// For SemOP
        const IPC_NOWAIT = 0x800;
        /// it will be automatically undone when the process terminates.
        const SEM_UNDO = 0x1000;
    }
}
