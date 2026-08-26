#![cfg_attr(not(any(test, feature = "std")), no_std)]

extern crate alloc;
#[macro_use]
extern crate log;

use alloc::{
    boxed::Box,
    collections::BTreeMap,
    string::String,
    sync::{Arc, Weak},
};
use core::{any::Any, future::Future, pin::Pin};
use rcore_fs::vfs::*;
use spin::RwLock;

/// The filesystem on which all the other filesystems are mounted
pub struct MountFS {
    /// The inner file system
    inner: Arc<dyn FileSystem>,
    /// All mounted children file systems
    mountpoints: RwLock<BTreeMap<INodeId, Arc<MountFS>>>,
    /// The mount point of this file system
    self_mountpoint: Option<Arc<MNode>>,
    /// Weak reference to self
    self_ref: Weak<MountFS>,
}

type INodeId = usize;

/// [diag] Magic stamped into every live `MNode` at construction and verified on
/// entry to hot `INode` methods (see `check_poison`). The "intermittent" kernel
/// corruption (`timeout -s TERM 1 sleep 5`) surfaces as a `#PF` in
/// `<MNode as INode>::metadata` dereferencing a garbage inner-inode vtable
/// (`0x87`). Checking this canary (plus the inode fat pointer) before the deref
/// lets the method return `EIO` instead of faulting, and logs whether the slot
/// was reused (UAF — `poison` clobbered to another allocation's data) or the
/// inode pointer alone was overwritten (targeted wild write, `poison` intact).
const MNODE_POISON: u64 = 0x4d4e_4f44_455f_4f4b; // "MNODE_OK"

/// INode for `MountFS`
///
/// [diag] `repr(C)` pins the field order so `check_poison` can inspect the raw
/// words at known offsets without dereferencing: word0 = `poison`, word1/2 =
/// the `inode` fat pointer (data ptr, vtable ptr). The recorded corruption
/// leaves `poison` intact but clobbers the inode's vtable word to a tiny value
/// (`0x87`), so validating that word — not just `poison` — is what catches it.
#[repr(C)]
pub struct MNode {
    /// [diag] Corruption canary — first field so a UAF/realloc or a wild write
    /// over the head of the struct clobbers it before the pointers.
    poison: u64,
    /// The inner INode
    inode: Arc<dyn INode>,
    /// Associated `MountFS`
    vfs: Arc<MountFS>,
    /// Weak reference to self
    self_ref: Weak<MNode>,
}

impl MNode {
    /// [defense-in-depth] Verify the corruption canary before dereferencing the
    /// inner inode. Called on entry to the hottest `INode` methods.
    ///
    /// The "intermittent" kernel corruption (`timeout -s TERM 1 sleep 5`) can
    /// leave an `MNode`'s inner `Arc<dyn INode>` fat pointer clobbered with a
    /// tiny value (`0x87`); the original code then called straight through that
    /// garbage vtable → `#PF` at `0x97` → (with the corruption already on the
    /// stack) a silent triple fault. This guard turns that machine-killing fault
    /// into a *recoverable* `EIO` for the one syscall that touched the bad node:
    /// the process gets an error, the kernel stays up. It also logs the exact
    /// clobber pattern (poison state + both halves of the inode fat pointer) so
    /// the corruption remains diagnosable.
    ///
    /// This does NOT fix the underlying wild write (whose primary victim is the
    /// executor stack, not these nodes — see `docs/README-crash-repro.md`); it
    /// is pure hardening of a known secondary crash site, in the spirit of the
    /// dedicated `#GP` IST stack.
    #[inline]
    fn check_poison(&self, who: &str) -> Result<()> {
        let raw = self as *const Self as *const u64;
        // SAFETY: self is a live &self reference; reading the first four words of
        // its own (repr(C)) storage is in-bounds even when the contents are
        // garbage. w0=poison, w1=inode.data, w2=inode.vtable, w3=vfs.
        let (w0, w1, w2, w3) = unsafe {
            (
                core::ptr::read_volatile(raw),
                core::ptr::read_volatile(raw.add(1)),
                core::ptr::read_volatile(raw.add(2)),
                core::ptr::read_volatile(raw.add(3)),
            )
        };
        // A live kernel pointer sits at 0xffff_ff00_0000_0000+ ; the corruption
        // sprays tiny values (0x01/0x87/…). Flag the inode fat pointer (data +
        // vtable) if either half is not a plausible kernel pointer — that is the
        // exact word (`0x87`) the recorded #PF dereferenced.
        let bad_kptr = |w: u64| w < 0xffff_8000_0000_0000;
        let poison_bad = w0 != MNODE_POISON;
        let inode_bad = bad_kptr(w1) || bad_kptr(w2);
        if poison_bad || inode_bad {
            error!(
                "[MNODE-CORRUPT] in {}: self={:p} poison={:#x} (want {:#x}) {} \
                 inode.data={:#x} inode.vtable={:#x} vfs={:#x} {} -> returning EIO \
                 instead of dereferencing (poison-intact + inode-bad == the wild \
                 write hit the inode fat pointer specifically, not a whole-slot UAF)",
                who, self, w0, MNODE_POISON,
                if poison_bad { "*POISON-CLOBBERED*" } else { "(poison ok)" },
                w1, w2, w3,
                if inode_bad { "*INODE-PTR-GARBAGE*" } else { "(inode ok)" },
            );
            return Err(FsError::DeviceError);
        }
        Ok(())
    }
}

impl MountFS {
    /// The filesystem mounted at this mount point (not nested children).
    pub fn inner_fs(&self) -> Arc<dyn FileSystem> {
        self.inner.clone()
    }

    /// Create a `MountFS` wrapper for file system `fs`
    pub fn new(fs: Arc<dyn FileSystem>) -> Arc<Self> {
        MountFS {
            inner: fs,
            mountpoints: RwLock::new(BTreeMap::new()),
            self_mountpoint: None,
            self_ref: Weak::default(),
        }
        .wrap()
    }

    /// Wrap pure `MountFS` with `Arc<..>`.
    fn wrap(self) -> Arc<Self> {
        let fs = Arc::new(self);
        let weak = Arc::downgrade(&fs);
        let ptr = Arc::into_raw(fs) as *mut Self;
        unsafe {
            (*ptr).self_ref = weak;
            Arc::from_raw(ptr)
        }
    }

    /// Strong type version of `root_inode`
    pub fn mountpoint_root_inode(&self) -> Arc<MNode> {
        MNode {
            poison: MNODE_POISON,
            inode: self.inner.root_inode(),
            vfs: self.self_ref.upgrade().unwrap(),
            self_ref: Weak::default(),
        }
        .wrap()
    }
}

impl MNode {
    fn wrap(self) -> Arc<Self> {
        let inode = Arc::new(self);
        let weak = Arc::downgrade(&inode);
        let ptr = Arc::into_raw(inode) as *mut Self;
        unsafe {
            (*ptr).self_ref = weak;
            Arc::from_raw(ptr)
        }
    }

    /// Mount file system `fs` at this INode
    pub fn mount(&self, fs: Arc<dyn FileSystem>) -> Result<Arc<MountFS>> {
        let metadata = self.inode.metadata()?;
        if metadata.type_ != FileType::Dir {
            return Err(FsError::NotDir);
        }
        if self.vfs.mountpoints.read().contains_key(&metadata.inode) {
            return Err(FsError::Busy);
        }
        let new_fs = MountFS {
            inner: fs,
            mountpoints: RwLock::new(BTreeMap::new()),
            self_mountpoint: Some(self.self_ref.upgrade().unwrap()),
            self_ref: Weak::default(),
        }
        .wrap();
        self.vfs
            .mountpoints
            .write()
            .insert(metadata.inode, new_fs.clone());
        Ok(new_fs)
    }

    /// Returns whether a child filesystem is mounted at this directory.
    pub fn is_mountpoint(&self) -> bool {
        let inode_id = self.inode.metadata().map(|m| m.inode).unwrap_or(0);
        self.vfs.mountpoints.read().contains_key(&inode_id)
    }

    /// Returns the mounted child filesystem, if any.
    pub fn mounted_inner_fs(&self) -> Option<Arc<dyn FileSystem>> {
        let inode_id = self.inode.metadata().ok()?.inode;
        self.vfs
            .mountpoints
            .read()
            .get(&inode_id)
            .map(|mfs| mfs.inner_fs())
    }

    /// Unmount a filesystem previously mounted at this directory.
    pub fn umount(&self) -> Result<()> {
        let inode_id = self.inode.metadata()?.inode;
        if self.vfs.mountpoints.write().remove(&inode_id).is_none() {
            return Err(FsError::InvalidParam);
        }
        Ok(())
    }

    fn overlaid_inode(&self) -> Arc<MNode> {
        let inode_id = self.metadata().unwrap().inode;
        if let Some(sub_vfs) = self.vfs.mountpoints.read().get(&inode_id) {
            sub_vfs.mountpoint_root_inode()
        } else {
            self.self_ref.upgrade().unwrap()
        }
    }

    fn is_mountpoint_root(&self) -> bool {
        self.inode.fs().root_inode().metadata().unwrap().inode
            == self.inode.metadata().unwrap().inode
    }

    /// Look up a direct child on the backing inode (no mount-overlay walk).
    pub fn backing_find(&self, name: &str) -> Result<Arc<Self>> {
        Ok(MNode::from_backing(self.vfs.clone(), self.inode.find(name)?))
    }

    /// Wrap a backing-store child inode without traversing mount overlays.
    pub fn from_backing(vfs: Arc<MountFS>, inode: Arc<dyn INode>) -> Arc<Self> {
        MNode {
            poison: MNODE_POISON,
            inode,
            vfs,
            self_ref: Weak::default(),
        }
        .wrap()
    }

    pub fn create(&self, name: &str, type_: FileType, mode: u32) -> Result<Arc<Self>> {
        Ok(MNode {
            poison: MNODE_POISON,
            inode: self.inode.create(name, type_, mode)?,
            vfs: self.vfs.clone(),
            self_ref: Weak::default(),
        }
        .wrap())
    }

    pub fn find(&self, root: bool, name: &str) -> Result<Arc<Self>> {
        match name {
            "" | "." => Ok(self.self_ref.upgrade().unwrap()),
            ".." => {
                if root {
                    Ok(self.self_ref.upgrade().unwrap())
                } else if self.is_mountpoint_root() {
                    match &self.vfs.self_mountpoint {
                        Some(inode) => inode.find(root, ".."),
                        None => Ok(self.self_ref.upgrade().unwrap()),
                    }
                } else {
                    Ok(MNode {
                        poison: MNODE_POISON,
                        inode: self.inode.find(name)?,
                        vfs: self.vfs.clone(),
                        self_ref: Weak::default(),
                    }
                    .wrap())
                }
            }
            _ => {
                let node = MNode {
                    poison: MNODE_POISON,
                    inode: self.inode.find(name)?,
                    vfs: self.vfs.clone(),
                    self_ref: Weak::default(),
                }
                .wrap();
                let inode_id = node.inode.metadata().map(|m| m.inode).unwrap_or(0);
                if let Some(sub_vfs) = self.vfs.mountpoints.read().get(&inode_id) {
                    Ok(sub_vfs.mountpoint_root_inode())
                } else {
                    Ok(node)
                }
            }
        }
    }

    pub fn find_name_by_child(&self, child: &Arc<MNode>) -> Result<String> {
        for index in 0.. {
            let name = self.inode.get_entry(index)?;
            match name.as_ref() {
                "." | ".." => {}
                _ => {
                    let queryback = self.find(false, &name)?.overlaid_inode();
                    if Arc::ptr_eq(&queryback.vfs, &child.vfs)
                        && queryback.inode.metadata()?.inode == child.inode.metadata()?.inode
                    {
                        return Ok(name);
                    }
                }
            }
        }
        Err(FsError::EntryNotFound)
    }
}

impl FileSystem for MountFS {
    fn sync(&self) -> Result<()> {
        self.inner.sync()?;
        for mount_fs in self.mountpoints.read().values() {
            mount_fs.sync()?;
        }
        Ok(())
    }

    fn root_inode(&self) -> Arc<dyn INode> {
        match &self.self_mountpoint {
            Some(inode) => inode.vfs.root_inode(),
            None => self.mountpoint_root_inode(),
        }
    }

    fn info(&self) -> FsInfo {
        self.inner.info()
    }
}

impl INode for MNode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        self.check_poison("read_at")?;
        self.inode.read_at(offset, buf)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize> {
        self.check_poison("write_at")?;
        self.inode.write_at(offset, buf)
    }

    fn poll(&self) -> Result<PollStatus> {
        self.inode.poll()
    }

    fn async_poll<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<PollStatus>> + Send + Sync + 'a>> {
        self.inode.async_poll()
    }

    fn metadata(&self) -> Result<Metadata> {
        self.check_poison("metadata")?;
        self.inode.metadata()
    }

    fn set_metadata(&self, metadata: &Metadata) -> Result<()> {
        self.inode.set_metadata(metadata)
    }

    fn sync_all(&self) -> Result<()> {
        self.inode.sync_all()
    }

    fn sync_data(&self) -> Result<()> {
        self.inode.sync_data()
    }

    fn resize(&self, len: usize) -> Result<()> {
        self.inode.resize(len)
    }

    fn create(&self, name: &str, type_: FileType, mode: u32) -> Result<Arc<dyn INode>> {
        Ok(self.create(name, type_, mode)?)
    }

    fn create2(
        &self,
        name: &str,
        type_: FileType,
        mode: u32,
        data: usize,
    ) -> Result<Arc<dyn INode>> {
        Ok(MNode {
            poison: MNODE_POISON,
            inode: self.inode.create2(name, type_, mode, data)?,
            vfs: self.vfs.clone(),
            self_ref: Weak::default(),
        }
        .wrap())
    }

    fn link(&self, name: &str, other: &Arc<dyn INode>) -> Result<()> {
        self.inode.link(name, other)
    }

    fn unlink(&self, name: &str) -> Result<()> {
        let inode_id = self.inode.find(name)?.metadata()?.inode;
        if self.vfs.mountpoints.read().contains_key(&inode_id) {
            return Err(FsError::Busy);
        }
        self.inode.unlink(name)
    }

    fn move_(&self, old_name: &str, target: &Arc<dyn INode>, new_name: &str) -> Result<()> {
        self.inode.move_(old_name, target, new_name)
    }

    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        self.check_poison("find")?;
        Ok(self.find(false, name)?)
    }

    fn get_entry(&self, id: usize) -> Result<String> {
        self.check_poison("get_entry")?;
        self.inode.get_entry(id)
    }

    fn get_entry_with_metadata(&self, id: usize) -> Result<(Metadata, String)> {
        self.inode.get_entry_with_metadata(id)
    }

    fn io_control(&self, cmd: u32, data: usize) -> Result<usize> {
        self.inode.io_control(cmd, data)
    }

    fn mmap(&self, area: MMapArea) -> Result<()> {
        self.inode.mmap(area)
    }

    fn fs(&self) -> Arc<dyn FileSystem> {
        self.vfs.clone()
    }

    fn as_any_ref(&self) -> &dyn Any {
        self.inode.as_any_ref()
    }
}
