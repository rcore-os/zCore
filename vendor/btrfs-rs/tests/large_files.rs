#![cfg(feature = "std")]
//! Large / huge single-file regression tests for the "I/O error on big files"
//! class of bugs (e.g. `apk fix` failing to extract the ~130 MiB `libLLVM.so`).
//!
//! The key trick: a *strict* block device that returns `Error::Io` for any
//! access outside `[0, size)`, exactly like real hardware rejecting a transfer
//! past the end of the disk. A plain file-backed device silently extends the
//! file on an out-of-bounds write, so geometry/allocation bugs that only bite
//! on real disks stay invisible. With the strict device they reproduce here.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::{env, fs};

use btrfs::device::BlockDevice;
use btrfs::{mkfs, Btrfs, Error, FileKind, Result};

/// File-backed device that strictly enforces its advertised size: any read or
/// write touching `[size, ..)` fails with `Io`, like a real disk would.
struct StrictDevice {
    file: Mutex<fs::File>,
    size: u64,
    oob_reads: AtomicU64,
    oob_writes: AtomicU64,
}

impl StrictDevice {
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
            oob_reads: AtomicU64::new(0),
            oob_writes: AtomicU64::new(0),
        })
    }
}

impl BlockDevice for StrictDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if offset + buf.len() as u64 > self.size {
            self.oob_reads.fetch_add(1, Ordering::Relaxed);
            return Err(Error::Io);
        }
        use std::io::{Read, Seek, SeekFrom};
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(offset)).map_err(|_| Error::Io)?;
        f.read_exact(buf).map_err(|_| Error::Io)
    }
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        if offset + buf.len() as u64 > self.size {
            self.oob_writes.fetch_add(1, Ordering::Relaxed);
            return Err(Error::Io);
        }
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
    let mut seed = 0x1234_5678_9abc_def0u64;
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
    env::temp_dir().join(format!("btrfs-large-{}-{}", std::process::id(), name))
}

fn have_progs() -> bool {
    std::process::Command::new("btrfs")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn check(path: &PathBuf) {
    if !have_progs() {
        return;
    }
    let out = std::process::Command::new("btrfs")
        .args(["check", "--force"])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "btrfs check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Pseudo-random but reproducible payload byte for (file, offset).
fn payload_byte(i: u64) -> u8 {
    let x = i
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (x >> 33) as u8
}

/// Write a single huge file in small chunks (mimicking apk/libarchive
/// extraction) on a device that strictly rejects out-of-bounds I/O, then read
/// it back and run `btrfs check`.
fn run_huge_file(name: &str, dev_size: u64, file_size: u64, grow_from: Option<u64>) {
    // Each test gets its own temp paths; several tests call this helper and the
    // suite runs them in parallel, so a shared path would let them clobber each
    // other's image (a flaky failure unrelated to the filesystem).
    let path = tmp(name);
    // If we simulate the installer, format a small image then present it on a
    // larger device and grow; otherwise format the whole device.
    let dev = StrictDevice::create(&path, dev_size);

    if let Some(small) = grow_from {
        // Format only within the first `small` bytes, then grow to dev_size.
        let ignored = tmp(&format!("{name}-ignored"));
        let small_dev = StrictDevice::create(&ignored, small);
        // Actually format directly on the big device but pretend it was small:
        // mkfs writes a superblock sized to `small`. Simplest: format a small
        // device file, copy it over the big one's head, then mount big.
        mkfs::format(&*small_dev, &opts()).unwrap();
        // Copy the formatted head onto the big device.
        let head = {
            let mut b = vec![0u8; small as usize];
            small_dev.read_at(0, &mut b).unwrap();
            b
        };
        dev.write_at(0, &head).unwrap();
        let _ = fs::remove_file(&ignored);
    } else {
        mkfs::format(&*dev, &opts()).unwrap();
    }

    let dev_arc: Arc<dyn BlockDevice> = dev.clone();
    let mut fs = Btrfs::mount(dev_arc, false).unwrap();
    if grow_from.is_some() {
        assert!(fs.grow_to_device().unwrap(), "grow_to_device should expand");
    }
    let root = fs.root_ino();
    let file = fs
        .create(root, "libLLVM.so", FileKind::Regular, 0o755, 0)
        .unwrap();

    // Write in 64 KiB chunks like an archive extractor.
    let chunk = 64 * 1024usize;
    let mut buf = vec![0u8; chunk];
    let mut off = 0u64;
    while off < file_size {
        let n = (chunk as u64).min(file_size - off) as usize;
        for j in 0..n {
            buf[j] = payload_byte(off + j as u64);
        }
        let w = fs.write(file, off, &buf[..n]).unwrap_or_else(|e| {
            panic!(
                "write at {} (of {}) failed: {:?} (oob_writes={}, oob_reads={})",
                off,
                file_size,
                e,
                dev.oob_writes.load(Ordering::Relaxed),
                dev.oob_reads.load(Ordering::Relaxed),
            )
        });
        assert_eq!(w, n, "short write at {}", off);
        off += n as u64;
    }
    fs.sync().unwrap();

    // Read it back and verify.
    let st = fs.stat(file).unwrap();
    assert_eq!(st.size, file_size, "size mismatch");
    let mut rbuf = vec![0u8; chunk];
    let mut off = 0u64;
    while off < file_size {
        let want = (chunk as u64).min(file_size - off) as usize;
        let mut got = 0usize;
        while got < want {
            let n = fs
                .read(file, off + got as u64, &mut rbuf[got..want])
                .unwrap();
            assert!(n > 0, "zero read at {}", off + got as u64);
            got += n;
        }
        for j in 0..want {
            assert_eq!(
                rbuf[j],
                payload_byte(off + j as u64),
                "data mismatch at {}",
                off + j as u64
            );
        }
        off += want as u64;
    }

    assert_eq!(
        dev.oob_writes.load(Ordering::Relaxed),
        0,
        "out-of-bounds writes"
    );
    assert_eq!(
        dev.oob_reads.load(Ordering::Relaxed),
        0,
        "out-of-bounds reads"
    );
    drop(fs);
    check(&path);
    fs::remove_file(&path).ok();
}

/// 200 MiB file on a 1 GiB device formatted in place — crosses several 256 MiB
/// data chunks and forces new chunk allocations mid-file.
#[test]
fn huge_file_plain() {
    run_huge_file("plain", 1024 * 1024 * 1024, 200 * 1024 * 1024, None);
}

/// The real installer flow: a 64 MiB image grown onto a 1 GiB partition, then a
/// 200 MiB file written into the grown space (where geometry bugs hide).
#[test]
fn huge_file_after_grow() {
    run_huge_file(
        "after-grow",
        1024 * 1024 * 1024,
        200 * 1024 * 1024,
        Some(64 * 1024 * 1024),
    );
}

/// A file larger than a single data chunk (256 MiB) on a tight device, so the
/// allocator must stitch multiple chunks and the device has little slack.
#[test]
fn file_spanning_multiple_data_chunks() {
    run_huge_file("multichunk", 700 * 1024 * 1024, 300 * 1024 * 1024, None);
}

/// Reproduce the "apk hangs on big packages" symptom: write a large file in
/// SMALL sequential chunks (like libarchive's block-by-block extraction) and
/// report throughput as the file grows. A super-linear slowdown (throughput
/// collapsing as size increases) is the stall; steady throughput rules out an
/// O(n^2) write path.
#[test]
fn big_file_small_chunks_scaling() {
    use std::time::Instant;
    let chunk = 4096usize; // libarchive-style small writes
    let total = 64 * 1024 * 1024u64;
    let step = 8 * 1024 * 1024u64; // report every 8 MiB
    println!("\n[big-file-small-chunks] 64 MiB written in 4 KiB chunks");
    let path = tmp("smallchunks");
    let dev = StrictDevice::create(&path, 512 * 1024 * 1024);
    mkfs::format(&*dev, &opts()).unwrap();
    let dev_arc: Arc<dyn BlockDevice> = dev.clone();
    let mut fs = Btrfs::mount(dev_arc, false).unwrap();
    let root = fs.root_ino();
    let file = fs.create(root, "big", FileKind::Regular, 0o644, 0).unwrap();

    let buf = vec![0xa5u8; chunk];
    let mut off = 0u64;
    let mut last_mark = 0u64;
    let mut mark_t = Instant::now();
    println!(
        "    {:>8}  {:>12}  {:>14}",
        "MiB", "MiB/s (window)", "us/4KiB-write"
    );
    while off < total {
        fs.write(file, off, &buf).unwrap();
        off += chunk as u64;
        if off - last_mark >= step {
            let secs = mark_t.elapsed().as_secs_f64();
            let mibps = (off - last_mark) as f64 / (1024.0 * 1024.0) / secs;
            let us = secs * 1e6 / ((off - last_mark) / chunk as u64) as f64;
            println!(
                "    {:>8}  {:>12.1}  {:>14.2}",
                off / (1024 * 1024),
                mibps,
                us
            );
            last_mark = off;
            mark_t = Instant::now();
        }
    }
    fs.sync().unwrap();
    drop(fs);
    fs::remove_file(&path).ok();
}

/// `ftruncate(size)` up-front then sequential writes (a common extractor
/// pattern): the file is one big hole that each write must convert to real
/// extents. Exercises the hole-extent path at scale on a strict device.
#[test]
fn huge_file_truncate_up_then_write() {
    let path = tmp("trunc");
    let dev = StrictDevice::create(&path, 1024 * 1024 * 1024);
    mkfs::format(&*dev, &opts()).unwrap();
    let dev_arc: Arc<dyn BlockDevice> = dev.clone();
    let mut fs = Btrfs::mount(dev_arc, false).unwrap();
    let root = fs.root_ino();
    let file = fs.create(root, "big", FileKind::Regular, 0o644, 0).unwrap();

    let size = 160 * 1024 * 1024u64;
    fs.truncate(file, size).unwrap(); // one giant hole

    let chunk = 64 * 1024usize;
    let mut buf = vec![0u8; chunk];
    let mut off = 0u64;
    while off < size {
        let n = (chunk as u64).min(size - off) as usize;
        for j in 0..n {
            buf[j] = payload_byte(off + j as u64);
        }
        let w = fs.write(file, off, &buf[..n]).unwrap_or_else(|e| {
            panic!(
                "write at {} failed: {:?} (oob_w={}, oob_r={})",
                off,
                e,
                dev.oob_writes.load(Ordering::Relaxed),
                dev.oob_reads.load(Ordering::Relaxed)
            )
        });
        assert_eq!(w, n);
        off += n as u64;
    }
    fs.sync().unwrap();

    let mut rbuf = vec![0u8; chunk];
    let mut off = 0u64;
    while off < size {
        let want = (chunk as u64).min(size - off) as usize;
        let mut got = 0usize;
        while got < want {
            got += fs
                .read(file, off + got as u64, &mut rbuf[got..want])
                .unwrap();
        }
        for j in 0..want {
            assert_eq!(
                rbuf[j],
                payload_byte(off + j as u64),
                "mismatch at {}",
                off + j as u64
            );
        }
        off += want as u64;
    }
    assert_eq!(dev.oob_writes.load(Ordering::Relaxed), 0);
    assert_eq!(dev.oob_reads.load(Ordering::Relaxed), 0);
    drop(fs);
    check(&path);
    fs::remove_file(&path).ok();
}

/// Sparse extraction: write large chunks at scattered, increasing offsets
/// leaving holes in between (tar/sparse-file pattern), then verify both data
/// and holes on a strict device.
#[test]
fn huge_file_sparse_writes() {
    let path = tmp("sparse");
    let dev = StrictDevice::create(&path, 1024 * 1024 * 1024);
    mkfs::format(&*dev, &opts()).unwrap();
    let dev_arc: Arc<dyn BlockDevice> = dev.clone();
    let mut fs = Btrfs::mount(dev_arc, false).unwrap();
    let root = fs.root_ino();
    let file = fs
        .create(root, "sparse", FileKind::Regular, 0o644, 0)
        .unwrap();

    // 200 data segments of 256 KiB, each separated by a 256 KiB hole.
    let seg = 256 * 1024usize;
    let segments = 200u64;
    let stride = 2 * seg as u64;
    let mut buf = vec![0u8; seg];
    for s in 0..segments {
        let off = s * stride;
        for j in 0..seg {
            buf[j] = payload_byte(off + j as u64);
        }
        let mut w = 0usize;
        while w < seg {
            w += fs.write(file, off + w as u64, &buf[w..]).unwrap();
        }
    }
    fs.sync().unwrap();

    let mut rbuf = vec![0u8; seg];
    for s in 0..segments {
        let off = s * stride;
        // data segment
        let mut got = 0usize;
        while got < seg {
            let n = fs.read(file, off + got as u64, &mut rbuf[got..]).unwrap();
            assert!(n > 0, "short read in data segment at {}", off + got as u64);
            got += n;
        }
        for j in 0..seg {
            assert_eq!(
                rbuf[j],
                payload_byte(off + j as u64),
                "data mismatch at {}",
                off + j as u64
            );
        }
        // The hole following every segment except the last (which is at EOF)
        // must read back as zeros.
        if s + 1 < segments {
            let hole = off + seg as u64;
            let mut got = 0usize;
            while got < seg {
                let n = fs.read(file, hole + got as u64, &mut rbuf[got..]).unwrap();
                assert!(n > 0, "short read in hole at {}", hole + got as u64);
                got += n;
            }
            assert!(rbuf.iter().all(|&b| b == 0), "hole not zero at {}", hole);
        }
    }
    assert_eq!(dev.oob_writes.load(Ordering::Relaxed), 0);
    assert_eq!(dev.oob_reads.load(Ordering::Relaxed), 0);
    drop(fs);
    check(&path);
    fs::remove_file(&path).ok();
}

/// REPRO ATTEMPT: real-hardware geometry. A 238 GiB device (sparse file) like
/// the actual rootfs, writing the ~130 MiB libLLVM.so. The first data chunk is
/// only ~28 MiB (broken by the 64 MiB superblock mirror), so the write crosses
/// into the second — *huge* — data chunk at ~21%, exactly where apk fails.
#[test]
fn repro_libllvm_real_geometry() {
    run_huge_file(
        "realgeom",
        238 * 1024 * 1024 * 1024,
        130 * 1024 * 1024,
        None,
    );
}
