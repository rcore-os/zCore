//! mount(2) and umount2(2)

use super::*;
use linux_object::fs::{mount_fs, umount_fs};

impl Syscall<'_> {
    /// Mount a filesystem.
    pub fn sys_mount(
        &self,
        source: UserInPtr<u8>,
        target: UserInPtr<u8>,
        fstype: UserInPtr<u8>,
        flags: usize,
        data: UserInPtr<u8>,
    ) -> SysResult {
        let source = source.as_c_str()?;
        let target = target.as_c_str()?;
        let fstype = fstype.as_c_str()?;
        let data = if data.is_null() { "" } else { data.as_c_str()? };
        info!(
            "mount: source={:?}, target={:?}, fstype={:?}, flags={:#x}",
            source, target, fstype, flags
        );
        mount_fs(self.linux_process(), source, target, fstype, flags, data)?;
        linux_object::fs::dcache_invalidate();
        Ok(0)
    }

    /// Unmount a filesystem.
    pub fn sys_umount2(&self, target: UserInPtr<u8>, flags: usize) -> SysResult {
        let target = target.as_c_str()?;
        info!("umount2: target={:?}, flags={:#x}", target, flags);
        umount_fs(target, flags)?;
        linux_object::fs::dcache_invalidate();
        Ok(0)
    }
}
