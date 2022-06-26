//! Linux Shared memory ipc
use super::*;
use crate::error::LxError;
use crate::time::TimeSpec;
use alloc::{collections::BTreeMap, sync::Arc, sync::Weak};
use lazy_static::lazy_static;
use lock::{Mutex, RwLock};
use zircon_object::vm::*;

lazy_static! {
    static ref KEY2SHM: RwLock<BTreeMap<u32, Weak<Mutex<ShmGuard>>>> = RwLock::new(BTreeMap::new());
}

/// shmid data structure
///
/// struct shmid_ds
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ShmidDs {
    /// Ownership and permissions
    pub perm: IpcPerm,
    /// Size of segment (bytes)
    pub segsz: usize,
    /// Last attach time
    pub atime: usize,
    /// Last detach time
    pub dtime: usize,
    /// Last change time
    pub ctime: usize,
    /// PID of creator
    pub cpid: u32,
    /// PID of last shmat(2)/shmdt(2)
    pub lpid: u32,
    /// number of current attaches
    pub nattch: usize,
}

/// shared memory Identifier for process
#[derive(Clone)]
pub struct ShmIdentifier {
    /// Shared memory address
    pub addr: usize,
    /// Shared memory buffer and data
    pub guard: Arc<Mutex<ShmGuard>>,
}

/// shared memory buffer and data
pub struct ShmGuard {
    /// shared memory buffer
    pub shared_guard: Arc<VmObject>,
    /// shared memory data
    pub shmid_ds: Mutex<ShmidDs>,
}

impl ShmIdentifier {
    /// set the shared memory address on attach
    pub fn set_addr(&mut self, addr: usize) {
        self.addr = addr;
    }

    /// Get or create a ShmGuard
    pub fn new_shared_guard(
        key: u32,
        memsize: usize,
        flags: usize,
        cpid: u32,
    ) -> Result<Arc<Mutex<ShmGuard>>, LxError> {
        let mut key2shm = KEY2SHM.write();
        let flag = IpcGetFlag::from_bits_truncate(flags);

        // found in the map
        if let Some(weak_guard) = key2shm.get(&key) {
            if let Some(guard) = weak_guard.upgrade() {
                if flag.contains(IpcGetFlag::CREAT) && flag.contains(IpcGetFlag::EXCLUSIVE) {
                    // exclusive
                    return Err(LxError::EEXIST);
                }
                return Ok(guard);
            }
        }
        let shared_guard = Arc::new(Mutex::new(ShmGuard {
            shared_guard: VmObject::new_paged(pages(memsize)),
            shmid_ds: Mutex::new(ShmidDs {
                perm: IpcPerm {
                    key,
                    uid: 0,
                    gid: 0,
                    cuid: 0,
                    cgid: 0,
                    // least significant 9 bits
                    mode: (flags as u32) & 0x1ff,
                    __seq: 0,
                    __pad1: 0,
                    __pad2: 0,
                },
                segsz: memsize,
                atime: 0,
                dtime: 0,
                ctime: TimeSpec::now().sec,
                cpid,
                lpid: 0,
                nattch: 0,
            }),
        }));
        // insert to global map
        key2shm.insert(key, Arc::downgrade(&shared_guard));
        Ok(shared_guard)
    }
}

impl ShmGuard {
    /// set last attach time
    pub fn attach(&self, pid: u32) {
        let mut ds = self.shmid_ds.lock();
        ds.atime = TimeSpec::now().sec;
        ds.nattch += 1;
        ds.lpid = pid;
    }

    /// set last detach time
    pub fn detach(&self, pid: u32) {
        let mut ds = self.shmid_ds.lock();
        ds.dtime = TimeSpec::now().sec;
        ds.nattch -= 1;
        ds.lpid = pid;
    }

    /// set last change time
    pub fn ctime(&self) {
        self.shmid_ds.lock().ctime = TimeSpec::now().sec;
    }

    /// for IPC_SET
    /// see man shmctl(2)
    pub fn set(&self, new: &ShmidDs) {
        let mut lock = self.shmid_ds.lock();
        lock.perm.uid = new.perm.uid;
        lock.perm.gid = new.perm.gid;
        lock.perm.mode = new.perm.mode & 0x1ff;
    }

    /// remove Shared memory
    pub fn remove(&self) {
        let mut key2shm = KEY2SHM.write();
        let key = self.shmid_ds.lock().perm.key;
        key2shm.remove(&key);
    }
}
