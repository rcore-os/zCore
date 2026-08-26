//! POSIX advisory record locks — `fcntl(2)` F_GETLK / F_SETLK / F_SETLKW.
//!
//! One global table keyed by the file's `(dev, inode)` identity holds byte
//! ranges per owning process. Package managers (`apk add` uses a lock file,
//! dpkg's `/var/lib/dpkg/lock`) and databases take whole-file write locks
//! through this API and treat `ENOSYS` as either "no locking, race freely"
//! or a hard error — both bad answers.
//!
//! Two deliberate simplifications, both safe on this single-user system:
//!
//! * Locks are dropped when the owning **process dies** (a conflict scan
//!   ignores and prunes entries whose owner pid no longer resolves), not on
//!   the infamous POSIX "close of any fd to the file" trigger. Locks held by
//!   a live process that closed the file linger until it exits.
//! * Open-file-description locks (`F_OFD_*`) share the classic table with
//!   the process as owner, which collapses their one behavioural difference
//!   (per-description ownership) into per-process ownership.

use alloc::vec::Vec;
use hashbrown::HashMap;
use lazy_static::lazy_static;
use lock::Mutex;
use zircon_object::object::KoID;
use zircon_object::task::ROOT_JOB;

/// `l_type` values from `<fcntl.h>`.
pub const F_RDLCK: i16 = 0;
/// Exclusive (write) lock.
pub const F_WRLCK: i16 = 1;
/// Unlock.
pub const F_UNLCK: i16 = 2;

/// A file's identity in the lock table: `(st_dev, st_ino)`.
pub type FileKey = (usize, usize);

/// One held lock: `[start, end)` with `end == u64::MAX` meaning "to EOF and
/// beyond" (`l_len == 0`).
#[derive(Debug, Clone, Copy)]
struct Held {
    exclusive: bool,
    start: u64,
    end: u64,
    owner: KoID,
}

/// A lock request/probe after `l_whence` resolution.
#[derive(Debug, Clone, Copy)]
pub struct LockRequest {
    /// True for `F_WRLCK`, false for `F_RDLCK`.
    pub exclusive: bool,
    /// Absolute first byte.
    pub start: u64,
    /// One past the last byte; `u64::MAX` = unbounded (`l_len == 0`).
    pub end: u64,
    /// Owning process.
    pub owner: KoID,
}

/// What `F_GETLK` reports about a conflicting lock.
#[derive(Debug, Clone, Copy)]
pub struct ConflictInfo {
    /// `F_RDLCK` or `F_WRLCK`.
    pub type_: i16,
    /// Absolute first byte of the conflicting lock.
    pub start: u64,
    /// `l_len` encoding: 0 for unbounded, else the byte count.
    pub len: u64,
    /// Owner pid.
    pub pid: KoID,
}

lazy_static! {
    static ref LOCKS: Mutex<HashMap<FileKey, Vec<Held>>> = Mutex::new(HashMap::new());
}

fn ranges_overlap(a_start: u64, a_end: u64, b_start: u64, b_end: u64) -> bool {
    a_start < b_end && b_start < a_end
}

/// Two locks conflict when their ranges overlap, the owners differ, and at
/// least one side is exclusive — fcntl(2)'s compatibility rule.
fn conflicts(held: &Held, req: &LockRequest) -> bool {
    held.owner != req.owner
        && (held.exclusive || req.exclusive)
        && ranges_overlap(held.start, held.end, req.start, req.end)
}

/// Remove `[start, end)` from a held range, returning the surviving pieces
/// (0, 1 or 2). This is what lets an unlock or a re-lock punch a hole in the
/// middle of an existing lock, per POSIX.
fn subtract(held: &Held, start: u64, end: u64) -> Vec<Held> {
    let mut out = Vec::new();
    if !ranges_overlap(held.start, held.end, start, end) {
        out.push(*held);
        return out;
    }
    if held.start < start {
        out.push(Held {
            end: start,
            ..*held
        });
    }
    if held.end > end {
        out.push(Held {
            start: end,
            ..*held
        });
    }
    out
}

fn owner_alive(owner: KoID) -> bool {
    ROOT_JOB.find_process(owner).is_some()
}

/// Drop every entry whose owner no longer exists (exit-time cleanup, done
/// lazily on each table visit).
fn prune_dead(locks: &mut Vec<Held>) {
    locks.retain(|l| owner_alive(l.owner));
}

/// `F_GETLK`: describe the first lock blocking `req`, or `None` when the
/// request would succeed.
pub fn getlk(key: FileKey, req: &LockRequest) -> Option<ConflictInfo> {
    let mut table = LOCKS.lock();
    let locks = table.get_mut(&key)?;
    prune_dead(locks);
    locks
        .iter()
        .find(|held| conflicts(held, req))
        .map(|held| ConflictInfo {
            type_: if held.exclusive { F_WRLCK } else { F_RDLCK },
            start: held.start,
            len: if held.end == u64::MAX {
                0
            } else {
                held.end - held.start
            },
            pid: held.owner,
        })
}

/// `F_SETLK` (one attempt): acquire, convert or release `req`'s range.
/// Returns false on conflict — the caller answers `EAGAIN` or, for
/// `F_SETLKW`, retries. An acquisition first carves the owner's existing
/// locks out of the range (POSIX replace semantics), an unlock only carves.
pub fn setlk(key: FileKey, req: &LockRequest, unlock: bool) -> bool {
    let mut table = LOCKS.lock();
    let locks = table.entry(key).or_default();
    prune_dead(locks);
    if !unlock && locks.iter().any(|held| conflicts(held, req)) {
        return false;
    }
    let mut next: Vec<Held> = Vec::with_capacity(locks.len() + 1);
    for held in locks.iter() {
        if held.owner == req.owner {
            next.extend(subtract(held, req.start, req.end));
        } else {
            next.push(*held);
        }
    }
    if !unlock {
        next.push(Held {
            exclusive: req.exclusive,
            start: req.start,
            end: req.end,
            owner: req.owner,
        });
    }
    if next.is_empty() {
        table.remove(&key);
    } else {
        *table.get_mut(&key).unwrap() = next;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(exclusive: bool, start: u64, end: u64, owner: KoID) -> LockRequest {
        LockRequest {
            exclusive,
            start,
            end,
            owner,
        }
    }

    fn held(exclusive: bool, start: u64, end: u64, owner: KoID) -> Held {
        Held {
            exclusive,
            start,
            end,
            owner,
        }
    }

    #[test]
    fn read_locks_share_write_locks_exclude() {
        let shared = held(false, 0, 100, 1);
        let exclusive = held(true, 0, 100, 1);
        // Same owner never conflicts with itself.
        assert!(!conflicts(&exclusive, &req(true, 0, 100, 1)));
        // Two readers coexist.
        assert!(!conflicts(&shared, &req(false, 0, 100, 2)));
        // A writer excludes readers and writers alike.
        assert!(conflicts(&shared, &req(true, 0, 100, 2)));
        assert!(conflicts(&exclusive, &req(false, 0, 100, 2)));
        // Disjoint ranges never conflict.
        assert!(!conflicts(&exclusive, &req(true, 100, 200, 2)));
    }

    #[test]
    fn subtract_splits_middle_and_trims_edges() {
        let h = held(true, 10, 50, 1);
        // Hole in the middle → two pieces.
        let pieces = subtract(&h, 20, 30);
        assert_eq!(pieces.len(), 2);
        assert_eq!((pieces[0].start, pieces[0].end), (10, 20));
        assert_eq!((pieces[1].start, pieces[1].end), (30, 50));
        // Trim the head.
        let pieces = subtract(&h, 0, 20);
        assert_eq!(pieces.len(), 1);
        assert_eq!((pieces[0].start, pieces[0].end), (20, 50));
        // Full cover → nothing left.
        assert!(subtract(&h, 0, 100).is_empty());
        // Disjoint → untouched.
        let pieces = subtract(&h, 50, 60);
        assert_eq!(pieces.len(), 1);
        assert_eq!((pieces[0].start, pieces[0].end), (10, 50));
    }

    #[test]
    fn unbounded_ranges_use_u64_max_end() {
        let h = held(true, 100, u64::MAX, 1);
        assert!(conflicts(&h, &req(false, 1000, 1001, 2)));
        assert!(!conflicts(&h, &req(true, 0, 100, 2)));
    }
}
