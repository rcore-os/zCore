//! Linux ELF Program Loader
#![deny(missing_docs)]

use {
    crate::error::LxResult,
    crate::fs::INodeExt,
    alloc::{collections::BTreeMap, string::String, sync::Arc, vec::Vec},
    rcore_fs::vfs::INode,
    xmas_elf::{program::ProgramHeader, ElfFile},
    zircon_object::{util::elf_loader::*, vm::*, ZxError},
};

mod abi;

/// Linux ELF Program Loader.
pub struct LinuxElfLoader {
    /// syscall entry
    pub syscall_entry: usize,
    /// stack page number
    pub stack_pages: usize,
    /// root inode of LinuxElfLoader
    pub root_inode: Arc<dyn INode>,
}

impl LinuxElfLoader {
    /// load a Linux ElfFile and return a tuple of (entry,sp)
    pub fn load(
        &self,
        vmar: &Arc<VmAddressRegion>,
        data: &[u8],
        mut args: Vec<String>,
        envs: Vec<String>,
        path: String,
    ) -> LxResult<(VirtAddr, VirtAddr)> {
        debug!(
            "load: vmar.addr & size: {:#x?}, data {:#x?}, args: {:?}, envs: {:?}",
            vmar.get_info(),
            data.as_ptr(),
            args,
            envs
        );

        let elf = ElfFile::new(data).map_err(|_| ZxError::INVALID_ARGS)?;

        debug!("elf info:  {:#x?}", elf.header.pt2);

        if let Ok(interp) = elf.get_interpreter() {
            info!("interp: {:?}", interp);
            let inode = self.root_inode.lookup(interp)?;
            let data = inode.read_as_vec()?;
            args[0] = path.clone();
            args.insert(0, interp.into());
            return self.load(vmar, &data, args, envs, path);
        }

        let size = elf.load_segment_size();
        let image_vmar = vmar.allocate(None, size, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)?;
        let mut base = image_vmar.addr();
        let vmo = image_vmar.load_from_elf(&elf)?;
        let entry = base + elf.header.pt2.entry_point() as usize;

        // for static exec program
        let ph: ProgramHeader = elf.program_iter().next().unwrap();
        let static_prog_base = ph.virtual_addr() as usize / PAGE_SIZE * PAGE_SIZE;
        debug!(
            "load: vmar.addr & size: {:#x?}, base: {:#x?}, entry: {:#x?}",
            vmar.get_info(),
            base,
            entry
        );

        // fill syscall entry
        if let Some(offset) = elf.get_symbol_address("rcore_syscall_entry") {
            vmo.write(offset as usize, &self.syscall_entry.to_ne_bytes())?;
        }

        match elf.relocate(image_vmar) {
            Ok(()) => info!("elf relocate passed !"),
            Err(error) => {
                base = static_prog_base;
                warn!("elf relocate Err:{:?}, base {:x?}", error, base);
            }
        }

        let stack_vmo = VmObject::new_paged(self.stack_pages);
        let flags = MMUFlags::READ | MMUFlags::WRITE | MMUFlags::USER;
        let stack_bottom = vmar.map(None, stack_vmo.clone(), 0, stack_vmo.len(), flags)?;
        let mut sp = stack_bottom + stack_vmo.len();
        debug!("load stack bottom: {:#x}", stack_bottom);

        let info = abi::ProcInitInfo {
            args,
            envs,
            auxv: {
                let mut map = BTreeMap::new();
                #[cfg(target_arch = "x86_64")]
                {
                    map.insert(abi::AT_BASE, base);
                    map.insert(abi::AT_PHDR, base + elf.header.pt2.ph_offset() as usize);
                    map.insert(abi::AT_ENTRY, entry);
                }
                #[cfg(target_arch = "riscv64")]
                if let Some(phdr_vaddr) = elf.get_phdr_vaddr() {
                    map.insert(abi::AT_PHDR, phdr_vaddr as usize);
                }
                map.insert(abi::AT_PHENT, elf.header.pt2.ph_entry_size() as usize);
                map.insert(abi::AT_PHNUM, elf.header.pt2.ph_count() as usize);
                map.insert(abi::AT_PAGESZ, PAGE_SIZE);
                map
            },
        };
        let init_stack = info.push_at(sp);
        stack_vmo.write(self.stack_pages * PAGE_SIZE - init_stack.len(), &init_stack)?;
        sp -= init_stack.len();

        debug!(
            "ProcInitInfo auxv: {:#x?}\nentry:{:#x}, sp:{:#x}",
            info.auxv, entry, sp
        );

        Ok((entry, sp))
    }
}
