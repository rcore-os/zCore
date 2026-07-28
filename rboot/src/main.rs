//! Simple ELF OS Loader on UEFI
//!
//! 1. Load config from "\EFI\Boot\rboot.conf"
//! 2. Load kernel ELF file
//! 3. Map ELF segments to virtual memory
//! 4. Map kernel stack and all physical memory
//! 5. Exit boot and jump to ELF entry

#![no_std]
#![no_main]
#![feature(abi_efiapi)]
#![feature(abi_x86_interrupt)]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::arch::asm;
use log::LevelFilter;
use rboot::{BootInfo, GraphicInfo};
use uefi::proto::console::gop::{GraphicsOutput, PixelFormat};
use uefi::proto::media::file::*;
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::proto::unsafe_protocol;
use uefi::table::boot::*;
use uefi::table::cfg::{ACPI2_GUID, SMBIOS_GUID};
use uefi::{prelude::*, CStr16};
use x86_64::registers::control::*;
use x86_64::structures::paging::*;
use x86_64::{PhysAddr, VirtAddr};
use xmas_elf::ElfFile;

mod config;
mod idt;
mod logo;
mod page_table;
mod progress;

const CONFIG_PATH: &str = "\\EFI\\Boot\\rboot.conf";

fn parse_log_level_from_cmdline(cmdline: &str) -> Option<LevelFilter> {
    // cmdline format example:
    //   "LOG=debug:ROOTPROC=/bin/busybox?sh:TERM=xterm-256color"
    // We keep this parser intentionally tiny (no alloc) and tolerant.
    for part in cmdline.split(':') {
        let mut it = part.splitn(2, '=');
        let k = it.next()?.trim();
        let v = it.next().unwrap_or("").trim();
        if k.eq_ignore_ascii_case("LOG") {
            return Some(v.parse().unwrap_or(LevelFilter::Info));
        }
    }
    None
}

fn has_cmdline_flag(cmdline: &str, key: &str) -> bool {
    for part in cmdline.split(':') {
        let mut it = part.splitn(2, '=');
        let k = it.next().unwrap_or("").trim();
        let v = it.next().unwrap_or("").trim();
        if k.eq_ignore_ascii_case(key) {
            return v.is_empty()
                || v == "1"
                || v.eq_ignore_ascii_case("true")
                || v.eq_ignore_ascii_case("on");
        }
    }
    false
}

#[entry]
fn efi_main(image: Handle, mut st: SystemTable<Boot>) -> Status {
    // Asegura que `BootServices::image_handle()` sea correcto (lo usa ExitBootServices internamente).
    unsafe { st.boot_services().set_image_handle(image) };
    uefi_services::init(&mut st).expect("failed to initialize utilities");

    //info!("bootloader is running");
    let bs = st.boot_services();
    let config = {
        let mut file = open_file(bs, CONFIG_PATH);
        let buf = load_file(bs, &mut file);
        config::Config::parse(buf)
    };

    if let Some(level) = parse_log_level_from_cmdline(config.cmdline) {
        log::set_max_level(level);
    }
    info!("rboot: start (log={:?})", log::max_level());

    // Snapshot cmdline flags before ExitBootServices: `config` lives on the stack
    // and must not be read after firmware reclaims that memory.
    let use_rboot_idt = has_cmdline_flag(config.cmdline, "RBOOT_IDT");

    let (graphic_info, edid, edid_size) = init_graphic(bs, config.resolution);
    // Boot progress is continuous across rboot (0..50) and kernel (50..100).
    if has_cmdline_flag(config.cmdline, "FB_ROT180") {
        progress::set_rot180(true);
    }
    // Draw splash logo immediately after GOP init (this also clears screen to white).
    logo::draw_centered(graphic_info.mode, graphic_info.fb_addr);
    progress::bar(graphic_info.mode, graphic_info.fb_addr, 0);
    debug!("rboot config: {:#x?}", config);
    progress::bar(graphic_info.mode, graphic_info.fb_addr, 5);

    let acpi2_addr = st
        .config_table()
        .iter()
        .find(|entry| entry.guid == ACPI2_GUID)
        .expect("failed to find ACPI 2 RSDP")
        .address;
    debug!("acpi2 rsdp: {:?}", acpi2_addr);

    let smbios_addr = st
        .config_table()
        .iter()
        .find(|entry| entry.guid == SMBIOS_GUID)
        .expect("failed to find SMBIOS")
        .address;
    debug!("smbios: {:?}", smbios_addr);

    let elf = {
        let mut file = open_file(bs, config.kernel_path);
        let buf = load_file(bs, &mut file);
        ElfFile::new(buf).expect("failed to parse ELF")
    };
    progress::bar(graphic_info.mode, graphic_info.fb_addr, 15);
    debug!(
        "kernel elf loaded: entry={:#x}",
        elf.header.pt2.entry_point()
    );
    unsafe {
        ENTRY = elf.header.pt2.entry_point() as usize;
    }

    let (initramfs_addr, initramfs_size) = if let Some(path) = config.initramfs {
        let mut file = open_file(bs, path);
        let buf = load_file(bs, &mut file);
        debug!(
            "initramfs loaded: addr={:#x} size={:#x}",
            buf.as_ptr() as u64,
            buf.len()
        );
        (buf.as_ptr() as u64, buf.len() as u64)
    } else {
        (0, 0)
    };
    progress::bar(graphic_info.mode, graphic_info.fb_addr, 20);

    let max_mmap_size = st.boot_services().memory_map_size().map_size;
    let mmap_storage = Box::leak(vec![0u8; max_mmap_size * 2].into_boxed_slice());
    let max_phys_addr = {
        let mmap = st
            .boot_services()
            .memory_map(mmap_storage)
            .expect("failed to get memory map");
        mmap.entries()
            .map(|m| m.phys_start + m.page_count * 0x1000)
            .max()
            .unwrap()
            .max(0x1_0000_0000) // include IOAPIC MMIO area
            // Ensure the GOP framebuffer is always within the mapped range.
            // On most systems the framebuffer is listed in the UEFI memory map,
            // but on some firmware/GPU combinations the framebuffer BAR can sit
            // above the highest RAM entry.  Without this the kernel's
            // phys_to_virt(fb_addr) would translate to an unmapped virtual
            // address and triple-fault in early_fb_console::try_init().
            .max(graphic_info.fb_addr + graphic_info.fb_size)
            // Ensure initramfs is always within the mapped range too. Some firmware
            // allocates LOADER_DATA at high physical addresses that are above the
            // highest conventional RAM entry in the memory map we iterated.
            .max(initramfs_addr + initramfs_size)
    };
    progress::bar(graphic_info.mode, graphic_info.fb_addr, 30);

    let mut page_table = current_page_table();
    unsafe {
        Cr0::update(|f| f.remove(Cr0Flags::WRITE_PROTECT));
        // On real hardware UEFI has already set EFER.NXE before we run, so
        // NO_EXECUTE bits we write into page-table entries are enforced.
        // Keep NXE enabled while running under firmware page tables. Some
        // UEFI mappings may already use NX bits and clearing NXE can fault
        // immediately on real hardware.
        Efer::update(|f| f.insert(EferFlags::NO_EXECUTE_ENABLE));
    }
    debug!("mapping elf segments...");
    page_table::map_elf(&elf, &mut page_table, &mut UEFIFrameAllocator(bs))
        .expect("failed to map ELF");
    debug!("mapping kernel stack...");
    page_table::map_stack(
        config.kernel_stack_address,
        config.kernel_stack_size,
        &mut page_table,
        &mut UEFIFrameAllocator(bs),
    )
    .expect("failed to map stack");
    debug!("mapping physical memory...");
    page_table::map_physical_memory(
        config.physical_memory_offset,
        max_phys_addr,
        &mut page_table,
        &mut UEFIFrameAllocator(bs),
    );
    progress::bar(graphic_info.mode, graphic_info.fb_addr, 40);
    debug!("sanity checks before ExitBootServices...");

    // Sanity checks while Boot Services are still alive.
    // If these fault on real hardware, the firmware is much more likely to show a dump.
    let stacktop = config.kernel_stack_address + config.kernel_stack_size * 0x1000;
    progress::bar(graphic_info.mode, graphic_info.fb_addr, 42);
    unsafe {
        // 1) Confirm the entry virtual address is mapped & readable.
        let entry_va = ENTRY as *const u8;
        let _first_byte = core::ptr::read_volatile(entry_va);
        // 2) Confirm the stack top page is mapped & writable.
        let sp_probe = (stacktop - 8) as *mut u64;
        core::ptr::write_volatile(sp_probe, 0);
    }
    progress::bar(graphic_info.mode, graphic_info.fb_addr, 44);
    unsafe {
        Cr0::update(|f| f.insert(Cr0Flags::WRITE_PROTECT));
    }

    //info!("exit boot services");

    let mut memory_map = Vec::with_capacity(256);
    // Pre-allocate BootInfo on the heap while boot services are still available.
    // We'll fill it with the real memory map after ExitBootServices.
    let mut bootinfo_box = Box::new(BootInfo {
        memory_map: Vec::new(),
        physical_memory_offset: config.physical_memory_offset,
        graphic_info,
        acpi2_rsdp_addr: acpi2_addr as u64,
        smbios_addr: smbios_addr as u64,
        initramfs_addr,
        initramfs_size,
        cmdline: config.cmdline,
        edid,
        edid_size,
    });

    // On some real machines, ExitBootServices can be the point where things go wrong.
    // Update the bar just before attempting it so we can pinpoint the hang visually.
    progress::bar(graphic_info.mode, graphic_info.fb_addr, 45);
    debug!("calling ExitBootServices (raw)...");
    let (map_size, desc_size) = exit_boot_services_raw(&mut st, image, mmap_storage);
    progress::bar(graphic_info.mode, graphic_info.fb_addr, 47);
    debug!("ExitBootServices ok, collecting memory map...");

    // Reinterpret the raw memory map buffer as `MemoryDescriptor` entries.
    // SAFETY: `mmap_storage` is leaked, and UEFI guarantees the memory map layout.
    let entry_size = desc_size;
    let len = map_size / entry_size;
    for i in 0..len {
        let p = unsafe { mmap_storage.as_ptr().add(i * entry_size) } as *const MemoryDescriptor;
        // Some firmware leaves unused tail bytes; stop on obviously empty descriptors.
        let d = unsafe { &*p };
        if d.page_count == 0 {
            break;
        }
        memory_map.push(d);
    }
    progress::bar(graphic_info.mode, graphic_info.fb_addr, 49);

    bootinfo_box.memory_map = memory_map;
    let bootinfo: &'static BootInfo = Box::leak(bootinfo_box);
    // Hand-off point to the kernel.
    progress::bar(graphic_info.mode, graphic_info.fb_addr, 50);

    unsafe {
        debug!("jumping to kernel entry...");
        // If we see 51% but not the kernel marker (52%), the hang is inside the
        // handoff asm / very first instruction fetch.
        progress::bar(graphic_info.mode, graphic_info.fb_addr, 51);
        if use_rboot_idt {
            idt::init(graphic_info.mode, graphic_info.fb_addr);
        }
        jump_to_entry(bootinfo, stacktop);
    }
}

fn exit_boot_services_raw(
    st: &mut SystemTable<Boot>,
    image: Handle,
    mmap_storage: &mut [u8],
) -> (usize, usize) {
    // Avoid `SystemTable::exit_boot_services` because it resets the machine on failure
    // without printing the underlying Status.
    //
    // Instead, call `GetMemoryMap` and `ExitBootServices` via the raw table pointers
    // and retry a couple of times.
    let st_raw = st.as_ptr() as *mut uefi_raw::table::system::SystemTable;
    let bs_raw = unsafe { &mut *(*st_raw).boot_services };

    let mut last = Status::ABORTED;
    for _ in 0..2 {
        let mut map_size = mmap_storage.len();
        let mut map_key: usize = 0;
        let mut desc_size: usize = 0;
        let mut desc_ver: u32 = 0;

        let status = unsafe {
            (bs_raw.get_memory_map)(
                &mut map_size,
                mmap_storage.as_mut_ptr().cast(),
                &mut map_key,
                &mut desc_size,
                &mut desc_ver,
            )
        };
        if status != Status::SUCCESS {
            last = status;
            continue;
        }

        let status = unsafe { (bs_raw.exit_boot_services)(image.as_ptr(), map_key) };
        if status == Status::SUCCESS {
            return (map_size, desc_size);
        }
        last = status;
    }

    panic!("ExitBootServices failed: {:?}", last);
}

fn open_file(bs: &BootServices, path: &str) -> RegularFile {
    //info!("opening file: {}", path);
    let fs_handle = bs
        .get_handle_for_protocol::<SimpleFileSystem>()
        .expect("failed to get FileSystem handle");
    let mut fs = bs
        .open_protocol_exclusive::<SimpleFileSystem>(fs_handle)
        .expect("failed to open FileSystem protocol");
    let mut buf = [0u16; 256];
    let path = CStr16::from_str_with_buf(path, &mut buf).expect("failed to convert path to ucs-2");
    let mut root = fs.open_volume().expect("failed to open volume");
    let handle = root
        .open(path, FileMode::Read, FileAttribute::empty())
        .expect("failed to open file");

    match handle.into_type().expect("failed to into_type") {
        FileType::Regular(regular) => regular,
        _ => panic!("Invalid file type"),
    }
}

fn load_file(bs: &BootServices, file: &mut RegularFile) -> &'static mut [u8] {
    //info!("loading file to memory");
    let mut info_buf = [0u8; 0x100];
    let info = file
        .get_info::<FileInfo>(&mut info_buf)
        .expect("failed to get file info");
    let pages = info.file_size() as usize / 0x1000 + 1;
    let mem_start = bs
        .allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, pages)
        .expect("failed to allocate pages");
    let buf = unsafe { core::slice::from_raw_parts_mut(mem_start as *mut u8, pages * 0x1000) };
    let len = file.read(buf).expect("failed to read file");
    &mut buf[..len]
}

/// Return the handle of the best GOP to use.
///
/// On systems with multiple GPUs (e.g. dual NVIDIA RTX) UEFI exposes one GOP
/// handle per GPU.  `locate_protocol` returns an arbitrary one which may be
/// the inactive/secondary card.  We enumerate all handles and prefer the one
/// with the largest accessible (non-BltOnly) framebuffer, which is
/// consistently the active/connected display on tested systems.
fn find_active_gop_handle(bs: &BootServices) -> Option<Handle> {
    let handles = bs
        .locate_handle_buffer(SearchType::from_proto::<GraphicsOutput>())
        .ok()?;

    let mut best: Option<Handle> = None;
    let mut best_size: usize = 0;

    for &h in handles.iter() {
        if let Ok(mut gop) = bs.open_protocol_exclusive::<GraphicsOutput>(h) {
            // BltOnly means there is no direct framebuffer.
            if gop.current_mode_info().pixel_format() == PixelFormat::BltOnly {
                continue;
            }
            let sz = gop.frame_buffer().size();
            if sz > best_size {
                best_size = sz;
                best = Some(h);
            }
        }
    }

    // If every handle was BltOnly (no direct framebuffer), return None so the
    // caller can fall back to locate_protocol.
    best
}

fn init_graphic(
    bs: &BootServices,
    resolution: Option<(usize, usize)>,
) -> (GraphicInfo, [u8; 128], u32) {
    let gop_handle = find_active_gop_handle(bs)
        .or_else(|| bs.get_handle_for_protocol::<GraphicsOutput>().ok())
        .expect("failed to find GraphicsOutput handle");
    let mut gop = bs
        .open_protocol_exclusive::<GraphicsOutput>(gop_handle)
        .expect("failed to open GraphicsOutput protocol");

    if let Some(resolution) = resolution {
        let mode = gop
            .modes(bs)
            .find(|mode| {
                let info = mode.info();
                info.resolution() == resolution
            })
            .expect("graphic mode not found");
        //info!("switching graphic mode");
        gop.set_mode(&mode).expect("Failed to set graphics mode");
    }
    let info = GraphicInfo {
        mode: gop.current_mode_info(),
        fb_addr: gop.frame_buffer().as_mut_ptr() as u64,
        fb_size: gop.frame_buffer().size() as u64,
    };
    let (edid, edid_size) = read_active_edid(bs, gop_handle);
    (info, edid, edid_size)
}

/// `EFI_EDID_ACTIVE_PROTOCOL` — the raw EDID of the currently-active display,
/// as read by the firmware at power-on to set the GOP mode (UEFI 2.x §12.9).
/// The console monitor hangs off the GPU that drives the GOP, so this is the
/// user's real panel EDID — obtained without any GPU display bring-up.
#[repr(C)]
#[unsafe_protocol("bd8c1056-9f36-44ec-92a8-a6337f817986")]
struct EdidActiveProtocol {
    size_of_edid: u32,
    edid: *const u8,
}

/// `EFI_EDID_DISCOVERED_PROTOCOL` — EDID as read over DDC, exposed by some
/// firmwares even when the active protocol is absent. Same layout.
#[repr(C)]
#[unsafe_protocol("1c0c34f6-d380-41fa-a049-8ad06c1a66aa")]
struct EdidDiscoveredProtocol {
    size_of_edid: u32,
    edid: *const u8,
}

/// Read the active display's EDID (first 128-byte block). Tries the GOP
/// handle first (where the active-display EDID normally lives), then a global
/// lookup, then the discovered-EDID protocol. Returns `([0; 128], 0)` when no
/// EDID is available.
fn edid_header_ok(b: &[u8]) -> bool {
    b.len() >= 8 && b[0] == 0x00 && b[7] == 0x00 && b[1..7].iter().all(|&x| x == 0xFF)
}

fn read_active_edid(bs: &BootServices, gop_handle: uefi::Handle) -> ([u8; 128], u32) {
    // Copy one source's bytes into a fresh buffer; returns (buf, len).
    let read_one = |size: u32, ptr: *const u8| -> ([u8; 128], u32) {
        let mut buf = [0u8; 128];
        let n = (size as usize).min(128);
        if ptr.is_null() || n == 0 {
            return (buf, 0);
        }
        unsafe { core::ptr::copy_nonoverlapping(ptr, buf.as_mut_ptr(), n) };
        (buf, n as u32)
    };

    // All candidate sources, in order of preference. Some firmwares populate
    // the DISCOVERED protocol (raw DDC read) but leave ACTIVE empty, or expose
    // the protocol on a child handle rather than the GOP handle -- so gather
    // every candidate and pick the first with a valid EDID header.
    let mut first_nonempty: Option<([u8; 128], u32)> = None;
    let mut consider = |buf: [u8; 128], n: u32| -> Option<([u8; 128], u32)> {
        if n == 0 {
            return None;
        }
        if edid_header_ok(&buf[..n.min(128) as usize]) {
            return Some((buf, n));
        }
        if first_nonempty.is_none() {
            first_nonempty = Some((buf, n));
        }
        None
    };

    if let Ok(p) = bs.open_protocol_exclusive::<EdidActiveProtocol>(gop_handle) {
        let (b, n) = read_one(p.size_of_edid, p.edid);
        if let Some(v) = consider(b, n) {
            return v;
        }
    }
    if let Ok(h) = bs.get_handle_for_protocol::<EdidActiveProtocol>() {
        if let Ok(p) = bs.open_protocol_exclusive::<EdidActiveProtocol>(h) {
            let (b, n) = read_one(p.size_of_edid, p.edid);
            if let Some(v) = consider(b, n) {
                return v;
            }
        }
    }
    if let Ok(p) = bs.open_protocol_exclusive::<EdidDiscoveredProtocol>(gop_handle) {
        let (b, n) = read_one(p.size_of_edid, p.edid);
        if let Some(v) = consider(b, n) {
            return v;
        }
    }
    if let Ok(h) = bs.get_handle_for_protocol::<EdidDiscoveredProtocol>() {
        if let Ok(p) = bs.open_protocol_exclusive::<EdidDiscoveredProtocol>(h) {
            let (b, n) = read_one(p.size_of_edid, p.edid);
            if let Some(v) = consider(b, n) {
                return v;
            }
        }
    }
    // No source had a valid header; hand back the first non-empty capture (if
    // any) so /proc can dump it for diagnosis.
    first_nonempty.unwrap_or(([0u8; 128], 0))
}

fn current_page_table() -> OffsetPageTable<'static> {
    let p4_table_addr = Cr3::read().0.start_address().as_u64();
    let p4_table = unsafe { &mut *(p4_table_addr as *mut PageTable) };
    unsafe { OffsetPageTable::new(p4_table, VirtAddr::new(0)) }
}

struct UEFIFrameAllocator<'a>(&'a BootServices);

unsafe impl FrameAllocator<Size4KiB> for UEFIFrameAllocator<'_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let addr = self
            .0
            .allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 1)
            .expect("failed to allocate frame");
        let frame = PhysFrame::containing_address(PhysAddr::new(addr));
        Some(frame)
    }
}

unsafe fn jump_to_entry(bootinfo: *const BootInfo, stacktop: u64) -> ! {
    asm!(
        // Rust/x86_64 assumes DF=0 for string ops.
        "cld",
        // After ExitBootServices the firmware IDT/handlers are no longer valid.
        // Ensure we don't take any interrupt before the kernel installs its own IDT.
        "cli",
        // Clean frame pointer for a predictable initial state.
        "xor rbp, rbp",
        // NOTE: Avoid touching CET MSRs here. On many real machines, writing
        // IA32_S_CET/IA32_U_CET without explicit support triggers #GP in firmware
        // context. If CET/IBT turns out to be required, we should gate it behind
        // CPUID feature detection.
        // Set stack and call kernel entry with SysV ABI:
        // - RDI = bootinfo
        // - RSP 16-byte aligned before `call`
        // Use `jmp` with ABI-correct stack alignment (rsp%16==8 at entry).
        "mov rsp, {stacktop}",
        "sub rsp, 8",
        "jmp {entry}",
        stacktop = in(reg) stacktop,
        entry   = in(reg) ENTRY,
        in("rdi") bootinfo,
        options(noreturn),
    );
}

static mut ENTRY: usize = 0;
