#![cfg(feature = "std")]
//! apk/​libarchive extraction-pattern regression tests.
//!
//! The plain large-file tests (`large_files.rs`) prove a single sequential
//! write of a huge file is correct. Real `apk fix` does more than that when it
//! unpacks a package, and the "failed to extract libLLVM.so: I/O error" only
//! shows up in that fuller workflow. These tests model the pieces apk actually
//! performs so a regression in any of them is caught on the host instead of on
//! hardware:
//!   * pre-sizing a file with fallocate/ftruncate, then filling it;
//!   * extracting to a temp file and renaming it over the target (atomic
//!     replace) — and doing it *over an existing* file (a package upgrade,
//!     which is exactly what llvm22-libs is);
//!   * setting mode/owner/mtime on every extracted file;
//!   * a package that is many small files plus one very large one;
//!   * unpacking into a filesystem that is already populated (88 packages
//!     installed), so the allocator is not starting from a pristine state.
//!
//! Device: a *sparse* strict block device (page map) so a realistic 238 GiB
//! geometry — first data chunk only ~28 MiB, huge second chunk — costs almost
//! no memory.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use btrfs::device::BlockDevice;
use btrfs::{mkfs, Btrfs, Error, FileKind, Result};

const PAGE: usize = 4096;

/// Sparse, strictly-bounded block device. Only written pages cost memory, so a
/// 238 GiB device is free until something is actually written to it. Any access
/// past `size` fails with `Io`, like real hardware rejecting an out-of-range
/// transfer (a plain growable file would hide geometry bugs).
struct SparseDevice {
    pages: Mutex<HashMap<u64, Box<[u8; PAGE]>>>,
    size: u64,
    oob: AtomicU64,
}

impl SparseDevice {
    fn new(size: u64) -> Arc<Self> {
        Arc::new(Self {
            pages: Mutex::new(HashMap::new()),
            size,
            oob: AtomicU64::new(0),
        })
    }
}

impl BlockDevice for SparseDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if offset + buf.len() as u64 > self.size {
            self.oob.fetch_add(1, Ordering::Relaxed);
            return Err(Error::Io);
        }
        let pages = self.pages.lock().unwrap();
        let mut done = 0usize;
        while done < buf.len() {
            let abs = offset + done as u64;
            let pno = abs / PAGE as u64;
            let poff = (abs % PAGE as u64) as usize;
            let n = (PAGE - poff).min(buf.len() - done);
            match pages.get(&pno) {
                Some(p) => buf[done..done + n].copy_from_slice(&p[poff..poff + n]),
                None => buf[done..done + n].fill(0), // unwritten reads as zero
            }
            done += n;
        }
        Ok(())
    }
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        if offset + buf.len() as u64 > self.size {
            self.oob.fetch_add(1, Ordering::Relaxed);
            return Err(Error::Io);
        }
        let mut pages = self.pages.lock().unwrap();
        let mut done = 0usize;
        while done < buf.len() {
            let abs = offset + done as u64;
            let pno = abs / PAGE as u64;
            let poff = (abs % PAGE as u64) as usize;
            let n = (PAGE - poff).min(buf.len() - done);
            let p = pages.entry(pno).or_insert_with(|| Box::new([0u8; PAGE]));
            p[poff..poff + n].copy_from_slice(&buf[done..done + n]);
            done += n;
        }
        Ok(())
    }
    fn sync(&self) -> Result<()> {
        Ok(())
    }
    fn size(&self) -> u64 {
        self.size
    }
}

fn opts() -> mkfs::MkfsOptions {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
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

/// Real rootfs geometry: 238 GiB device, formatted in place.
fn mount_real() -> (Arc<SparseDevice>, Btrfs) {
    let dev = SparseDevice::new(238 * 1024 * 1024 * 1024);
    mkfs::format(&*dev, &opts()).unwrap();
    let arc: Arc<dyn BlockDevice> = dev.clone();
    let fs = Btrfs::mount(arc, false).unwrap();
    (dev, fs)
}

fn payload_byte(salt: u64, i: u64) -> u8 {
    let x = (i ^ salt.rotate_left(17))
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (x >> 33) as u8
}

/// Write `size` bytes of salted pseudo-random data into `ino` in `chunk`-sized
/// writes, like an archive extractor streaming a member.
fn fill(fs: &mut Btrfs, ino: u64, size: u64, salt: u64, chunk: usize) {
    let mut buf = vec![0u8; chunk];
    let mut off = 0u64;
    while off < size {
        let n = (chunk as u64).min(size - off) as usize;
        for j in 0..n {
            buf[j] = payload_byte(salt, off + j as u64);
        }
        let w = fs
            .write(ino, off, &buf[..n])
            .unwrap_or_else(|e| panic!("write @{off} (size {size}) failed: {e:?}"));
        assert_eq!(w, n, "short write @{off}");
        off += n as u64;
    }
}

fn verify(fs: &mut Btrfs, ino: u64, size: u64, salt: u64) {
    assert_eq!(fs.stat(ino).unwrap().size, size, "size mismatch");
    let chunk = 64 * 1024usize;
    let mut rbuf = vec![0u8; chunk];
    let mut off = 0u64;
    while off < size {
        let want = (chunk as u64).min(size - off) as usize;
        let mut got = 0usize;
        while got < want {
            let n = fs
                .read(ino, off + got as u64, &mut rbuf[got..want])
                .unwrap();
            assert!(n > 0, "zero read @{}", off + got as u64);
            got += n;
        }
        for j in 0..want {
            assert_eq!(
                rbuf[j],
                payload_byte(salt, off + j as u64),
                "data mismatch @{}",
                off + j as u64
            );
        }
        off += want as u64;
    }
}

const LIBLLVM: u64 = 130 * 1024 * 1024;

/// apk pre-sizes a big file with fallocate (== truncate-up in our VFS), then
/// streams the data in, fsyncs, and sets mode — the exact sequence that turns
/// the file into one big hole that each write must fill.
#[test]
fn fallocate_then_fill_then_chmod() {
    let (_dev, mut fs) = mount_real();
    let root = fs.root_ino();
    let f = fs
        .create(root, "libLLVM.so.22.1", FileKind::Regular, 0o644, 0)
        .unwrap();
    fs.truncate(f, LIBLLVM).unwrap(); // fallocate / ftruncate up front
    fill(&mut fs, f, LIBLLVM, 1, 64 * 1024);
    fs.set_attr(
        f,
        Some(0o755),
        Some(0),
        Some(0),
        None,
        Some((1_700_000_100, 0)),
    )
    .unwrap();
    fs.sync().unwrap();
    verify(&mut fs, f, LIBLLVM, 1);
}

/// apk extracts to a temporary name, fsyncs, then renames it over the final
/// path. Here the final path is a *pre-existing smaller* file — a package
/// upgrade, which is exactly what llvm22-libs is.
#[test]
fn upgrade_temp_then_rename_over_existing() {
    let (_dev, mut fs) = mount_real();
    let root = fs.root_ino();

    // Old version already installed.
    let old = fs
        .create(root, "libLLVM.so.22.1", FileKind::Regular, 0o755, 0)
        .unwrap();
    fill(&mut fs, old, 40 * 1024 * 1024, 7, 64 * 1024);
    fs.sync().unwrap();

    // New version extracted to a temp name, then renamed over the old.
    let tmp = fs
        .create(root, "libLLVM.so.22.1.apk-new", FileKind::Regular, 0o644, 0)
        .unwrap();
    fill(&mut fs, tmp, LIBLLVM, 9, 64 * 1024);
    fs.sync().unwrap();
    fs.rename(root, "libLLVM.so.22.1.apk-new", root, "libLLVM.so.22.1")
        .unwrap();
    fs.sync().unwrap();

    let now = fs.lookup(root, "libLLVM.so.22.1").unwrap();
    verify(&mut fs, now, LIBLLVM, 9); // must be the NEW content
}

/// Overwrite a large file in place: truncate to zero then re-stream (another
/// common replace strategy). Exercises freeing all extents then re-allocating.
#[test]
fn overwrite_in_place_truncate_zero_then_refill() {
    let (_dev, mut fs) = mount_real();
    let root = fs.root_ino();
    let f = fs
        .create(root, "lib.so", FileKind::Regular, 0o644, 0)
        .unwrap();
    fill(&mut fs, f, 50 * 1024 * 1024, 3, 64 * 1024);
    fs.sync().unwrap();
    fs.truncate(f, 0).unwrap(); // drop all extents
    fill(&mut fs, f, LIBLLVM, 4, 64 * 1024);
    fs.sync().unwrap();
    verify(&mut fs, f, LIBLLVM, 4);
}

/// A whole package: dozens of small files plus the one huge shared object, each
/// written then chmod'd, all under one extraction, fsync at the end.
#[test]
fn package_many_small_plus_one_huge() {
    let (_dev, mut fs) = mount_real();
    let root = fs.root_ino();
    let dir = fs.create(root, "usr", FileKind::Dir, 0o755, 0).unwrap();
    let lib = fs.create(dir, "lib", FileKind::Dir, 0o755, 0).unwrap();

    let mut smalls = Vec::new();
    for i in 0..64u64 {
        let name = format!("part{i}.so");
        let f = fs.create(lib, &name, FileKind::Regular, 0o644, 0).unwrap();
        let sz = 4096 + (i * 9973) % (512 * 1024);
        fill(&mut fs, f, sz, 100 + i, 16 * 1024);
        fs.set_attr(f, Some(0o755), Some(0), Some(0), None, None)
            .unwrap();
        smalls.push((f, sz, 100 + i));
    }
    let big = fs
        .create(lib, "libLLVM.so.22.1", FileKind::Regular, 0o644, 0)
        .unwrap();
    fill(&mut fs, big, LIBLLVM, 42, 64 * 1024);
    fs.set_attr(big, Some(0o755), Some(0), Some(0), None, None)
        .unwrap();
    fs.sync().unwrap();

    for (f, sz, salt) in smalls {
        verify(&mut fs, f, sz, salt);
    }
    verify(&mut fs, big, LIBLLVM, 42);
}

/// Unpack into an already-populated filesystem: write ~300 MiB across many
/// files first (≈ the 88 packages already installed), THEN extract the huge
/// file, so the allocator and chunks are not starting pristine.
#[test]
fn prefill_then_extract_huge() {
    let (_dev, mut fs) = mount_real();
    let root = fs.root_ino();

    // Existing install: 150 files totalling ~300 MiB, spanning several chunks.
    for i in 0..150u64 {
        let name = format!("pkgfile{i}.bin");
        let f = fs.create(root, &name, FileKind::Regular, 0o644, 0).unwrap();
        fill(&mut fs, f, 2 * 1024 * 1024, 200 + i, 64 * 1024);
        if i % 16 == 0 {
            fs.sync().unwrap();
        }
    }
    fs.sync().unwrap();

    let big = fs
        .create(root, "libLLVM.so.22.1", FileKind::Regular, 0o644, 0)
        .unwrap();
    fill(&mut fs, big, LIBLLVM, 55, 64 * 1024);
    fs.sync().unwrap();
    verify(&mut fs, big, LIBLLVM, 55);
}

/// Upgrade that frees space first: delete several installed files (freeing their
/// extents), then extract the huge file so the allocator must reuse the freed,
/// fragmented space — the fragmentation pattern a real upgrade produces.
#[test]
fn delete_to_free_then_extract_huge() {
    let (_dev, mut fs) = mount_real();
    let root = fs.root_ino();

    let mut names = Vec::new();
    for i in 0..80u64 {
        let name = format!("old{i}.bin");
        let f = fs.create(root, &name, FileKind::Regular, 0o644, 0).unwrap();
        fill(&mut fs, f, 1024 * 1024, 300 + i, 64 * 1024);
        names.push(name);
    }
    fs.sync().unwrap();
    // Free every other file → fragmented free space.
    for (i, name) in names.iter().enumerate() {
        if i % 2 == 0 {
            fs.unlink(root, name).unwrap();
        }
    }
    fs.sync().unwrap();

    let big = fs
        .create(root, "libLLVM.so.22.1", FileKind::Regular, 0o644, 0)
        .unwrap();
    fill(&mut fs, big, LIBLLVM, 66, 64 * 1024);
    fs.sync().unwrap();
    verify(&mut fs, big, LIBLLVM, 66);
}

/// Stream a big file with apk-style small (4 KiB) writes and an fsync every few
/// MiB (apk fsyncs periodically), across the real geometry.
#[test]
fn small_writes_periodic_fsync() {
    let (_dev, mut fs) = mount_real();
    let root = fs.root_ino();
    let f = fs
        .create(root, "libLLVM.so.22.1", FileKind::Regular, 0o644, 0)
        .unwrap();

    let chunk = 4096usize;
    let mut buf = vec![0u8; chunk];
    let mut off = 0u64;
    let mut since_sync = 0u64;
    while off < LIBLLVM {
        let n = (chunk as u64).min(LIBLLVM - off) as usize;
        for j in 0..n {
            buf[j] = payload_byte(77, off + j as u64);
        }
        fs.write(f, off, &buf[..n]).unwrap();
        off += n as u64;
        since_sync += n as u64;
        if since_sync >= 4 * 1024 * 1024 {
            fs.sync().unwrap();
            since_sync = 0;
        }
    }
    fs.sync().unwrap();
    verify(&mut fs, f, LIBLLVM, 77);
}

/// Durability: extract the package, fsync, DROP the filesystem, then re-mount
/// the same device and verify every byte survived. This is the crash-
/// consistency path apk relies on (commit + superblock write-back); immediate-
/// verify tests miss a broken commit because the data is still in the live
/// trees. The sparse device keeps its pages across the remount.
#[test]
fn extract_sync_remount_persists() {
    let dev = SparseDevice::new(238 * 1024 * 1024 * 1024);
    mkfs::format(&*dev, &opts()).unwrap();

    // Session 1: install a few small files and the huge one, then fsync + drop.
    {
        let arc: Arc<dyn BlockDevice> = dev.clone();
        let mut fs = Btrfs::mount(arc, false).unwrap();
        let root = fs.root_ino();
        for i in 0..8u64 {
            let f = fs
                .create(root, &format!("s{i}"), FileKind::Regular, 0o644, 0)
                .unwrap();
            fill(&mut fs, f, 256 * 1024, 500 + i, 64 * 1024);
        }
        let big = fs
            .create(root, "libLLVM.so.22.1", FileKind::Regular, 0o755, 0)
            .unwrap();
        fill(&mut fs, big, LIBLLVM, 88, 64 * 1024);
        fs.sync().unwrap();
        // fs dropped here — nothing else flushes.
    }

    // Session 2: fresh mount of the same bytes. Everything must be present.
    {
        let arc: Arc<dyn BlockDevice> = dev.clone();
        let mut fs = Btrfs::mount(arc, false).unwrap();
        let root = fs.root_ino();
        for i in 0..8u64 {
            let f = fs.lookup(root, &format!("s{i}")).unwrap();
            verify(&mut fs, f, 256 * 1024, 500 + i);
        }
        let big = fs.lookup(root, "libLLVM.so.22.1").unwrap();
        verify(&mut fs, big, LIBLLVM, 88);
    }
    assert_eq!(
        dev.oob.load(Ordering::Relaxed),
        0,
        "out-of-bounds device access"
    );
}

/// Repeated upgrades of the same large file (install, then upgrade several
/// times), each via temp-file + rename, with the old version unlinked. Churns
/// allocate/free of large extents so a leak or free-space-accounting drift
/// surfaces; verifies the final content and that the fs still remounts clean.
#[test]
fn repeated_large_upgrades_churn() {
    let dev = SparseDevice::new(238 * 1024 * 1024 * 1024);
    mkfs::format(&*dev, &opts()).unwrap();
    let arc: Arc<dyn BlockDevice> = dev.clone();
    let mut fs = Btrfs::mount(arc, false).unwrap();
    let root = fs.root_ino();

    let mut salt = 1000u64;
    // Initial install.
    let f = fs
        .create(root, "libLLVM.so.22.1", FileKind::Regular, 0o755, 0)
        .unwrap();
    fill(&mut fs, f, LIBLLVM, salt, 64 * 1024);
    fs.sync().unwrap();

    // Five upgrade cycles: new temp, fill, rename over, sync.
    for _ in 0..5 {
        salt += 1;
        let tmp = fs
            .create(root, "libLLVM.so.22.1.apk-new", FileKind::Regular, 0o644, 0)
            .unwrap();
        fill(&mut fs, tmp, LIBLLVM, salt, 64 * 1024);
        fs.sync().unwrap();
        fs.rename(root, "libLLVM.so.22.1.apk-new", root, "libLLVM.so.22.1")
            .unwrap();
        fs.sync().unwrap();
    }
    let cur = fs.lookup(root, "libLLVM.so.22.1").unwrap();
    verify(&mut fs, cur, LIBLLVM, salt);
    drop(fs);

    // Remount: the churned filesystem is still consistent and keeps the latest.
    let arc2: Arc<dyn BlockDevice> = dev.clone();
    let mut fs2 = Btrfs::mount(arc2, false).unwrap();
    let root2 = fs2.root_ino();
    let cur2 = fs2.lookup(root2, "libLLVM.so.22.1").unwrap();
    verify(&mut fs2, cur2, LIBLLVM, salt);
    assert_eq!(
        dev.oob.load(Ordering::Relaxed),
        0,
        "out-of-bounds device access"
    );
}
