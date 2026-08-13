#![cfg(feature = "std")]
//! Regression tests for the per-inode extent read-cache under *scattered*
//! (non-sequential) reads — the exact access pattern of a dynamic linker
//! demand-paging a large mmap'd `.so`.
//!
//! The bug this pins down: `Btrfs::read`'s jump path cleared the cached
//! extent list and rescanned from the jump offset, but then filtered out the
//! extent that STARTS BEFORE the jump offset yet spans into the requested
//! window (`e.0 >= scan_from`), so the window read back as zeros. Worse, the
//! `end > cached_end` guard then served any LOWER offset entirely from the
//! (now cleared) cache — so re-reading a library's ELF header at offset 0
//! after a scattered fault returned zeros: musl's ldso saw an invalid magic
//! and failed every library with "Exec format error" (ENOEXEC), which is how
//! it presented on the installed btrfs system (labwc respawning with a
//! growing set of unloadable libraries as more inode caches were poisoned).
//! QEMU's live root is not btrfs, so it never reproduced there.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::Arc;
use std::{env, fs};

use btrfs::device::BlockDevice;
use btrfs::{mkfs, Btrfs, Error, FileKind, Result};

/// Minimal file-backed device (no strict OOB semantics needed here — these
/// tests exercise read-cache logic, not geometry).
struct FileDevice {
    file: Mutex<fs::File>,
    size: u64,
}

impl FileDevice {
    fn create(path: &PathBuf, size: u64) -> Arc<Self> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .unwrap();
        file.set_len(size).unwrap();
        Arc::new(Self {
            file: Mutex::new(file),
            size,
        })
    }
}

impl BlockDevice for FileDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(offset)).map_err(|_| Error::Io)?;
        f.read_exact(buf).map_err(|_| Error::Io)
    }
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(offset)).map_err(|_| Error::Io)?;
        f.write_all(buf).map_err(|_| Error::Io)
    }
    fn sync(&self) -> Result<()> {
        self.file.lock().unwrap().sync_data().map_err(|_| Error::Io)
    }
    fn size(&self) -> u64 {
        self.size
    }
}

fn opts() -> mkfs::MkfsOptions {
    let mut seed = 0xfeed_beef_dead_c0deu64;
    let mut uuid = || {
        let mut u = [0u8; 16];
        for b in u.iter_mut() {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *b = (seed >> 33) as u8;
        }
        u[6] = (u[6] & 0x0f) | 0x40;
        u[8] = (u[8] & 0x3f) | 0x80;
        u
    };
    mkfs::MkfsOptions {
        label: "eclipse".into(),
        fsid: uuid(),
        chunk_uuid: uuid(),
        dev_uuid: uuid(),
        subvol_uuid: uuid(),
        now: (1_700_000_000, 0),
    }
}

fn tmp(name: &str) -> PathBuf {
    env::temp_dir().join(format!("btrfs-scattered-{}-{}", std::process::id(), name))
}

/// Reproducible payload byte for offset `i` (same generator as large_files.rs).
fn payload_byte(i: u64) -> u8 {
    let x = i
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (x >> 33) as u8
}

/// Build an image with one multi-extent file of `file_size` bytes (written in
/// 64 KiB chunks like an archive extractor), remount read-only-ish fresh, and
/// hand it to the caller.
fn setup(name: &str, file_size: u64) -> (Btrfs, u64, PathBuf) {
    let path = tmp(name);
    let dev = FileDevice::create(&path, 64 * 1024 * 1024);
    mkfs::format(&*dev, &opts()).unwrap();
    let dev_arc: Arc<dyn BlockDevice> = dev.clone();
    let mut fs = Btrfs::mount(dev_arc, false).unwrap();
    let root = fs.root_ino();
    let file = fs
        .create(root, "libwlroots.so", FileKind::Regular, 0o755, 0)
        .unwrap();
    let chunk = 64 * 1024usize;
    let mut buf = vec![0u8; chunk];
    let mut off = 0u64;
    while off < file_size {
        let n = (chunk as u64).min(file_size - off) as usize;
        for (j, b) in buf[..n].iter_mut().enumerate() {
            *b = payload_byte(off + j as u64);
        }
        let w = fs.write(file, off, &buf[..n]).unwrap();
        assert_eq!(w, n, "short write at {}", off);
        off += n as u64;
    }
    fs.sync().unwrap();
    (fs, file, path)
}

/// Read exactly `len` bytes at `offset` (looping over short reads) and assert
/// every byte matches the reproducible payload.
fn read_and_verify(fs: &mut Btrfs, file: u64, offset: u64, len: usize, what: &str) {
    let mut buf = vec![0u8; len];
    let mut got = 0usize;
    while got < len {
        let n = fs.read(file, offset + got as u64, &mut buf[got..]).unwrap();
        assert!(n > 0, "zero read at {} ({})", offset + got as u64, what);
        got += n;
    }
    for (j, &b) in buf.iter().enumerate() {
        assert_eq!(
            b,
            payload_byte(offset + j as u64),
            "{}: data mismatch at offset {} (byte {} of the read)",
            what,
            offset + j as u64,
            j
        );
    }
}

/// The ldso pattern distilled: a page fault lands deep inside the library
/// (jump forward past the cached range), then the ELF header is re-read at
/// offset 0. Before the fix, the jump dropped the covering extent (window read
/// zeros) and the later low read was served from the cleared cache (zeros →
/// ENOEXEC in musl).
#[test]
fn jump_forward_then_reread_header() {
    let (mut fs, file, path) = setup("jump-header", 8 * 1024 * 1024);

    // 1. Scattered fault: one page deep inside the file. With a fresh cache
    //    this is a jump (offset > cached_end=0), exercising the clear+rescan.
    read_and_verify(&mut fs, file, 6 * 1024 * 1024 + 4096, 4096, "deep fault");

    // 2. Header re-read at offset 0 — end (64) <= cached_end, so the buggy
    //    guard served it from the jump-cleared cache: all zeros.
    read_and_verify(&mut fs, file, 0, 64, "ELF header re-read");

    // 3. And a full first page, still below the jump point.
    read_and_verify(&mut fs, file, 0, 4096, "first page");

    let _ = fs;
    let _ = std::fs::remove_file(&path);
}

/// Many scattered single-page reads in fault-like order (high/low alternating,
/// none sequential), each verified. Mimics relocation processing walking a
/// library's symbol/reloc sections out of file order.
#[test]
fn scattered_page_faults_all_verify() {
    let file_size = 8 * 1024 * 1024u64;
    let (mut fs, file, path) = setup("scattered", file_size);

    let page = 4096u64;
    // Deterministic scattered sequence: stride around the file, alternating
    // high/low, never sequential.
    let offsets = [
        5 * 1024 * 1024,
        64 * 1024,
        7 * 1024 * 1024 + 8 * 1024,
        0,
        3 * 1024 * 1024 + 4096,
        128 * 1024,
        6 * 1024 * 1024,
        4096,
        2 * 1024 * 1024,
        7 * 1024 * 1024,
        512 * 1024,
        1024 * 1024,
    ];
    for (i, &off) in offsets.iter().enumerate() {
        assert!(off + page <= file_size);
        read_and_verify(&mut fs, file, off, page as usize, &format!("fault #{i} @{off:#x}"));
    }

    let _ = fs;
    let _ = std::fs::remove_file(&path);
}

/// Sequential streaming still verifies end-to-end after a scattered prelude —
/// the cache must not lose extents needed by a later linear pass (apk-style
/// extraction reading the file front to back).
#[test]
fn sequential_after_scatter_verifies() {
    let file_size = 4 * 1024 * 1024u64;
    let (mut fs, file, path) = setup("seq-after-scatter", file_size);

    // Scattered prelude.
    read_and_verify(&mut fs, file, 3 * 1024 * 1024, 4096, "prelude high");
    read_and_verify(&mut fs, file, 16 * 1024, 4096, "prelude low");

    // Full linear pass.
    let chunk = 64 * 1024usize;
    let mut off = 0u64;
    while off < file_size {
        let n = (chunk as u64).min(file_size - off) as usize;
        read_and_verify(&mut fs, file, off, n, "linear pass");
        off += n as u64;
    }

    let _ = fs;
    let _ = std::fs::remove_file(&path);
}
