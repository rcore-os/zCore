//! This file is modified from 'page_table.rs' in 'rust-osdev/bootloader'

use x86_64::structures::paging::{mapper::*, *};
use x86_64::{align_up, PhysAddr, VirtAddr};
use xmas_elf::{program, ElfFile};

/// A frame allocator that can also hand out a physically-contiguous run in
/// one call. UEFI's `allocate_pages` takes a page count natively, so per-frame
/// calls for a big run (the kernel's ~523 MiB BSS is ~134k frames) are pure
/// firmware-round-trip overhead — a visible slice of boot wall time.
pub trait BulkFrameAllocator: FrameAllocator<Size4KiB> {
    /// Allocate `count` contiguous 4 KiB frames, returning the first, or
    /// `None` if no contiguous run of that size exists.
    fn allocate_frames(&mut self, count: u64) -> Option<PhysFrame>;
}

pub fn map_elf(
    elf: &ElfFile,
    page_table: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl BulkFrameAllocator,
) -> Result<(), MapToError<Size4KiB>> {
    //info!("mapping ELF");
    let kernel_start = PhysAddr::new(elf.input.as_ptr() as u64);
    for segment in elf.program_iter() {
        map_segment(&segment, kernel_start, page_table, frame_allocator)?;
    }
    Ok(())
}

pub fn map_stack(
    addr: u64,
    pages: u64,
    page_table: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    //info!("mapping stack at {:#x}", addr);
    // create a stack
    let stack_start = Page::containing_address(VirtAddr::new(addr));
    let stack_end = stack_start + pages;

    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

    for page in Page::range(stack_start, stack_end) {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        unsafe {
            page_table
                .map_to(page, frame, flags, frame_allocator)?
                .flush();
        }
    }

    Ok(())
}

fn map_segment(
    segment: &program::ProgramHeader,
    kernel_start: PhysAddr,
    page_table: &mut impl Mapper<Size4KiB>,
    frame_allocator: &mut impl BulkFrameAllocator,
) -> Result<(), MapToError<Size4KiB>> {
    if segment.get_type().unwrap() != program::Type::Load {
        return Ok(());
    }
    //debug!("mapping segment: {:#x?}", segment);
    let mem_size = segment.mem_size();
    let file_size = segment.file_size();
    let file_offset = segment.offset() & !0xfff;
    let phys_start_addr = kernel_start + file_offset;
    let virt_start_addr = VirtAddr::new(segment.virtual_addr());

    let start_page: Page = Page::containing_address(virt_start_addr);
    let start_frame = PhysFrame::containing_address(phys_start_addr);
    let end_frame = PhysFrame::containing_address(phys_start_addr + file_size - 1u64);

    let flags = segment.flags();
    let mut page_table_flags = PageTableFlags::PRESENT;
    // Don't force NX on loader-created kernel mappings. Some real-world kernel
    // ELF layouts/flags can make this deny the very first instruction fetch,
    // resulting in "100% progress but no logs/shell". The kernel installs its
    // own page tables/policy right after handoff.
    if flags.is_write() {
        page_table_flags |= PageTableFlags::WRITABLE
    };

    for frame in PhysFrame::range_inclusive(start_frame, end_frame) {
        let offset = frame - start_frame;
        let page = start_page + offset;
        unsafe {
            match page_table.map_to(page, frame, page_table_flags, frame_allocator) {
                Ok(flush) => flush.flush(),
                Err(MapToError::PageAlreadyMapped(_)) => {
                    // Si el firmware ya mapeó esta página, aceptarlo siempre que
                    // apunte al mismo frame físico.
                    if page_table.translate_page(page).ok() != Some(frame) {
                        return Err(MapToError::PageAlreadyMapped(frame));
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    if mem_size > file_size {
        // .bss section (or similar), which needs to be zeroed
        let zero_start = virt_start_addr + file_size;
        let zero_end = virt_start_addr + mem_size;
        if zero_start.as_u64() & 0xfff != 0 {
            // A part of the last mapped frame needs to be zeroed. This is
            // not possible since it could already contains parts of the next
            // segment. Thus, we need to copy it before zeroing.

            let new_frame = frame_allocator
                .allocate_frame()
                .ok_or(MapToError::FrameAllocationFailed)?;

            type PageArray = [u64; Size4KiB::SIZE as usize / 8];

            let last_page = Page::containing_address(virt_start_addr + file_size - 1u64);
            let last_page_ptr = end_frame.start_address().as_u64() as *mut PageArray;
            let temp_page_ptr = new_frame.start_address().as_u64() as *mut PageArray;

            unsafe {
                // copy contents
                temp_page_ptr.write(last_page_ptr.read());
            }

            // remap last page
            if let Err(e) = page_table.unmap(last_page) {
                return Err(match e {
                    UnmapError::ParentEntryHugePage => MapToError::ParentEntryHugePage,
                    UnmapError::PageNotMapped => unreachable!(),
                    UnmapError::InvalidFrameAddress(_) => unreachable!(),
                });
            }
            unsafe {
                page_table
                    .map_to(last_page, new_frame, page_table_flags, frame_allocator)?
                    .flush();
            }
        }

        // Map additional frames. Allocate them in the largest contiguous runs
        // the firmware will grant (halving the request on failure) instead of
        // one `allocate_pages` firmware call PER 4 KiB frame — the kernel's
        // ~523 MiB BSS is ~134k frames, and the per-frame round-trips cost
        // whole seconds of boot. The PTEs stay 4 KiB (the kernel's stack
        // guard punches 4 KiB holes into this range). No per-page invlpg:
        // these virtual addresses were never accessed, so no stale
        // translation can exist, and the first touch is the zeroing below —
        // after the mapping is installed.
        let start_page: Page =
            Page::containing_address(VirtAddr::new(align_up(zero_start.as_u64(), Size4KiB::SIZE)));
        let end_page = Page::containing_address(zero_end);
        let mut page = start_page;
        let mut remaining = end_page - start_page + 1;
        while remaining > 0 {
            let mut run = remaining;
            let base = loop {
                match frame_allocator.allocate_frames(run) {
                    Some(f) => break f,
                    None if run > 1 => run = run.div_ceil(2),
                    None => return Err(MapToError::FrameAllocationFailed),
                }
            };
            for i in 0..run {
                unsafe {
                    page_table
                        .map_to(page, base + i, page_table_flags, frame_allocator)?
                        .ignore();
                }
                page += 1;
            }
            remaining -= run;
        }

        // zero bss
        unsafe {
            core::ptr::write_bytes(
                zero_start.as_mut_ptr::<u8>(),
                0,
                (mem_size - file_size) as usize,
            );
        }
    }
    Ok(())
}

/// Map physical memory [0, max_addr)
/// to virtual space [offset, offset + max_addr)
///
/// Uses 2 MiB pages for the bulk of the range: mapping every 4 KiB frame
/// individually cost one `map_to` (plus page-table frames allocated one
/// firmware call at a time) per page — 1M+ iterations at 4 GiB, growing
/// linearly with RAM, i.e. whole seconds of boot. Per-page invlpg is also
/// dropped: these physmap virtual addresses were never accessed (x86 does
/// not cache not-present translations), and CR3 is reloaded at kernel
/// handoff anyway.
///
/// The GOP framebuffer range stays 4 KiB-mapped (`fb_addr`/`fb_size`,
/// rounded out to 2 MiB borders): the kernel's `pat.rs` retypes the fb's
/// physmap PTEs to write-combining and deliberately refuses huge leaves —
/// a huge-mapped fb would silently fall back to UC and lose the display
/// path's biggest optimization.
///
/// Any 2 MiB chunk the huge mapping cannot cover (firmware already holds a
/// conflicting entry) falls back to the old per-4KiB path for that chunk,
/// with the same already-mapped-to-the-same-frame tolerance as before.
pub fn map_physical_memory(
    offset: u64,
    max_addr: u64,
    fb_addr: u64,
    fb_size: u64,
    page_table: &mut (impl Mapper<Size4KiB> + Mapper<Size2MiB>),
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) {
    //info!("mapping physical memory");
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
    let huge = Size2MiB::SIZE;
    let end = align_up(max_addr, huge);
    // Framebuffer carve-out, rounded OUT to whole 2 MiB chunks. An empty fb
    // yields an empty range that no chunk can intersect.
    let (carve_start, carve_end) = if fb_size > 0 {
        (fb_addr & !(huge - 1), align_up(fb_addr + fb_size, huge))
    } else {
        (u64::MAX, u64::MAX)
    };
    let mut addr = 0u64;
    while addr < end {
        let in_carve = addr >= carve_start && addr < carve_end;
        if !in_carve {
            let frame = PhysFrame::<Size2MiB>::containing_address(PhysAddr::new(addr));
            let page = Page::<Size2MiB>::containing_address(VirtAddr::new(addr + offset));
            let mapped = unsafe { page_table.map_to(page, frame, flags, frame_allocator) };
            if let Ok(flush) = mapped {
                flush.ignore();
                addr += huge;
                continue;
            }
            // Fall through: cover this chunk with 4 KiB pages instead.
        }
        let chunk_end = (addr + huge).min(end);
        while addr < chunk_end {
            let frame = PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(addr));
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr + offset));
            unsafe {
                page_table
                    .map_to(page, frame, flags, frame_allocator)
                    .map(|flush| flush.ignore())
                    .or_else(|e| match e {
                        MapToError::PageAlreadyMapped(_)
                            if page_table.translate_page(page).ok() == Some(frame) =>
                        {
                            Ok(())
                        }
                        other => Err(other),
                    })
                    .expect("failed to map physical memory");
            }
            addr += Size4KiB::SIZE;
        }
    }
}
