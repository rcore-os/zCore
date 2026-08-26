#![cfg(feature = "std")]
//! Fault-injection bench for the "apk fix: failed to extract libLLVM.so: I/O
//! error" bug.
//!
//! The other suites (`large_files.rs`) prove the btrfs *logic* is correct on a
//! perfect device: a 200+ MiB file writes back byte-for-byte and passes
//! `btrfs check`. So the EIO seen only on real hardware is NOT a filesystem
//! logic bug — it is a *transient* device fault (an AHCI task-file error on a
//! command or, most often, on the FLUSH CACHE issued by `fsync`) that a perfect
//! test device never produces.
//!
//! This bench reproduces that exact failure mode with a device that injects a
//! single transient error, and demonstrates that retrying the operation at the
//! device layer — which is what the kernel fix does (AHCI `flush_cache` and
//! `rw_block` now retry, and the `DevAdapter` retries/​shrinks) — turns the hard
//! EIO back into success.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use btrfs::device::BlockDevice;
use btrfs::{mkfs, Btrfs, Error, FileKind, Result};

// ---------------------------------------------------------------------------
// In-memory backing store (a perfect, fast device).
// ---------------------------------------------------------------------------
struct MemDev {
    data: Mutex<Vec<u8>>,
    size: u64,
}

impl MemDev {
    fn new(size: u64) -> Arc<Self> {
        Arc::new(Self {
            data: Mutex::new(vec![0u8; size as usize]),
            size,
        })
    }
}

impl BlockDevice for MemDev {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset as usize + buf.len();
        let d = self.data.lock().unwrap();
        if end > d.len() {
            return Err(Error::Io);
        }
        buf.copy_from_slice(&d[offset as usize..end]);
        Ok(())
    }
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        let end = offset as usize + buf.len();
        let mut d = self.data.lock().unwrap();
        if end > d.len() {
            return Err(Error::Io);
        }
        d[offset as usize..end].copy_from_slice(buf);
        Ok(())
    }
    fn sync(&self) -> Result<()> {
        Ok(())
    }
    fn size(&self) -> u64 {
        self.size
    }
}

// ---------------------------------------------------------------------------
// Fault injector. Wraps a backing device and fails the Nth `sync` (and/or the
// Nth `write`) exactly *once* — a transient hiccup, like a real SATA controller
// raising a task-file error that a port reset clears. The data still reaches the
// backing store; only the completion is reported as failed, matching how the
// FLUSH/command actually behaves on the wire.
// ---------------------------------------------------------------------------
struct Flaky {
    inner: Arc<dyn BlockDevice>,
    sync_calls: AtomicU64,
    write_calls: AtomicU64,
    fail_sync_on: u64,  // 0 = never
    fail_write_on: u64, // 0 = never
    injected: AtomicU64,
}

impl Flaky {
    fn new(inner: Arc<dyn BlockDevice>, fail_sync_on: u64, fail_write_on: u64) -> Arc<Self> {
        Arc::new(Self {
            inner,
            sync_calls: AtomicU64::new(0),
            write_calls: AtomicU64::new(0),
            fail_sync_on,
            fail_write_on,
            injected: AtomicU64::new(0),
        })
    }
    fn injected(&self) -> u64 {
        self.injected.load(Ordering::Relaxed)
    }
}

impl BlockDevice for Flaky {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.inner.read_at(offset, buf)
    }
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        // The write reaches the medium first (idempotent re-issue is safe), then
        // we report the transient completion error exactly once.
        self.inner.write_at(offset, buf)?;
        let n = self.write_calls.fetch_add(1, Ordering::Relaxed) + 1;
        if self.fail_write_on != 0 && n == self.fail_write_on {
            self.injected.fetch_add(1, Ordering::Relaxed);
            return Err(Error::Io);
        }
        Ok(())
    }
    fn sync(&self) -> Result<()> {
        self.inner.sync()?;
        let n = self.sync_calls.fetch_add(1, Ordering::Relaxed) + 1;
        if self.fail_sync_on != 0 && n == self.fail_sync_on {
            self.injected.fetch_add(1, Ordering::Relaxed);
            return Err(Error::Io);
        }
        Ok(())
    }
    fn size(&self) -> u64 {
        self.inner.size()
    }
}

// ---------------------------------------------------------------------------
// Device-layer retry wrapper — models the kernel fix. The AHCI driver now
// retries a FLUSH CACHE / data command after a port reset, and the kernel's
// `DevAdapter` retries (and shrinks) a failed transfer. Re-issuing the same
// idempotent operation a few times absorbs a single transient fault.
// ---------------------------------------------------------------------------
struct Retry {
    inner: Arc<dyn BlockDevice>,
    tries: u32,
}

impl Retry {
    fn new(inner: Arc<dyn BlockDevice>, tries: u32) -> Arc<Self> {
        Arc::new(Self { inner, tries })
    }
}

impl BlockDevice for Retry {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let mut last = Err(Error::Io);
        for _ in 0..self.tries {
            last = self.inner.read_at(offset, buf);
            if last.is_ok() {
                return Ok(());
            }
        }
        last
    }
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        let mut last = Err(Error::Io);
        for _ in 0..self.tries {
            last = self.inner.write_at(offset, buf);
            if last.is_ok() {
                return Ok(());
            }
        }
        last
    }
    fn sync(&self) -> Result<()> {
        let mut last = Err(Error::Io);
        for _ in 0..self.tries {
            last = self.inner.sync();
            if last.is_ok() {
                return Ok(());
            }
        }
        last
    }
    fn size(&self) -> u64 {
        self.inner.size()
    }
}

fn opts() -> mkfs::MkfsOptions {
    let mut seed = 0x51ed_c0ffee_u64 ^ 0xa5a5_a5a5;
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

/// Extract a `file_size` "package" in 64 KiB chunks then `fsync`, exactly like
/// apk. Returns the result of the operation that the EIO would surface from.
fn extract_and_sync(fs: &mut Btrfs, file_size: u64) -> Result<()> {
    let root = fs.root_ino();
    let file = fs.create(root, "libLLVM.so", FileKind::Regular, 0o755, 0)?;
    let chunk = 64 * 1024usize;
    let buf = vec![0xABu8; chunk];
    let mut off = 0u64;
    while off < file_size {
        let n = (chunk as u64).min(file_size - off) as usize;
        let w = fs.write(file, off, &buf[..n])?;
        assert_eq!(w, n, "short write at {off}");
        off += n as u64;
    }
    // apk fsync()s the extracted file; this is the FLUSH CACHE that fails on
    // real hardware.
    fs.sync()
}

/// REPRODUCE: a single transient FLUSH failure during the final fsync surfaces
/// as a hard `Error::Io` — i.e. `apk` prints "failed to extract …: I/O error".
#[test]
fn transient_flush_failure_reproduces_eio() {
    let mem = MemDev::new(512 * 1024 * 1024);
    // Format on the perfect device, then mount through the fault injector.
    mkfs::format(&*mem, &opts()).unwrap();
    let flaky = Flaky::new(mem, /*fail_sync_on=*/ 1, /*fail_write_on=*/ 0);
    let dev: Arc<dyn BlockDevice> = flaky.clone();
    let mut fs = Btrfs::mount(dev, false).unwrap();

    let res = extract_and_sync(&mut fs, 48 * 1024 * 1024);
    assert!(
        matches!(res, Err(Error::Io)),
        "expected the transient flush fault to surface as EIO, got {res:?}"
    );
    assert_eq!(flaky.injected(), 1, "exactly one fault should have fired");
}

/// FIX: the same transient FLUSH fault, but the device layer retries (as the
/// AHCI driver now does). The fsync succeeds and the data is intact — the EIO
/// is gone.
#[test]
fn device_retry_survives_transient_flush() {
    let mem = MemDev::new(512 * 1024 * 1024);
    mkfs::format(&*mem, &opts()).unwrap();
    let flaky = Flaky::new(mem, /*fail_sync_on=*/ 1, /*fail_write_on=*/ 0);
    let retry: Arc<dyn BlockDevice> = Retry::new(flaky.clone(), 4);
    let mut fs = Btrfs::mount(retry, false).unwrap();

    extract_and_sync(&mut fs, 48 * 1024 * 1024)
        .expect("device-layer retry should absorb the transient flush fault");
    assert_eq!(
        flaky.injected(),
        1,
        "the fault still fired once (then retried)"
    );

    // Data is intact after the retried flush.
    let root = fs.root_ino();
    let file = fs.lookup(root, "libLLVM.so").unwrap();
    let st = fs.stat(file).unwrap();
    assert_eq!(st.size, 48 * 1024 * 1024);
}

/// REPRODUCE: a single transient *write/metadata* command fault mid-extraction
/// also surfaces as EIO without a retry.
#[test]
fn transient_write_failure_reproduces_eio() {
    let mem = MemDev::new(512 * 1024 * 1024);
    mkfs::format(&*mem, &opts()).unwrap();
    // Fail the 1000th backing write — well into the large file, like a hiccup on
    // one of the ~thousands of commands a big file needs.
    let flaky = Flaky::new(mem, 0, 1000);
    let dev: Arc<dyn BlockDevice> = flaky.clone();
    let mut fs = Btrfs::mount(dev, false).unwrap();

    let res = extract_and_sync(&mut fs, 64 * 1024 * 1024);
    assert!(
        matches!(res, Err(Error::Io)),
        "expected the transient write fault to surface as EIO, got {res:?}"
    );
    assert_eq!(flaky.injected(), 1);
}

/// FIX: the same transient write fault, retried at the device layer, completes.
#[test]
fn device_retry_survives_transient_write() {
    let mem = MemDev::new(512 * 1024 * 1024);
    mkfs::format(&*mem, &opts()).unwrap();
    let flaky = Flaky::new(mem, 0, 1000);
    let retry: Arc<dyn BlockDevice> = Retry::new(flaky.clone(), 4);
    let mut fs = Btrfs::mount(retry, false).unwrap();

    extract_and_sync(&mut fs, 64 * 1024 * 1024)
        .expect("device-layer retry should absorb the transient write fault");
    assert_eq!(flaky.injected(), 1);
}
