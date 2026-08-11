//! Linux ELF Program Loader
#![deny(missing_docs)]

use {
    crate::error::LxResult,
    crate::fs::INodeExt,
    crate::process::Abi,
    alloc::{collections::BTreeMap, string::String, sync::Arc, vec::Vec},
    rcore_fs::vfs::INode,
    xmas_elf::ElfFile,
    zircon_object::{util::elf_loader::*, vm::*, ZxError},
};

/// `__FreeBSD_version` advertised through `AT_OSRELDATE` (FreeBSD 14.0). Kept in
/// step with the value the syscall layer reports via `sysctl`.
#[cfg(target_arch = "x86_64")]
const FREEBSD_OSRELDATE: usize = 1_400_097;

/// Detect the ABI personality of an ELF image from its header and notes.
///
/// The primary signal is `EI_OSABI == ELFOSABI_FREEBSD` (9), which FreeBSD's
/// toolchain stamps on static executables. As a fallback — some FreeBSD
/// binaries leave `EI_OSABI` at `SYSV` and instead carry a `PT_NOTE` whose
/// vendor name is "FreeBSD" — the note segments are scanned for that name.
/// Non-x86_64 targets always report [`Abi::Linux`]: the ABI this kernel
/// implements is FreeBSD/amd64.
fn detect_abi(data: &[u8], _elf: &ElfFile) -> Abi {
    #[cfg(target_arch = "x86_64")]
    {
        const ELFOSABI_FREEBSD: u8 = 9;
        if data.get(7) == Some(&ELFOSABI_FREEBSD) {
            return Abi::Freebsd;
        }
        for ph in _elf.program_iter() {
            if ph.get_type() == Ok(xmas_elf::program::Type::Note) {
                let off = ph.offset() as usize;
                let end = off.saturating_add(ph.file_size() as usize);
                if let Some(seg) = data.get(off..end.min(data.len())) {
                    if seg.windows(7).any(|w| w == b"FreeBSD") {
                        return Abi::Freebsd;
                    }
                }
            }
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = data;
    }
    Abi::Linux
}

/// Stack top: place the user stack at the very top of the user address space so
/// that the heap (at `initial_brk` just after the loaded image) never collides
/// with the stack.  Linux uses a similar high-address default for the stack.
const STACK_TOP: usize = USER_ASPACE_BASE as usize + USER_ASPACE_SIZE as usize;

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
    /// load a Linux ElfFile and return a tuple of (entry, sp, brk)
    ///
    /// `brk` is the initial program break (end of the loaded image, page-aligned).
    /// Callers should store it on the process with `proc.linux().set_brk(brk)`.
    /// load a Linux ElfFile and return a tuple of (entry, sp, brk)
    ///
    /// `brk` is the initial program break (end of the loaded image, page-aligned).
    /// Callers should store it on the process with `proc.linux().set_brk(brk)`.
    pub fn load(
        &self,
        vmar: &Arc<VmAddressRegion>,
        vmo: &Arc<VmObject>,
        args: Vec<String>,
        envs: Vec<String>,
        path: String,
    ) -> LxResult<(VirtAddr, VirtAddr, usize, String, Abi)> {
        let size = zircon_object::vm::roundup_pages(vmo.len());
        let virt_addr = zircon_object::vm::KERNEL_ASPACE.map(
            None,
            vmo.clone(),
            0,
            size,
            zircon_object::vm::MMUFlags::READ | zircon_object::vm::MMUFlags::WRITE,
        )?;
        let data = unsafe { core::slice::from_raw_parts(virt_addr as *const u8, vmo.len()) };

        let res = self.load_impl(vmar, data, args, envs, path, 0);

        zircon_object::vm::KERNEL_ASPACE.unmap(virt_addr, size)?;
        res
    }

    /// Maximum number of interpreter levels (shebang + ELF PT_INTERP combined).
    const MAX_INTERP_DEPTH: usize = 4;

    /// Internal recursive loader that tracks interpreter depth.
    fn load_impl(
        &self,
        vmar: &Arc<VmAddressRegion>,
        data: &[u8],
        args: Vec<String>,
        envs: Vec<String>,
        path: String,
        recursion: u8,
    ) -> LxResult<(VirtAddr, VirtAddr, usize, String, Abi)> {
        debug!(
            "elf: load_impl recursion={} len={:#x} path={:?}",
            recursion,
            data.len(),
            path
        );
        debug!(
            "load: vmar.addr & size: {:#x?}, data {:#x?}, args: {:?}, envs: {:?}",
            vmar.get_info(),
            data.as_ptr(),
            args,
            envs
        );

        if recursion as usize > Self::MAX_INTERP_DEPTH {
            error!("load: interpreter chain too deep (depth={})", recursion);
            return Err(ZxError::INVALID_ARGS.into());
        }

        // Handle shebang scripts (#!).
        // Limit scan to the first 512 bytes to match typical OS shebang length restrictions.
        if data.starts_with(b"\x7fELF") {
            debug!("elf: detected ELF for {:?}", path);
            if data.len() < 64 {
                error!("elf: truncated header for {:?}", path);
                return Err(ZxError::INVALID_ARGS.into());
            }
        } else if data.starts_with(b"#!") {
            debug!("elf: detected shebang for {:?}", path);
            let scan_limit = data.len().min(512);
            let newline = data[..scan_limit]
                .iter()
                .position(|&b| b == b'\n')
                .unwrap_or(scan_limit);
            let line = core::str::from_utf8(&data[2..newline])
                .map_err(|_| ZxError::INVALID_ARGS)?
                .trim_end_matches('\r')
                .trim();
            // Split only on ASCII space/tab (POSIX shebang convention).
            let mut parts = line.splitn(2, [' ', '\t']);
            let interp = match parts.next() {
                Some(i) if !i.is_empty() => i,
                _ => return Err(ZxError::INVALID_ARGS.into()),
            };
            let interp_arg = parts.next().map(|s| s.trim()).filter(|s| !s.is_empty());
            debug!(
                "shebang: interp={:?}, arg={:?}, script={:?}",
                interp, interp_arg, path
            );
            // hunter P7: audit the shebang interpreter through the exec-path
            // policy so `#!/tmp/evil` is recorded (or blocked in Enforce). Use
            // the path-only check — the interpreter may itself be a script, for
            // which ELF-magic validation would be inappropriate.
            if !hunter::check_exec_path(interp) {
                return Err(ZxError::ACCESS_DENIED.into());
            }
            let interp_rel = interp.trim_start_matches('/');
            let inode = self.root_inode.lookup_follow(interp_rel, 1).map_err(|e| {
                error!("shebang: lookup interp {:?} failed: {:?}", interp_rel, e);
                e
            })?;
            let interp_vmo = inode.read_as_vmo_cached().map_err(|e| {
                error!("shebang: read interp {:?} failed: {:?}", interp_rel, e);
                e
            })?;
            let interp_size = zircon_object::vm::roundup_pages(interp_vmo.len());
            let interp_virt = zircon_object::vm::KERNEL_ASPACE.map(
                None,
                interp_vmo.clone(),
                0,
                interp_size,
                zircon_object::vm::MMUFlags::READ | zircon_object::vm::MMUFlags::WRITE,
            )?;
            let interp_data =
                unsafe { core::slice::from_raw_parts(interp_virt as *const u8, interp_vmo.len()) };

            let interp_path: String = interp.into();
            let mut new_args = vec![interp_path.clone()];
            if let Some(arg) = interp_arg {
                new_args.push(arg.into());
            }
            new_args.push(path);
            new_args.extend_from_slice(args.get(1..).unwrap_or_default());
            let res = self.load_impl(
                vmar,
                interp_data,
                new_args,
                envs,
                interp_path,
                recursion + 1,
            );

            zircon_object::vm::KERNEL_ASPACE.unmap(interp_virt, interp_size)?;
            return res;
        }

        let elf = ElfFile::new(data).map_err(|e| {
            error!("elf: ElfFile::new failed for {:?}: {:?}", path, e);
            ZxError::INVALID_ARGS
        })?;

        debug!("elf info:  {:#x?}", elf.header.pt2);

        // Which OS ABI does this image speak? Consulted below to build the right
        // initial stack, and returned so the caller can set the process's
        // syscall personality.
        let abi = detect_abi(data, &elf);
        if abi == Abi::Freebsd {
            info!("elf: detected FreeBSD ABI for {:?}", path);
        }

        if let Ok(interp) = elf.get_interpreter() {
            info!("interp: {:?}, path: {:?}", interp, path);

            // A PIE (ET_DYN) executable bases its LOAD segments at vaddr 0.
            // Loading one at VMAR offset 0 maps its first page — and the whole
            // low range including the null page — at address 0. That breaks the
            // null-pointer-faults invariant that libc and allocators such as
            // Firefox's mozjemalloc rely on, and puts AT_PHDR at a near-null
            // address. Reserve the low range with a guard sub-VMAR so the
            // program, interpreter, mmap and stack all load above it and any
            // access below it faults, matching Linux's non-zero PIE load base.
            // A non-PIE (ET_EXEC) binary already carries high absolute vaddrs
            // (0x400000+ on x86-64), so it needs no bias and gets none.
            const PIE_LOAD_BASE: usize = 0x40_0000;
            let is_pie = elf.header.pt2.type_().as_type() == xmas_elf::header::Type::SharedObject;
            if is_pie {
                vmar.allocate_at(0, PIE_LOAD_BASE, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                    .inspect_err(|&e| {
                        error!("elf: reserve PIE low-address guard failed: {:?}", e);
                    })?;
            }

            // Load the main program into the first free sub-VMAR. With the PIE
            // guard in place this lands at PIE_LOAD_BASE; for a non-PIE binary
            // there is no guard and app_base is 0 (segments carry their own
            // absolute vaddrs).
            let app_size = elf.load_segment_size();
            let app_vmar = vmar
                .allocate(None, app_size, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .inspect_err(|&e| {
                    error!(
                        "elf: allocate vmar for app size {:#x} failed: {:?}",
                        app_size, e
                    );
                })?;
            let app_base = app_vmar.addr();
            let _app_vmo = app_vmar.load_from_elf(&elf).inspect_err(|&e| {
                error!("elf: load app from elf failed: {:?}", e);
            })?;
            let app_entry = app_base + elf.header.pt2.entry_point() as usize;

            // Patch any in-binary syscall-entry trampoline present in the main program.
            // Write through the VMAR (which resolves the per-segment VMO): the symbol
            // usually lives in .data/.rodata, NOT in the first LOAD segment's VMO.
            if let Some(offset) = elf.get_symbol_address("rcore_syscall_entry") {
                app_vmar.write_memory(
                    app_base + offset as usize,
                    &self.syscall_entry.to_ne_bytes(),
                )?;
            }

            // Load the interpreter (ld.so) into a second sub-VMAR placed right after the
            // main program.  Because app_vmar occupies [0, app_size), the allocator places
            // interp_vmar at interp_base = app_size (> 0).
            //
            // A non-zero AT_BASE tells musl/glibc it is running as a PT_INTERP interpreter
            // rather than in standalone mode.  In interpreter mode the dynamic linker uses
            // the already-kernel-mapped binary via AT_PHDR / AT_ENTRY instead of calling
            // mmap() from user space to re-load it – which is the path that breaks in the
            // fork+execve case and causes a page fault at the raw e_entry (e.g. 0x423a7).
            // hunter P7: audit the dynamic linker (PT_INTERP) through the
            // exec-path policy so a `/tmp/ld.so` interpreter is recorded (or
            // blocked in Enforce) before it is mapped and executed.
            if !hunter::check_exec_path(interp) {
                return Err(ZxError::ACCESS_DENIED.into());
            }
            let interp_rel = interp.trim_start_matches('/');
            let inode = self.root_inode.lookup_follow(interp_rel, 4).map_err(|e| {
                error!(
                    "elf: lookup PT_INTERP {:?} failed: {:?} (check if file exists in rootfs)",
                    interp, e
                );
                e
            })?;
            let interp_vmo = inode.read_as_vmo_cached().map_err(|e| {
                error!("elf: read interp {:?} failed: {:?}", interp, e);
                e
            })?;
            let interp_size_aligned = zircon_object::vm::roundup_pages(interp_vmo.len());
            let interp_virt = zircon_object::vm::KERNEL_ASPACE.map(
                None,
                interp_vmo.clone(),
                0,
                interp_size_aligned,
                zircon_object::vm::MMUFlags::READ | zircon_object::vm::MMUFlags::WRITE,
            )?;
            let interp_data =
                unsafe { core::slice::from_raw_parts(interp_virt as *const u8, interp_vmo.len()) };

            let interp_elf = ElfFile::new(interp_data).map_err(|_| {
                error!("elf: interp {:?} is not a valid ELF", interp);
                ZxError::INVALID_ARGS
            })?;
            let interp_size = interp_elf.load_segment_size();
            let interp_vmar = vmar
                .allocate(None, interp_size, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
                .inspect_err(|&e| {
                    error!(
                        "elf: allocate vmar for interp {:?} size {:#x} failed: {:?}",
                        interp, interp_size, e
                    );
                })?;
            let interp_base = interp_vmar.addr();
            let _interp_vmo = interp_vmar.load_from_elf(&interp_elf).inspect_err(|&e| {
                error!("elf: load interp {:?} from elf failed: {:?}", interp, e);
            })?;
            let interp_entry = interp_base + interp_elf.header.pt2.entry_point() as usize;

            match interp_elf.relocate(interp_vmar, vmar) {
                Ok(()) => info!("interp relocate passed!"),
                Err(e) => {
                    debug!(
                        "interp relocate Err: {:?}, keeping base {:#x}",
                        e, interp_base
                    )
                }
            }

            zircon_object::vm::KERNEL_ASPACE.unmap(interp_virt, interp_size_aligned)?;

            let stack_vmo = VmObject::new_paged(self.stack_pages);
            let stack_flags = MMUFlags::READ | MMUFlags::WRITE | MMUFlags::USER;
            // Place the stack at the top of the process VMAR (which equals the
            // top of the user address space on bare metal, but is a smaller
            // window in aspace-separate/libos builds) so the heap, which grows
            // up from initial_brk, never collides with the stack.
            let stack_top = vmar.end_addr().min(STACK_TOP);
            let stack_bottom = stack_top - stack_vmo.len();
            // map_range=false: don't commit all 128 stack pages eagerly on every
            // exec — the argv/env/auxv tail is committed by the `stack_vmo.write`
            // below and the rest demand-zeroes on first touch like any anon
            // mmap. Eager commit cost ~0.5 MB zero-fill + 128 PTE installs per
            // spawn, and every later fork re-walked those committed pages.
            vmar.map_ext(
                Some(stack_bottom - vmar.addr()),
                stack_vmo.clone(),
                0,
                stack_vmo.len(),
                MMUFlags::RXW,
                stack_flags,
                false,
                false,
            )?;
            let mut sp = stack_top;
            // The vDSO is a Linux ABI object. A FreeBSD binary gets a
            // FreeBSD-shaped stack that never carries `AT_SYSINFO_EHDR`, so
            // mapping it there would leave an executable region in the address
            // space that nothing can ever reach.
            let vdso_base = (abi == Abi::Linux)
                .then(|| crate::vdso::map_into(vmar, stack_bottom))
                .flatten();

            let info = abi::ProcInitInfo {
                args,
                envs,
                auxv: {
                    let mut map = BTreeMap::new();
                    // AT_SYSINFO_EHDR: where the C library looks for the vDSO.
                    // Absent when this kernel has none, which is how musl is
                    // told to keep issuing the syscall.
                    if let Some(base) = vdso_base {
                        map.insert(crate::vdso::AT_SYSINFO_EHDR, base);
                    }
                    #[cfg(target_arch = "x86_64")]
                    {
                        // AT_BASE: interpreter load address; non-zero triggers interpreter
                        // mode in musl/glibc.
                        map.insert(abi::AT_BASE, interp_base);
                        // AT_PHDR: virtual address of the main program's program-header
                        // table in memory.  Use get_phdr_vaddr() which handles both PIE
                        // (vaddr relative to load base) and non-PIE (absolute vaddr)
                        // correctly, unlike the raw ph_offset() file field.
                        let phdr_vaddr =
                            elf.get_phdr_vaddr().unwrap_or(elf.header.pt2.ph_offset()) as usize;
                        map.insert(abi::AT_PHDR, app_base + phdr_vaddr);
                        // AT_ENTRY: main program's entry point.
                        map.insert(abi::AT_ENTRY, app_entry);
                    }
                    #[cfg(target_arch = "riscv64")]
                    {
                        map.insert(abi::AT_BASE, interp_base);
                        map.insert(abi::AT_ENTRY, app_entry);
                        if let Some(phdr_vaddr) = elf.get_phdr_vaddr() {
                            map.insert(abi::AT_PHDR, app_base + phdr_vaddr as usize);
                        }
                    }
                    #[cfg(target_arch = "aarch64")]
                    {
                        map.insert(abi::AT_BASE, interp_base);
                        map.insert(abi::AT_ENTRY, app_entry);
                        if let Some(phdr_vaddr) = elf.get_phdr_vaddr() {
                            map.insert(abi::AT_PHDR, app_base + phdr_vaddr as usize);
                        }
                    }
                    map.insert(abi::AT_PHENT, elf.header.pt2.ph_entry_size() as usize);
                    map.insert(abi::AT_PHNUM, elf.header.pt2.ph_count() as usize);
                    map.insert(abi::AT_PAGESZ, PAGE_SIZE);
                    // Identity + AT_SECURE block. musl computes `libc.secure`
                    // at startup as "AT_UID/EUID/GID/EGID not all present, or
                    // ruid != euid, or rgid != egid, or AT_SECURE != 0"; glib's
                    // g_check_setuid() treats an unreadable AT_SECURE the same
                    // way. Omitting these made EVERY process run in secure
                    // mode: musl silently dropped LD_PRELOAD/LD_LIBRARY_PATH
                    // and GLib refused to autolaunch a D-Bus session bus
                    // ("Cannot spawn a message bus when AT_SECURE is set",
                    // which killed waybar). Everything runs as root (uid 0)
                    // and nothing is setuid, so publish 0s explicitly.
                    map.insert(abi::AT_UID, 0usize);
                    map.insert(abi::AT_EUID, 0usize);
                    map.insert(abi::AT_GID, 0usize);
                    map.insert(abi::AT_EGID, 0usize);
                    map.insert(abi::AT_SECURE, 0usize);
                    map
                },
            };
            let init_stack = info.push_at(sp);
            stack_vmo.write(self.stack_pages * PAGE_SIZE - init_stack.len(), &init_stack)?;
            sp -= init_stack.len();

            // Initial brk: right after the interpreter (which is placed after the main
            // program). Using interp_base + interp_size ensures brk does not overlap
            // any already-allocated segment.
            //
            // NOTE: dynamically-linked FreeBSD binaries reach here and are built
            // with the Linux-style stack above; running them additionally needs
            // the FreeBSD dynamic linker (`/libexec/ld-elf.so.1`), which this
            // tree does not ship — so in practice only *static* FreeBSD binaries
            // (handled in the no-interpreter path below) get a FreeBSD stack.
            let initial_brk = interp_base + interp_size;
            return Ok((interp_entry, sp, initial_brk, path, abi));
        }

        let size = elf.load_segment_size();
        let image_vmar = vmar
            .allocate(None, size, VmarFlags::CAN_MAP_RXW, PAGE_SIZE)
            .inspect_err(|&e| {
                error!("elf: allocate vmar for size {:#x} failed: {:?}", size, e);
            })?;
        let base = image_vmar.addr();
        let _vmo = image_vmar.load_from_elf(&elf).inspect_err(|&e| {
            error!("elf: load_from_elf failed: {:?}", e);
        })?;
        let entry = base + elf.header.pt2.entry_point() as usize;

        debug!(
            "load: vmar.addr & size: {:#x?}, base: {:#x?}, entry: {:#x?}",
            vmar.get_info(),
            base,
            entry
        );

        // fill syscall entry
        // Write through the VMAR (which resolves the per-segment VMO): the symbol
        // usually lives in .data/.rodata, NOT in the first LOAD segment's VMO.
        if let Some(offset) = elf.get_symbol_address("rcore_syscall_entry") {
            image_vmar.write_memory(base + offset as usize, &self.syscall_entry.to_ne_bytes())?;
        }

        match elf.relocate(image_vmar, vmar) {
            Ok(()) => info!("elf relocate passed !"),
            Err(error) => {
                // Segments stay mapped under `image_vmar.addr()`; do not clobber `base` with the
                // first program header vaddr (often not PT_LOAD). Wrong AT_BASE breaks PIE/musl
                // (e.g. user PC stuck at raw e_entry like 0x423a7 -> page fault NOT_FOUND).
                // A missing `.rela.dyn` is the normal case for non-PIE static
                // binaries, so this is a debug note, not a warning (it fired on
                // every program load at the default LOG=warn).
                debug!(
                    "elf relocate Err:{:?}, keeping load base {:#x}",
                    error, base
                );
            }
        }

        let stack_vmo = VmObject::new_paged(self.stack_pages);
        let flags = MMUFlags::READ | MMUFlags::WRITE | MMUFlags::USER;
        // Place the stack at the top of the process VMAR (which equals the top
        // of the user address space on bare metal, but is a smaller window in
        // aspace-separate/libos builds) so the heap, which grows up from
        // initial_brk, never collides with the stack.
        let stack_top = vmar.end_addr().min(STACK_TOP);
        let stack_bottom = stack_top - stack_vmo.len();
        // map_range=false: lazy stack, same rationale as the interpreter path
        // above — the init_stack tail is committed by `stack_vmo.write` below,
        // everything else demand-zeroes on first touch.
        vmar.map_ext(
            Some(stack_bottom - vmar.addr()),
            stack_vmo.clone(),
            0,
            stack_vmo.len(),
            MMUFlags::RXW,
            flags,
            false,
            false,
        )?;
        let mut sp = stack_top;
        debug!("load stack bottom: {:#x}", stack_bottom);
        let vdso_base = (abi == Abi::Linux)
            .then(|| crate::vdso::map_into(vmar, stack_bottom))
            .flatten();

        let info = abi::ProcInitInfo {
            args,
            envs,
            auxv: {
                let mut map = BTreeMap::new();
                // AT_SYSINFO_EHDR — see the interpreter path above. Static
                // binaries need it just as much: musl resolves
                // `__vdso_clock_gettime` lazily on the first `clock_gettime`,
                // out of the aux vector it saved at startup, and how the
                // program was linked never enters into it.
                if let Some(base) = vdso_base {
                    map.insert(crate::vdso::AT_SYSINFO_EHDR, base);
                }
                #[cfg(target_arch = "x86_64")]
                {
                    // AT_BASE: interpreter load address; 0 means no interpreter (static binary).
                    map.insert(abi::AT_BASE, 0usize);
                    // AT_PHDR: virtual address of program headers in memory.
                    // Use get_phdr_vaddr() which handles both PIE and non-PIE correctly.
                    // If None, the ELF has no loadable segment covering the program headers
                    // (degenerate case warned about inside get_phdr_vaddr()); fall back to
                    // ph_offset() as a best-effort value — AT_PHDR is optional for static
                    // binaries and musl only uses it for TLS initialisation.
                    let phdr_vaddr =
                        elf.get_phdr_vaddr().unwrap_or(elf.header.pt2.ph_offset()) as usize;
                    map.insert(abi::AT_PHDR, base + phdr_vaddr);
                    map.insert(abi::AT_ENTRY, entry);
                }
                #[cfg(target_arch = "riscv64")]
                if let Some(phdr_vaddr) = elf.get_phdr_vaddr() {
                    map.insert(abi::AT_PHDR, base + phdr_vaddr as usize);
                }
                #[cfg(target_arch = "aarch64")]
                {
                    // AT_BASE: 0 means no interpreter (static binary).
                    map.insert(abi::AT_BASE, 0usize);
                    map.insert(abi::AT_ENTRY, entry);
                    if let Some(phdr_vaddr) = elf.get_phdr_vaddr() {
                        map.insert(abi::AT_PHDR, base + phdr_vaddr as usize);
                    }
                }
                map.insert(abi::AT_PHENT, elf.header.pt2.ph_entry_size() as usize);
                map.insert(abi::AT_PHNUM, elf.header.pt2.ph_count() as usize);
                map.insert(abi::AT_PAGESZ, PAGE_SIZE);
                // Identity + AT_SECURE block — same rationale as the sys_execve
                // path above: without it musl flips every process into secure
                // mode (LD_PRELOAD dropped, GLib refuses D-Bus autolaunch).
                map.insert(abi::AT_UID, 0usize);
                map.insert(abi::AT_EUID, 0usize);
                map.insert(abi::AT_GID, 0usize);
                map.insert(abi::AT_EGID, 0usize);
                map.insert(abi::AT_SECURE, 0usize);
                map
            },
        };
        // A FreeBSD static binary needs a FreeBSD-shaped stack (different auxv
        // types, an SSP canary, a `ps_strings` block); everything else keeps the
        // Linux layout untouched.
        let init_stack = match abi {
            #[cfg(target_arch = "x86_64")]
            Abi::Freebsd => {
                let phdr_vaddr =
                    elf.get_phdr_vaddr().unwrap_or(elf.header.pt2.ph_offset()) as usize;
                let fbsd = abi::FreebsdAuxv {
                    phdr: base + phdr_vaddr,
                    phent: elf.header.pt2.ph_entry_size() as usize,
                    phnum: elf.header.pt2.ph_count() as usize,
                    base: 0, // static binary: no interpreter
                    entry,
                    pagesz: PAGE_SIZE,
                    ehdrflags: 0,
                    osreldate: FREEBSD_OSRELDATE,
                    ncpus: kernel_hal::vdso::vdso_constants().max_num_cpus.max(1) as usize,
                    execpath: path.clone(),
                };
                info.push_at_freebsd(sp, &fbsd)
            }
            _ => info.push_at(sp),
        };
        stack_vmo.write(self.stack_pages * PAGE_SIZE - init_stack.len(), &init_stack)?;
        sp -= init_stack.len();

        debug!(
            "ProcInitInfo auxv: {:#x?}\nentry:{:#x}, sp:{:#x}",
            info.auxv, entry, sp
        );

        // Initial brk: right after the loaded image.
        let initial_brk = base + size;
        Ok((entry, sp, initial_brk, path, abi))
    }
}
