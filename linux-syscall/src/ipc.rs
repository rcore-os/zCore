use super::*;
use bitflags::*;
use numeric_enum_macro::numeric_enum;
use zircon_object::vm::*;

pub use linux_object::ipc::*;

/// Syscalls of inter-process communication and System V semaphore Set operation.
///
/// # Menu
///
/// - [`semget`](Self::sys_semget)
/// - [`semop`](Self::sys_semop)
/// - [`semctl`](Self::sys_semctl)
/// - [`shmget`](Self::sys_shmget)
/// - [`shmat`](Self::sys_shmat)
/// - [`shmdt`](Self::sys_shmdt)
/// - [`shmctl`](Self::sys_shmctl)
impl Syscall<'_> {
    /// Get a System V semaphore set identifier
    /// (see [linux man semget(2)](https://www.man7.org/linux/man-pages/man2/semget.2.html)).
    ///
    /// The `sys_semget` system call returns
    /// the System V semaphore set identifier associated with the argument `key`.
    /// It may be used either to obtain the identifier of a previously created semaphore set
    /// (when `flags` is zero and `key` is not zero),
    /// or to create a new set.
    ///
    /// A new set of `nsems` (number of semaphores) semaphores is created if `key` is zero
    /// or if no existing semaphore set is associated with `key` and `IpcGetFlag::CREAT` is specified in `semflg`.
    ///
    /// If `flags` specifies both `IpcGetFlag::CREAT` and `IpcGetFlag::EXCLUSIVE`
    /// and a semaphore set already exists for key, then `sys_semget` fails with [`EEXIST`](LxError::EEXIST).
    /// (This is analogous to the effect of the combination `OpenFlags::CREATE | OpenFlags::EXCLUSIVE` for [`sys_open`](Self::sys_open).)
    ///
    /// Upon creation, the least significant 9 bits of the argument `flags` define
    /// the permissions (for owner, group, and others) for the semaphore set.
    /// These bits have the same format, and the same meaning, as the `mode` argument of [`sys_open`](Self::sys_open)
    /// (though the execute permissions are not meaningful for semaphores,
    /// and write permissions mean permission to alter semaphore values).
    ///
    /// When creating a new semaphore set, `sys_semget` initializes the set's associated data structure,
    /// semid_ds (see [`sys_semctl`](Self::sys_semctl)), as follows:
    ///
    /// - sem_perm.cuid and sem_perm.uid are set to the effective user ID of the calling process.
    /// - sem_perm.cgid and sem_perm.gid are set to the effective group ID of the calling process.
    /// - The least significant 9 bits of sem_perm.mode are set to the least significant 9 bits of `flags`.
    /// - sem_nsems is set to the value of nsems.
    /// - sem_otime is set to 0.
    /// - sem_ctime is set to the current time.
    ///
    /// The argument nsems can be 0 (a don't care) when a semaphore set is not being created.
    /// Otherwise, nsems must be greater than 0 and
    /// less than or equal to the maximum number of semaphores per semaphore set (SEMMSL, constant 256).
    ///
    /// If the semaphore set already exists, the permissions are verified.
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

    /// System V semaphore operations
    /// (see [linux man semop(2)](https://www.man7.org/linux/man-pages/man2/semop.2.html)).
    ///
    /// `semop` performs operations on selected semaphores in the set indicated by `id`.
    /// An array `[SemBuf; num_ops]` pointed to by `ops` specifies an operation to be performed on a single semaphore.
    /// The declaration of `SemBuf` is like this:
    ///
    /// ```rust
    /// struct SemBuf {
    ///    num: u16,
    ///    op: i16,
    ///    flags: i16,
    /// }
    /// ```
    ///
    /// Flags recognized in `SemBuf::flags` are `SemFlags::IPC_NOWAIT` and `SemFlags::SEM_UNDO`.
    /// If an operation specifies `SEM_UNDO`, it will be automatically undone when the process terminates.
    ///
    /// Each operation is performed on the `SemBuf::num`-th semaphore of the semaphore set,
    /// where the first semaphore of the set is numbered 0.
    /// There are two types of operation, distinguished by the value of `SemBuf::op`.
    ///
    /// - If `op` is +1, see [`acquire`](linux_object::sync::Semaphore::acquire).
    /// - If `op` is -1, see [`release`](linux_object::sync::Semaphore::release).
    pub async fn sys_semop(&self, id: usize, ops: UserInPtr<SemBuf>, num_ops: usize) -> SysResult {
        info!("semop: id: {}", id);
        let ops = ops.as_slice(num_ops)?;

        let sem_array = self
            .linux_process()
            .semaphores_get(id)
            .ok_or(LxError::EINVAL)?;
        sem_array.otime();
        for &SemBuf { num, op, flags } in ops {
            let flags = SemFlags::from_bits_truncate(flags);
            if flags.contains(SemFlags::IPC_NOWAIT) {
                unimplemented!("Semaphore: semop.IPC_NOWAIT");
            }
            let sem = &sem_array[num as usize];

            match op {
                1 => sem.release(),
                -1 => sem.acquire().await?,
                _ => unimplemented!("Semaphore: semop.(Not 1/-1)"),
            }
            sem.set_pid(self.zircon_process().id() as usize);
            if flags.contains(SemFlags::SEM_UNDO) {
                self.linux_process().semaphores_add_undo(id, num, op);
            }
        }
        Ok(0)
    }

    /// System V semaphore control operations
    /// (see [linux man semctl(2)](https://www.man7.org/linux/man-pages/man2/semctl.2.html)).
    ///
    /// `semctl` performs the control operation specified by cmd
    /// on the System V semaphore set identified by `id`,
    /// or on the `num`-th semaphore of that set
    /// (The semaphores in a set are numbered starting at 0).
    ///
    /// TODO
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

    /// Allocates a System V shared memory segment
    /// (see [linux man shmget(2)](https://www.man7.org/linux/man-pages/man2/shmget.2.html)).
    ///
    /// `shmget` returns the identifier of the System V shared memory segment
    /// associated with the value of the argument key.
    /// Differ from linux, this syscall always create a new set.
    pub fn sys_shmget(&self, key: usize, size: usize, shmflg: usize) -> SysResult {
        info!(
            "shmget: key: {}, size: {}, shmflg: {:#x}",
            key, size, shmflg
        );

        let shared_guard = ShmIdentifier::new_shared_guard(
            key as u32,
            size,
            shmflg,
            self.zircon_process().id() as u32,
        )?;
        let id = self.linux_process().shm_add(shared_guard);
        Ok(id)
    }

    /// System V shared memory operations
    /// (see [linux man shmat(2)](https://www.man7.org/linux/man-pages/man2/shmat.2.html)).
    ///
    /// `shmat` attaches the System V shared memory segment identified by `id`
    /// to the address space of the calling process.
    /// The attaching address is specified by `addr`.
    /// If `addr` is zero, the system chooses a suitable page-aligned address to attach the segment.
    pub fn sys_shmat(&self, id: usize, mut addr: VirtAddr, shmflg: usize) -> SysResult {
        let mut shm_identifier = self.linux_process().shm_get(id).ok_or(LxError::EINVAL)?;

        let proc = self.zircon_process();
        let vmar = proc.vmar();
        if addr == 0 {
            // although NULL can be a valid address
            // but in C, NULL is regarded as allocation failure
            // so just skip it
            addr = PAGE_SIZE;
        }
        let shm_guard = shm_identifier.guard.lock();
        let vmo = shm_guard.shared_guard.clone();
        info!(
            "shmat: id: {}, addr = {:#x}, size = {}, flags = {:#x}",
            id,
            addr,
            vmo.len(),
            shmflg
        );
        let addr = vmar.map(
            None,
            vmo.clone(),
            0,
            vmo.len(),
            MMUFlags::READ | MMUFlags::WRITE | MMUFlags::EXECUTE,
        )?;
        shm_identifier.addr = addr;
        self.linux_process().shm_set(id, shm_identifier.clone());

        shm_guard.attach(proc.id() as u32);
        Ok(addr)
    }

    /// System V shared memory operations
    /// (see [linux man shmdt(2)](https://www.man7.org/linux/man-pages/man2/shmdt.2.html)).
    ///
    /// `shmdt` detaches the shared memory segment located at the address specified by `addr`
    /// from the address space of the calling process.
    /// The to-be-detached segment must be currently attached with `addr`
    /// equal to the value returned by the attaching [`sys_shmat`](Self::sys_shmat) call.
    pub fn sys_shmdt(&self, id: usize, addr: VirtAddr, shmflg: usize) -> SysResult {
        info!(
            "shmdt: id = {}, addr = {:#x}, flag = {:#x}",
            id, addr, shmflg
        );
        let proc = self.linux_process();
        let opt_id = proc.shm_get_id(addr);
        if let Some(id) = opt_id {
            let shm_identifier = proc.shm_get(id).ok_or(LxError::EINVAL)?;
            proc.shm_pop(id);
            shm_identifier
                .guard
                .lock()
                .detach(self.zircon_process().id() as u32);
        }
        Ok(0)
    }

    /// System V shared memory operations
    /// (see [linux man shmctl(2)](https://www.man7.org/linux/man-pages/man2/shmctl.2.html)).
    ///
    /// performs the control operation specified by cmd on the shared memory segment whose identifier is given in id
    pub fn sys_shmctl(&self, id: usize, cmd: usize, buffer: usize) -> SysResult {
        info!("shmctl: id: {}, cmd: {} buffer: {:#x}", id, cmd, buffer);
        let shm_identifier = self.linux_process().shm_get(id).ok_or(LxError::EINVAL)?;
        let shm_guard = shm_identifier.guard.lock();
        let cmd = match ShmctlCmds::try_from(cmd) {
            Ok(t) => t,
            Err(_) => {
                error!("invalid semctl cmd: {}", cmd);
                return Err(LxError::EINVAL);
            }
        };
        match cmd {
            ShmctlCmds::IPC_RMID => {
                shm_guard.remove();
                self.linux_process().shm_pop(id);
                Ok(0)
            }
            ShmctlCmds::IPC_SET => {
                let buffer: UserInPtr<ShmidDs> = buffer.into();
                let set_ds = buffer.read()?;
                shm_guard.set(&set_ds);
                shm_guard.ctime();
                Ok(0)
            }
            ShmctlCmds::IPC_STAT | ShmctlCmds::SHM_STAT => {
                let shmid_ds = shm_guard.shmid_ds.lock();
                let mut buffer: UserOutPtr<ShmidDs> = buffer.into();
                buffer.write(*shmid_ds)?;
                Ok(0)
            }
            ShmctlCmds::SHM_INFO => {
                let mut buffer: UserOutPtr<ShmInfo> = buffer.into();
                buffer.write(ShmInfo::default())?;
                Ok(0)
            }
            _ => unimplemented!("Semaphore Semctl cmd: {:?}", cmd),
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

numeric_enum! {
    #[repr(usize)]
    #[derive(Debug, Eq, PartialEq)]
    #[allow(non_camel_case_types)]
    /// for the third argument of semctl(), specified the control operation
    pub enum ShmctlCmds {
        /// Mark the segment to be destroyed, actually be destroyed after the last process detaches it
        IPC_RMID = 0,
        /// Write the values of some members of the shmid_ds structure pointed to by arg
        IPC_SET = 1,
        /// Copy information from the kernel data structure associated with
        /// shmid into the shmid_ds structure pointed to by arg.buf.
        IPC_STAT = 2,
        /// Prevent swapping of the shared memory segment
        SHM_LOCK = 11,
        /// Unlock the segment, allowing it to be swapped out.
        SHM_UNLOCK = 12,
        /// Returns a shmid_ds structure as for IPC_STAT
        SHM_STAT = 13,
        /// Returns a shm_info structure whose fields contain information
        /// about system resources consumed by shared memory.
        SHM_INFO = 14,
    }
}

/// An operation to be performed on a single semaphore
///
/// Ref: <http://man7.org/linux/man-pages/man2/semop.2.html>
#[repr(C)]
pub struct SemBuf {
    num: u16,
    op: i16,
    flags: i16,
}

/// shm_info structure for shmctl
#[repr(C)]
#[derive(Default)]
struct ShmInfo {
    /// currently existing segments
    used_ids: i32,
    /// Total number of shared memory pages
    shm_tot: usize,
    /// of resident shared memory pages
    shm_rss: usize,
    /// of swapped shared memory pages
    shm_swp: usize,
}

bitflags! {
    pub struct SemFlags: i16 {
        /// For SemOP
        const IPC_NOWAIT = 0x800;
        /// it will be automatically undone when the process terminates.
        const SEM_UNDO = 0x1000;
    }
}
