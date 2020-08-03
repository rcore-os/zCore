use super::*;
use bitflags::bitflags;
use zircon_object::vm::*;

impl Syscall<'_> {
    /// creates a new mapping in the virtual address space of the calling process.
    /// - `addr` - The starting address for the new mapping
    /// - `len` - specifies the length of the mapping
    /// - `prot ` - describes the desired memory protection of the mapping
    /// - `flags` - determines whether updates to the mapping are visible to other processes mapping the same region,
    ///     and whether updates are carried through to the underlying file.
    /// - `fd` - mapping file descriptor
    /// - `offset` - offset in the file
    pub async fn sys_mmap(
        &self,
        addr: usize,
        len: usize,
        prot: usize,
        flags: usize,
        fd: FileDesc,
        offset: u64,
    ) -> SysResult {
        let prot = MmapProt::from_bits(prot).ok_or(LxError::EINVAL)?;
        let flags = MmapFlags::from_bits(flags).ok_or(LxError::EINVAL)?;
        info!(
            "mmap: addr={:#x}, size={:#x}, prot={:?}, flags={:?}, fd={:?}, offset={:#x}",
            addr, len, prot, flags, fd, offset
        );

        let proc = self.zircon_process();
        let vmar = proc.vmar();

        if flags.contains(MmapFlags::FIXED) {
            // unmap first
            vmar.unmap(addr, len)?;
        }
        let vmar_offset = flags.contains(MmapFlags::FIXED).then(|| addr - vmar.addr());
        if flags.contains(MmapFlags::ANONYMOUS) {
            if flags.contains(MmapFlags::SHARED) {
                return Err(LxError::EINVAL);
            }
            let vmo = VmObject::new_paged(pages(len));
            let addr = vmar.map(vmar_offset, vmo.clone(), 0, vmo.len(), prot.to_flags())?;
            Ok(addr)
        } else {
            let file = self.linux_process().get_file(fd)?;
            let mut buf = vec![0; len];
            let len = file.read_at(offset, &mut buf).await?;
            let vmo = VmObject::new_paged(pages(len));
            vmo.write(0, &buf[..len])?;
            let addr = vmar.map(vmar_offset, vmo.clone(), 0, vmo.len(), prot.to_flags())?;
            Ok(addr)
        }
    }

    /// changes the access protections for the calling process's memory pages
    /// containing any part of the address range in the interval [addr, addr+len-1]
    /// TODO: unimplemented
    pub fn sys_mprotect(&self, addr: usize, len: usize, prot: usize) -> SysResult {
        let prot = MmapProt::from_bits(prot).ok_or(LxError::EINVAL)?;
        info!(
            "mprotect: addr={:#x}, size={:#x}, prot={:?}",
            addr, len, prot
        );
        warn!("mprotect: unimplemented");
        Ok(0)
    }

    /// Deletes the mappings for the specified address range, and causes further references to addresses
    /// within the range to generate invalid memory references.
    pub fn sys_munmap(&self, addr: usize, len: usize) -> SysResult {
        info!("munmap: addr={:#x}, size={:#x}", addr, len);
        let proc = self.thread.proc();
        let vmar = proc.vmar();
        vmar.unmap(addr, len)?;
        Ok(0)
    }
}

bitflags! {
    /// for the flag argument in mmap()
    pub struct MmapFlags: usize {
        #[allow(clippy::identity_op)]
        /// Changes are shared.
        const SHARED = 1 << 0;
        /// Changes are private.
        const PRIVATE = 1 << 1;
        /// Place the mapping at the exact address
        const FIXED = 1 << 4;
        /// The mapping is not backed by any file. (non-POSIX)
        const ANONYMOUS = MMAP_ANONYMOUS;
    }
}

/// MmapFlags `MMAP_ANONYMOUS` depends on arch
#[cfg(target_arch = "mips")]
const MMAP_ANONYMOUS: usize = 0x800;
#[cfg(not(target_arch = "mips"))]
const MMAP_ANONYMOUS: usize = 1 << 5;

bitflags! {
    /// for the prot argument in mmap()
    pub struct MmapProt: usize {
        #[allow(clippy::identity_op)]
        /// Data can be read
        const READ = 1 << 0;
        /// Data can be written
        const WRITE = 1 << 1;
        /// Data can be executed
        const EXEC = 1 << 2;
    }
}

impl MmapProt {
    /// convert MmapProt to MMUFlags
    fn to_flags(self) -> MMUFlags {
        let mut flags = MMUFlags::USER;
        if self.contains(MmapProt::READ) {
            flags |= MMUFlags::READ;
        }
        if self.contains(MmapProt::WRITE) {
            flags |= MMUFlags::WRITE;
        }
        if self.contains(MmapProt::EXEC) {
            flags |= MMUFlags::EXECUTE;
        }
        // FIXME: hack for unimplemented mprotect
        if self.is_empty() {
            flags = MMUFlags::READ | MMUFlags::WRITE;
        }
        flags
    }
}
