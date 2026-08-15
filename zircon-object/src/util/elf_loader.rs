//! ELF loading of Zircon and Linux.
use crate::{error::*, vm::*};
use alloc::sync::Arc;
use xmas_elf::{
    program::{Flags, ProgramHeader, SegmentData, Type},
    sections::SectionData,
    symbol_table::{DynEntry64, Entry},
    ElfFile,
};

/// Extensional ELF loading methods for `VmAddressRegion`.
pub trait VmarExt {
    /// Create `VMObject` from all LOAD segments of `elf` and map them to this VMAR.
    /// Return the first `VMObject`.
    fn load_from_elf(&self, elf: &ElfFile) -> ZxResult<Arc<VmObject>>;
    /// Same as `load_from_elf`, but the `vmo` is an existing one instead of a lot of new ones.
    fn map_from_elf(&self, elf: &ElfFile, vmo: Arc<VmObject>) -> ZxResult;
}

impl VmarExt for VmAddressRegion {
    fn load_from_elf(&self, elf: &ElfFile) -> ZxResult<Arc<VmObject>> {
        let mut first_vmo = None;
        for ph in elf.program_iter() {
            if ph.get_type().unwrap() != Type::Load {
                continue;
            }
            let vmo = make_vmo(elf, ph)?;
            let offset = ph.virtual_addr() as usize / PAGE_SIZE * PAGE_SIZE;
            let flags = ph.flags().to_mmu_flags();
            trace!("ph:{:#x?}, offset:{:#x?}, flags:{:#x?}", ph, offset, flags);
            //映射vmo物理内存块到 VMAR
            self.map_at(offset, vmo.clone(), 0, vmo.len(), flags)?;
            debug!("Map [{:x}, {:x})", offset, offset + vmo.len());
            first_vmo.get_or_insert(vmo);
        }
        Ok(first_vmo.unwrap())
    }
    fn map_from_elf(&self, elf: &ElfFile, vmo: Arc<VmObject>) -> ZxResult {
        for ph in elf.program_iter() {
            if ph.get_type().unwrap() != Type::Load {
                continue;
            }
            let offset = ph.virtual_addr() as usize;
            let flags = ph.flags().to_mmu_flags();
            let vmo_offset = pages(ph.physical_addr() as usize) * PAGE_SIZE;
            let len = pages(ph.mem_size() as usize) * PAGE_SIZE;
            self.map_at(offset, vmo.clone(), vmo_offset, len, flags)?;
        }
        Ok(())
    }
}

trait FlagsExt {
    fn to_mmu_flags(&self) -> MMUFlags;
}

impl FlagsExt for Flags {
    fn to_mmu_flags(&self) -> MMUFlags {
        let mut flags = MMUFlags::USER;
        if self.is_read() {
            flags.insert(MMUFlags::READ);
        }
        if self.is_write() {
            flags.insert(MMUFlags::WRITE);
        }
        if self.is_execute() {
            flags.insert(MMUFlags::EXECUTE);
        }
        flags
    }
}

fn make_vmo(elf: &ElfFile, ph: ProgramHeader) -> ZxResult<Arc<VmObject>> {
    assert_eq!(ph.get_type().unwrap(), Type::Load);
    let page_offset = ph.virtual_addr() as usize % PAGE_SIZE;
    // (VirtAddr余数 + MemSiz)的pages
    let pages = pages(ph.mem_size() as usize + page_offset);
    trace!(
        "VmObject new pages: {:#x}, virtual_addr: {:#x}",
        pages,
        page_offset
    );
    let vmo = VmObject::new_paged(pages);
    let data = match ph.get_data(elf).unwrap() {
        SegmentData::Undefined(data) => data,
        _ => return Err(ZxError::INVALID_ARGS),
    };
    //调用 VMObjectTrait.write, 分配物理内存，后写入程序数据
    vmo.write(page_offset, data)?;
    Ok(vmo)
}

/// Extensional ELF loading methods for `ElfFile`.
pub trait ElfExt {
    /// Get total size of all LOAD segments.
    fn load_segment_size(&self) -> usize;
    /// Get address of the given `symbol`.
    fn get_symbol_address(&self, symbol: &str) -> Option<u64>;
    /// Get the program interpreter path name.
    fn get_interpreter(&self) -> Result<&str, &str>;
    /// Get address of elf phdr
    fn get_phdr_vaddr(&self) -> Option<u64>;
    /// Get the symbol table for dynamic linking (.dynsym section).
    fn dynsym(&self) -> Result<&[DynEntry64], &'static str>;
    /// Relocate according to the dynamic relocation section (.rel.dyn section).
    ///
    /// `scratch_vmar` is where an `R_X86_64_IRELATIVE` entry (if any) borrows a
    /// throwaway page for its resolver's stack. It must NOT be `vmar` itself
    /// when `vmar` is a tightly-packed sub-region (as both call sites' interp/
    /// image VMARs are, sized to their LOAD segments with zero spare room) —
    /// pass the PARENT address space, which still has free space to carve a
    /// page from. Unused (and fine to pass the same VMAR) on any binary with no
    /// IRELATIVE relocations, i.e. every non-glibc / non-dynamically-linked one.
    fn relocate(
        &self,
        vmar: Arc<VmAddressRegion>,
        scratch_vmar: &Arc<VmAddressRegion>,
    ) -> Result<(), &'static str>;
}

impl ElfExt for ElfFile<'_> {
    fn load_segment_size(&self) -> usize {
        self.program_iter()
            .filter(|ph| ph.get_type().unwrap() == Type::Load)
            .map(|ph| pages((ph.virtual_addr() + ph.mem_size()) as usize))
            .max()
            .unwrap_or(0)
            * PAGE_SIZE
    }

    fn get_symbol_address(&self, symbol: &str) -> Option<u64> {
        for section in self.section_iter() {
            if let Ok(SectionData::SymbolTable64(entries)) = section.get_data(self) {
                for e in entries {
                    if let Ok(name) = e.get_name(self) {
                        if name == symbol {
                            return Some(e.value());
                        }
                    }
                }
            }
        }
        None
    }

    fn get_interpreter(&self) -> Result<&str, &str> {
        let header = self
            .program_iter()
            .find(|ph| ph.get_type() == Ok(Type::Interp))
            .ok_or("no interp header")?;
        let data = match header.get_data(self)? {
            SegmentData::Undefined(data) => data,
            _ => return Err("bad interp"),
        };
        let len = (0..).find(|&i| data[i] == 0).unwrap();
        let path = core::str::from_utf8(&data[..len]).map_err(|_| "failed to convert to utf8")?;
        Ok(path)
    }

    /*
     * [ ERROR ] page fualt from user mode 0x40 READ
     */

    fn get_phdr_vaddr(&self) -> Option<u64> {
        if let Some(phdr) = self
            .program_iter()
            .find(|ph| ph.get_type() == Ok(Type::Phdr))
        {
            // if phdr exists in program header, use it
            Some(phdr.virtual_addr())
        } else if let Some(elf_addr) = self
            .program_iter()
            .find(|ph| ph.get_type() == Ok(Type::Load) && ph.offset() == 0)
        {
            // otherwise, check if elf is loaded from the beginning, then phdr can be inferred.
            Some(elf_addr.virtual_addr() + self.header.pt2.ph_offset())
        } else {
            warn!("elf: no phdr found, tls might not work");
            None
        }
    }

    fn dynsym(&self) -> Result<&[DynEntry64], &'static str> {
        match self
            .find_section_by_name(".dynsym")
            .ok_or(".dynsym not found")?
            .get_data(self)
            .map_err(|_| "corrupted .dynsym")?
        {
            SectionData::DynSymbolTable64(dsym) => Ok(dsym),
            _ => Err("bad .dynsym"),
        }
    }

    #[allow(unsafe_code)]
    // `scratch_vmar` only feeds `resolve_irelative_x86_64`, which is gated to
    // x86_64 (IFUNC resolution means *executing* the resolver, and only the
    // x86_64 trap plumbing to do that exists). On every other arch the
    // parameter is genuinely unused, and the crate's `#[deny(warnings)]` turns
    // that into a build failure — aarch64 and riscv64 did not compile at all.
    // Scoped to those arches so the lint keeps its teeth on x86_64.
    #[cfg_attr(not(target_arch = "x86_64"), allow(unused_variables))]
    fn relocate(
        &self,
        vmar: Arc<VmAddressRegion>,
        scratch_vmar: &Arc<VmAddressRegion>,
    ) -> Result<(), &'static str> {
        // Symbol-resolving relocations (write `S + A`).
        // x86_64
        const REL_GOT: u32 = 6; // R_X86_64_GLOB_DAT
        const REL_PLT: u32 = 7; // R_X86_64_JUMP_SLOT
        const R_X86_64_64: u32 = 1;
        // riscv64
        const R_RISCV_64: u32 = 2;
        // aarch64
        const R_AARCH64_GLOBAL_DATA: u32 = 0x401;
        const R_AARCH64_JUMP_SLOT: u32 = 0x402;

        // Base-relative relocations (write `B + A`).
        // x86_64
        const REL_RELATIVE: u32 = 8; // R_X86_64_RELATIVE
                                     // riscv64
        const R_RISCV_RELATIVE: u32 = 3;
        // aarch64
        const R_AARCH64_RELATIVE: u32 = 0x403;

        // R_X86_64_IRELATIVE: the value to write is not a fixed address but the
        // RETURN VALUE of calling a resolver function at `base + addend` (an
        // IFUNC). glibc's own dynamic linker (`ld-linux-x86-64.so.2`) uses
        // CPU-feature-dispatched routines (optimized memcpy/memset/strlen etc.)
        // even internally, so a real glibc interpreter's `.rela.plt` legitimately
        // carries these. Collected here and resolved in a batch after the main
        // relocation loop (see the call to `resolve_irelative_x86_64` below) —
        // resolving one requires actually EXECUTING the resolver, which this
        // per-entry loop has no business doing mid-scan.
        #[cfg(target_arch = "x86_64")]
        const R_X86_64_IRELATIVE: u32 = 37;
        #[cfg(target_arch = "x86_64")]
        let mut irelative: alloc::vec::Vec<(usize, usize)> = alloc::vec::Vec::new();

        let base = vmar.addr();
        // `.dynsym` may be absent for binaries that only carry RELATIVE
        // relocations; resolve it lazily so those still get applied.
        let dynsym = self.dynsym().ok();
        let mut found_any = false;

        // One-entry mapping cache. Relocation targets are heavily clustered
        // (GOT / PLT / .data.rel.ro live in one or two mappings), while
        // `Vmar::write_memory` re-resolves the mapping with a linear VMAR
        // scan on EVERY call — O(entries × mappings) per exec for a PIE
        // binary with thousands of R_*_RELATIVE entries.
        let mut cached: Option<Arc<VmMapping>> = None;
        let write_word = |cached: &mut Option<Arc<VmMapping>>,
                          addr: usize,
                          value: usize|
         -> Result<(), &'static str> {
            let bytes = value.to_ne_bytes();
            if let Some(map) = cached.as_ref() {
                if map
                    .write_memory_if_contains(addr, &bytes)
                    .map_err(|_| "Invalid Vmar")?
                {
                    return Ok(());
                }
            }
            let map = vmar.find_mapping(addr).ok_or("Invalid Vmar")?;
            if !map
                .write_memory_if_contains(addr, &bytes)
                .map_err(|_| "Invalid Vmar")?
            {
                // Boundary case (write straddles the mapping end): preserve the
                // old clamped-partial-write behaviour.
                vmar.write_memory(addr, &bytes)
                    .map_err(|_| "Invalid Vmar")?;
            }
            *cached = Some(map);
            Ok(())
        };

        // Apply both the general dynamic relocations (`.rela.dyn`) and the PLT
        // relocations (`.rela.plt`). The latter holds the JUMP_SLOT entries that
        // back the procedure linkage table; skipping it leaves call targets
        // pointing at unrelocated stubs (observed as a jump to a low address and
        // an Invalid Opcode #UD fault).
        for &sec_name in [".rela.dyn", ".rela.plt"].iter() {
            let section = match self.find_section_by_name(sec_name) {
                Some(section) => section,
                None => continue,
            };
            let entries = match section
                .get_data(self)
                .map_err(|_| "corrupted relocation section")?
            {
                SectionData::Rela64(entries) => entries,
                _ => continue,
            };
            found_any = true;
            for entry in entries.iter() {
                match entry.get_type() {
                    REL_GOT
                    | REL_PLT
                    | R_X86_64_64
                    | R_RISCV_64
                    | R_AARCH64_GLOBAL_DATA
                    | R_AARCH64_JUMP_SLOT => {
                        let dynsym = match dynsym {
                            Some(dynsym) => dynsym,
                            None => {
                                warn!("relocate: symbol relocation but no .dynsym; skipping");
                                continue;
                            }
                        };
                        let sym = &dynsym[entry.get_symbol_table_index() as usize];
                        // An undefined symbol (shndx == 0) is resolved later by the
                        // dynamic linker in user space (or is simply unavailable to
                        // the in-kernel loader). Skip it instead of panicking — a
                        // user binary must never be able to crash the kernel.
                        if sym.shndx() == 0 {
                            let name = sym.get_name(self).unwrap_or("<unknown>");
                            warn!("relocate: undefined symbol {:?}, skipping", name);
                            continue;
                        }
                        let symval = base + sym.value() as usize;
                        let value = symval + entry.get_addend() as usize;
                        let addr = base + entry.get_offset() as usize;
                        trace!("GOT write: {:#x} @ {:#x}", value, addr);
                        write_word(&mut cached, addr, value)?;
                    }
                    REL_RELATIVE | R_RISCV_RELATIVE | R_AARCH64_RELATIVE => {
                        let value = base + entry.get_addend() as usize;
                        let addr = base + entry.get_offset() as usize;
                        trace!("RELATIVE write: {:#x} @ {:#x}", value, addr);
                        write_word(&mut cached, addr, value)?;
                    }
                    // Unsupported relocation type (e.g. TLS relocations). Log and
                    // skip rather than `unimplemented!()`, which would panic the
                    // whole kernel because of one user program.
                    other => {
                        #[cfg(target_arch = "x86_64")]
                        if other == R_X86_64_IRELATIVE {
                            let got_addr = base + entry.get_offset() as usize;
                            let resolver_addr = base + entry.get_addend() as usize;
                            irelative.push((got_addr, resolver_addr));
                            continue;
                        }
                        warn!(
                            "relocate: skipping unsupported relocation type {} in {}",
                            other, sec_name
                        );
                    }
                }
            }
        }

        // Resolve every collected IRELATIVE entry by actually running its
        // resolver, then write the results back through the same `write_word`
        // path as every other relocation type.
        #[cfg(target_arch = "x86_64")]
        if !irelative.is_empty() {
            let n = irelative.len();
            match resolve_irelative_x86_64(scratch_vmar, &irelative) {
                Ok(resolved) => {
                    let got = resolved.len();
                    for (addr, value) in resolved {
                        write_word(&mut cached, addr, value)?;
                    }
                    debug!("relocate: resolved {}/{} IRELATIVE entries", got, n);
                }
                Err(e) => {
                    warn!(
                        "relocate: IRELATIVE batch resolution failed ({}); {} entries left unresolved",
                        e, n
                    );
                }
            }
        }

        if found_any {
            Ok(())
        } else {
            Err(".rela.dyn not found")
        }
    }
}

/// Resolve a batch of `R_X86_64_IRELATIVE` relocations by actually EXECUTING
/// each resolver function once, synchronously, in a brief ring-3 excursion into
/// the target address space. Unlike every other relocation type, IRELATIVE's
/// value is not a fixed address — it is the RETURN VALUE of *calling* the
/// resolver at `base + addend`, exactly what a real ld.so does for its own
/// IFUNCs (glibc's `elf_ifunc_invoke`). The kernel does it here because this
/// loader eagerly relocates the ELF interpreter itself in kernel space, before
/// it ever runs its own bootstrap — real Linux never needs this, since ld.so
/// self-relocates in userspace and resolves its own IFUNCs as ordinary running
/// C code.
///
/// Leaving these unresolved (the previous behaviour: skip + warn) left the GOT
/// slot at zero, so any call through it jumped to address 0. That is silent
/// death for a glibc-linked desktop stack specifically: the interpreter
/// bootstraps far enough to load and run the main program (labwc logs
/// correctly through its whole configuration/output-setup phase — none of
/// which happens to call a broken slot), but the first render pass — heavy on
/// memcpy/memset for the software (pixman) blit path, both IFUNC-resolved in
/// glibc — hits one and the process goes silently dark: modeset succeeds, the
/// log stops dead, nothing is ever drawn. Confirmed by reproducing the exact
/// symptom in QEMU with a glibc labwc/wlroots stack and tracing the fault to
/// `relocate: skipping unsupported relocation type 37 in .rela.plt`.
///
/// A resolver is a small ABI-guaranteed leaf function (checks CPUID/HWCAP,
/// returns a function pointer; no syscalls, no callbacks into the loader, no
/// blocking), so running it via the SAME synchronous user-mode entry primitive
/// every thread already uses (`UserContext::enter_uspace`) is safe: any trap
/// other than the deliberately-unmapped sentinel return address we set up
/// (a clean instruction-fetch page fault, chosen so `ret` cannot be confused
/// with a legitimate jump) aborts the WHOLE batch rather than writing a
/// possibly-bogus value into a live GOT slot — the same fail-closed contract
/// `relocate()` already has for every other error path. A real hardware
/// interrupt (the 250 Hz timer) is serviced and the SAME resolver call is
/// simply resumed, exactly as the normal thread-execution loop
/// (`loader/src/linux.rs::run_user`) already does for ordinary user code.
#[cfg(target_arch = "x86_64")]
fn resolve_irelative_x86_64(
    vmar: &Arc<VmAddressRegion>,
    entries: &[(usize, usize)],
) -> Result<alloc::vec::Vec<(usize, usize)>, &'static str> {
    use kernel_hal::context::{TrapReason, UserContext, UserContextField};

    // Deliberately unmapped, canonical, and cheap to recognise: `ret` popping
    // this into RIP takes an immediate instruction-fetch #PF at EXACTLY this
    // address, which is how a resolver's return is detected. Address 0 is
    // avoided (a NULL-derived bug elsewhere could coincidentally target it).
    const SENTINEL_RETURN: usize = 0x8;

    // A scratch stack: this runs before the process has a real stack (that is
    // allocated by the caller AFTER this relocation pass), so borrow one page
    // from the interpreter's own VMAR for the duration of this call, then give
    // it back. One word (the sentinel return address) is all any of these
    // leaf resolvers need.
    let scratch = vmar
        .map(
            None,
            VmObject::new_paged(1),
            0,
            PAGE_SIZE,
            MMUFlags::READ | MMUFlags::WRITE | MMUFlags::USER,
        )
        .map_err(|_| "IRELATIVE: failed to map scratch stack")?;
    let scratch_top = scratch + PAGE_SIZE;
    let sp = scratch_top - core::mem::size_of::<usize>();
    let unmap_scratch = || {
        let _ = vmar.unmap(scratch, PAGE_SIZE);
    };
    if vmar.write_memory(sp, &SENTINEL_RETURN.to_ne_bytes()).is_err() {
        unmap_scratch();
        return Err("IRELATIVE: failed to seed scratch stack");
    }

    let mut resolved = alloc::vec::Vec::with_capacity(entries.len());
    // One CR3 write for the whole batch rather than one per entry; nothing
    // between entries needs the kernel's own address space.
    kernel_hal::vm::activate_paging(vmar.table_phys());

    let mut aborted = false;
    for &(got_addr, resolver_addr) in entries {
        let mut ctx = UserContext::new();
        ctx.setup_uspace(resolver_addr, sp, &[0, 0, 0]);
        // Loop instead of a single `enter_uspace`: a real hardware interrupt
        // (the periodic timer) legitimately traps here too. Service it and
        // resume the SAME context exactly where it left off — x86 interrupts
        // are precise, so this is indistinguishable from the interrupt never
        // having happened, from the resolver's point of view.
        loop {
            ctx.enter_uspace();
            match ctx.trap_reason() {
                TrapReason::Interrupt(vector) => {
                    kernel_hal::interrupt::handle_irq(vector);
                    continue;
                }
                TrapReason::PageFault(vaddr, flags)
                    if vaddr == SENTINEL_RETURN && flags.contains(MMUFlags::EXECUTE) =>
                {
                    resolved.push((got_addr, ctx.get_field(UserContextField::ReturnValue)));
                    break;
                }
                other => {
                    warn!(
                        "relocate: IRELATIVE resolver at {:#x} did not return cleanly ({:?}); \
                         aborting the remaining {} entr{} in this batch",
                        resolver_addr,
                        other,
                        entries.len() - resolved.len(),
                        if entries.len() - resolved.len() == 1 { "y" } else { "ies" }
                    );
                    aborted = true;
                    break;
                }
            }
        }
        if aborted {
            break;
        }
    }

    kernel_hal::vm::activate_kernel_paging();
    unmap_scratch();
    Ok(resolved)
}
