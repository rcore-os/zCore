//! System V message queues (msgget/msgsnd/msgrcv/msgctl).
//!
//! Queues live in a global table and — unlike the `Weak`-held semaphore keys —
//! are kept alive until `IPC_RMID`, matching sysvipc(7): a producer may write
//! and exit before the consumer ever attaches. Ids are global, like Linux's.

use super::{IpcGetFlag, IpcPerm};
use crate::error::LxError;
use crate::time::TimeSpec;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use lock::{Mutex, RwLock};

/// Largest single message, bytes (`MSGMAX`, Documentation/admin-guide/sysctl/kernel.rst).
pub const MSGMAX: usize = 8192;
/// Default queue capacity, bytes (`MSGMNB`).
pub const MSGMNB: usize = 16384;

/// `struct msqid_ds` as 64-bit userland (musl/glibc `msqid64_ds`) lays it out.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MsqidDs {
    /// Ownership and permissions
    pub perm: IpcPerm,
    /// Time of last msgsnd
    pub stime: usize,
    /// Time of last msgrcv
    pub rtime: usize,
    /// Time of last change
    pub ctime: usize,
    /// Bytes currently queued
    pub cbytes: usize,
    /// Messages currently queued
    pub qnum: usize,
    /// Queue capacity in bytes
    pub qbytes: usize,
    /// pid of last msgsnd
    pub lspid: u32,
    /// pid of last msgrcv
    pub lrpid: u32,
    __unused: [usize; 2],
}

/// One queued message: its type tag and payload.
struct MsgItem {
    mtype: isize,
    data: Vec<u8>,
}

/// Outcome of a non-blocking send attempt.
pub enum MsgSendError {
    /// Queue was removed (`EIDRM` to the caller).
    Removed,
    /// Not enough room; a blocking caller waits and retries.
    Full,
}

/// Outcome of a non-blocking receive attempt.
pub enum MsgRecvError {
    /// Queue was removed (`EIDRM` to the caller).
    Removed,
    /// No message of the requested type; a blocking caller waits.
    NoMsg,
    /// First matching message is larger than the caller's buffer and
    /// `MSG_NOERROR` was not given (`E2BIG`).
    TooBig,
}

struct MsgQueueInner {
    ds: MsqidDs,
    messages: VecDeque<MsgItem>,
    /// Set by `IPC_RMID`: blocked senders/receivers wake into `EIDRM`.
    removed: bool,
}

/// A System V message queue.
pub struct MsgQueue {
    inner: Mutex<MsgQueueInner>,
}

lazy_static! {
    /// Global id → queue table. Strong references: queues survive their
    /// creator until `IPC_RMID`, per sysvipc(7).
    static ref MSG_QUEUES: RwLock<BTreeMap<usize, Arc<MsgQueue>>> =
        RwLock::new(BTreeMap::new());
}

impl MsgQueue {
    fn new(key: u32, mode: u32, uid: u32, gid: u32) -> Self {
        MsgQueue {
            inner: Mutex::new(MsgQueueInner {
                ds: MsqidDs {
                    perm: IpcPerm {
                        key,
                        uid,
                        gid,
                        cuid: uid,
                        cgid: gid,
                        mode: mode & 0o777,
                        __seq: 0,
                        __pad1: 0,
                        __pad2: 0,
                    },
                    stime: 0,
                    rtime: 0,
                    ctime: TimeSpec::now().sec,
                    cbytes: 0,
                    qnum: 0,
                    qbytes: MSGMNB,
                    lspid: 0,
                    lrpid: 0,
                    __unused: [0; 2],
                },
                messages: VecDeque::new(),
                removed: false,
            }),
        }
    }

    /// Try to append a message. `Full` asks the caller to block and retry
    /// (the payload is only copied in on success); bookkeeping
    /// (`cbytes`/`qnum`/`stime`/`lspid`) matches msgsnd(2).
    pub fn try_send(&self, mtype: isize, data: &[u8], sender: u32) -> Result<(), MsgSendError> {
        let mut inner = self.inner.lock();
        if inner.removed {
            return Err(MsgSendError::Removed);
        }
        // msgsnd(2) blocks when adding the message would exceed msg_qbytes.
        if inner.ds.cbytes + data.len() > inner.ds.qbytes {
            return Err(MsgSendError::Full);
        }
        inner.ds.cbytes += data.len();
        inner.ds.qnum += 1;
        inner.ds.stime = TimeSpec::now().sec;
        inner.ds.lspid = sender;
        inner.messages.push_back(MsgItem {
            mtype,
            data: data.to_vec(),
        });
        Ok(())
    }

    /// Try to take the first message matching `msgtyp` under msgrcv(2)'s
    /// selection rules; on success returns `(mtype, payload)` already
    /// truncated to `max_size` when `noerror` allows it.
    pub fn try_recv(
        &self,
        msgtyp: isize,
        max_size: usize,
        noerror: bool,
        except: bool,
        receiver: u32,
    ) -> Result<(isize, Vec<u8>), MsgRecvError> {
        let mut inner = self.inner.lock();
        if inner.removed {
            return Err(MsgRecvError::Removed);
        }
        let idx = select_message(inner.messages.iter().map(|m| m.mtype), msgtyp, except)
            .ok_or(MsgRecvError::NoMsg)?;
        if inner.messages[idx].data.len() > max_size && !noerror {
            return Err(MsgRecvError::TooBig);
        }
        let mut msg = inner.messages.remove(idx).unwrap();
        inner.ds.cbytes -= msg.data.len();
        inner.ds.qnum -= 1;
        inner.ds.rtime = TimeSpec::now().sec;
        inner.ds.lrpid = receiver;
        msg.data.truncate(max_size);
        Ok((msg.mtype, msg.data))
    }

    /// `IPC_STAT`: snapshot the queue's `msqid_ds`.
    pub fn stat(&self) -> MsqidDs {
        self.inner.lock().ds
    }

    /// `IPC_SET`: owner/permission bits and `msg_qbytes`, per msgctl(2).
    pub fn set(&self, new: &MsqidDs) {
        let mut inner = self.inner.lock();
        inner.ds.perm.uid = new.perm.uid;
        inner.ds.perm.gid = new.perm.gid;
        inner.ds.perm.mode = new.perm.mode & 0o777;
        inner.ds.qbytes = new.qbytes;
        inner.ds.ctime = TimeSpec::now().sec;
    }

    fn mark_removed(&self) {
        self.inner.lock().removed = true;
    }

    fn key(&self) -> u32 {
        self.inner.lock().ds.perm.key
    }
}

/// msgrcv(2) message selection, factored out for unit testing:
/// - `msgtyp == 0`: first message;
/// - `msgtyp > 0`: first with that exact type — or, with `MSG_EXCEPT`, the
///   first with a *different* type;
/// - `msgtyp < 0`: the message with the lowest type ≤ `|msgtyp|`.
fn select_message(
    mut types: impl Iterator<Item = isize>,
    msgtyp: isize,
    except: bool,
) -> Option<usize> {
    if msgtyp == 0 {
        return types.next().map(|_| 0);
    }
    if msgtyp > 0 {
        if except {
            types
                .enumerate()
                .find(|&(_, t)| t != msgtyp)
                .map(|(i, _)| i)
        } else {
            types
                .enumerate()
                .find(|&(_, t)| t == msgtyp)
                .map(|(i, _)| i)
        }
    } else {
        let bound = -msgtyp;
        types
            .enumerate()
            .filter(|&(_, t)| t <= bound)
            .min_by_key(|&(_, t)| t)
            .map(|(i, _)| i)
    }
}

/// `msgget(2)`: resolve `key` to a queue id, creating the queue as the flags
/// demand. `key == 0` is `IPC_PRIVATE` (always a fresh queue).
pub fn msg_get(key: u32, flags: usize, uid: u32, gid: u32) -> Result<usize, LxError> {
    let flag = IpcGetFlag::from_bits_truncate(flags);
    let mut queues = MSG_QUEUES.write();
    if key != 0 {
        if let Some((&id, _)) = queues.iter().find(|(_, q)| q.key() == key) {
            if flag.contains(IpcGetFlag::CREAT) && flag.contains(IpcGetFlag::EXCLUSIVE) {
                return Err(LxError::EEXIST);
            }
            return Ok(id);
        }
        if !flag.contains(IpcGetFlag::CREAT) {
            return Err(LxError::ENOENT);
        }
    }
    let id = (0..).find(|i| !queues.contains_key(i)).unwrap();
    queues.insert(id, Arc::new(MsgQueue::new(key, flags as u32, uid, gid)));
    Ok(id)
}

/// Look up a queue by id.
pub fn msg_queue(id: usize) -> Option<Arc<MsgQueue>> {
    MSG_QUEUES.read().get(&id).cloned()
}

/// `IPC_RMID`: drop the queue from the table and wake blocked callers into
/// `EIDRM` via the `removed` latch (their `Arc` keeps the object alive until
/// they notice).
pub fn msg_remove(id: usize) -> Result<(), LxError> {
    let queue = MSG_QUEUES.write().remove(&id).ok_or(LxError::EINVAL)?;
    queue.mark_removed();
    Ok(())
}

/// `/proc/sysvipc/msg` (Documentation/filesystems/proc.rst): one line per
/// queue in the kernel's column layout, consumed by `ipcs -q`.
pub fn msg_proc_table() -> alloc::string::String {
    use core::fmt::Write as _;
    let mut out = alloc::string::String::from(
        "       key      msqid perms      cbytes       qnum lspid lrpid   uid   gid  cuid  cgid      stime      rtime      ctime\n",
    );
    for (id, queue) in MSG_QUEUES.read().iter() {
        let ds = queue.stat();
        let _ = writeln!(
            out,
            "{:>10} {:>10} {:>5o} {:>11} {:>10} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>10} {:>10} {:>10}",
            ds.perm.key as i32,
            id,
            ds.perm.mode,
            ds.cbytes,
            ds.qnum,
            ds.lspid,
            ds.lrpid,
            ds.perm.uid,
            ds.perm.gid,
            ds.perm.cuid,
            ds.perm.cgid,
            ds.stime,
            ds.rtime,
            ds.ctime,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_first_message_for_type_zero() {
        assert_eq!(select_message([3, 1, 2].iter().copied(), 0, false), Some(0));
        assert_eq!(select_message(core::iter::empty(), 0, false), None);
    }

    #[test]
    fn select_exact_type_and_except() {
        let types = || [3isize, 1, 2].iter().copied();
        assert_eq!(select_message(types(), 2, false), Some(2));
        assert_eq!(select_message(types(), 9, false), None);
        // MSG_EXCEPT: first message whose type differs.
        assert_eq!(select_message(types(), 3, true), Some(1));
    }

    #[test]
    fn select_negative_takes_lowest_type_within_bound() {
        let types = || [5isize, 3, 4, 1].iter().copied();
        // |msgtyp| = 4: candidates {3,4,1}, lowest type wins (1, index 3).
        assert_eq!(select_message(types(), -4, false), Some(3));
        assert_eq!(select_message(types(), -1, false), Some(3));
        assert_eq!(select_message([5isize].iter().copied(), -4, false), None);
    }
}
