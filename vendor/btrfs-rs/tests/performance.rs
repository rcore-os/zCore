#![cfg(feature = "std")]
//! Performance & stress harness for the btrfs driver.
//!
//! These tests exercise the driver across a range of realistic workloads
//! (big files, small files, many files per directory, deep trees, random
//! I/O, metadata churn, ...) and print a throughput / device-I/O report for
//! each.  Every scenario also verifies data integrity, so a regression in
//! correctness shows up as a failed assertion rather than a silent bad
//! number.  When `btrfs-progs` is installed the resulting image is validated
//! with `btrfs check` at the end of the heavier scenarios.
//!
//! Run with:
//!
//! ```text
//! cargo test --features std --test performance -- --nocapture --test-threads=1
//! ```
//!
//! Set `BTRFS_BENCH_SCALE` (default 1) to scale the heavy workloads up or
//! down, e.g. `BTRFS_BENCH_SCALE=4` for a longer run.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use std::{env, fs};

use btrfs::device::{BlockDevice, FileDevice};
use btrfs::{mkfs, Btrfs, FileKind, Result};

// ----------------------------------------------------------------------------
// Instrumented block device: wraps a real device and counts every access so we
// can report write amplification and reads-per-operation.
// ----------------------------------------------------------------------------

#[derive(Default)]
struct Counters {
    reads: AtomicU64,
    read_bytes: AtomicU64,
    writes: AtomicU64,
    write_bytes: AtomicU64,
    syncs: AtomicU64,
}

impl Counters {
    fn snapshot(&self) -> CountersSnap {
        CountersSnap {
            reads: self.reads.load(Ordering::Relaxed),
            read_bytes: self.read_bytes.load(Ordering::Relaxed),
            writes: self.writes.load(Ordering::Relaxed),
            write_bytes: self.write_bytes.load(Ordering::Relaxed),
            syncs: self.syncs.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy)]
struct CountersSnap {
    reads: u64,
    read_bytes: u64,
    writes: u64,
    write_bytes: u64,
    syncs: u64,
}

impl std::ops::Sub for CountersSnap {
    type Output = CountersSnap;
    fn sub(self, rhs: CountersSnap) -> CountersSnap {
        CountersSnap {
            reads: self.reads - rhs.reads,
            read_bytes: self.read_bytes - rhs.read_bytes,
            writes: self.writes - rhs.writes,
            write_bytes: self.write_bytes - rhs.write_bytes,
            syncs: self.syncs - rhs.syncs,
        }
    }
}

struct CountingDevice {
    inner: Arc<dyn BlockDevice>,
    counters: Arc<Counters>,
}

impl BlockDevice for CountingDevice {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.counters.reads.fetch_add(1, Ordering::Relaxed);
        self.counters
            .read_bytes
            .fetch_add(buf.len() as u64, Ordering::Relaxed);
        self.inner.read_at(offset, buf)
    }
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<()> {
        self.counters.writes.fetch_add(1, Ordering::Relaxed);
        self.counters
            .write_bytes
            .fetch_add(buf.len() as u64, Ordering::Relaxed);
        self.inner.write_at(offset, buf)
    }
    fn sync(&self) -> Result<()> {
        self.counters.syncs.fetch_add(1, Ordering::Relaxed);
        self.inner.sync()
    }
    fn size(&self) -> u64 {
        self.inner.size()
    }
}

// ----------------------------------------------------------------------------
// Test scaffolding.
// ----------------------------------------------------------------------------

fn scale() -> u64 {
    env::var("BTRFS_BENCH_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&v| v > 0)
        .unwrap_or(1)
}

fn have_progs() -> bool {
    std::process::Command::new("btrfs")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn tmpfile(name: &str, size: u64) -> PathBuf {
    let path = env::temp_dir().join(format!("btrfs-bench-{}-{}", std::process::id(), name));
    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    f.set_len(size).unwrap();
    path
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

/// Build a freshly-formatted, instrumented filesystem of `size` bytes.
fn fresh(name: &str, size: u64) -> (Btrfs, Arc<Counters>, PathBuf) {
    let path = tmpfile(name, size);
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let backing: Arc<dyn BlockDevice> = Arc::new(FileDevice::open(file).unwrap());
    mkfs::format(&*backing, &opts()).unwrap();
    let counters = Arc::new(Counters::default());
    let dev: Arc<dyn BlockDevice> = Arc::new(CountingDevice {
        inner: backing,
        counters: counters.clone(),
    });
    let fs = Btrfs::mount(dev, false).unwrap();
    (fs, counters, path)
}

fn btrfs_check(path: &Path) {
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
        "btrfs check failed for {:?}\nstdout:\n{}\nstderr:\n{}",
        path,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn mib_s(bytes: u64, secs: f64) -> f64 {
    if secs <= 0.0 {
        return f64::INFINITY;
    }
    (bytes as f64 / (1024.0 * 1024.0)) / secs
}

/// Deterministic pseudo-random byte generator (LCG); reproducible payloads.
struct Lcg(u64);
impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
    fn fill(&mut self, buf: &mut [u8]) {
        for b in buf.iter_mut() {
            *b = (self.next_u64() >> 33) as u8;
        }
    }
}

fn report_io(label: &str, c: CountersSnap) {
    println!(
        "    {:<22} dev reads={:>7} ({:>8.2} MiB)  writes={:>7} ({:>8.2} MiB)  syncs={}",
        label,
        c.reads,
        c.read_bytes as f64 / (1024.0 * 1024.0),
        c.writes,
        c.write_bytes as f64 / (1024.0 * 1024.0),
        c.syncs,
    );
}

// ----------------------------------------------------------------------------
// Scenario 1: one large file, sequential write then sequential read.
// ----------------------------------------------------------------------------

#[test]
fn perf_large_file_sequential() {
    let mib = 64 * scale();
    let total = mib * 1024 * 1024;
    let img = (total * 3).max(256 * 1024 * 1024);
    println!("\n[large-file-sequential] file = {} MiB", mib);
    let (mut fs, counters, path) = fresh("seq", img);
    let root = fs.root_ino();
    let file = fs
        .create(root, "big.bin", FileKind::Regular, 0o644, 0)
        .unwrap();

    let chunk = 128 * 1024usize;
    let mut rng = Lcg::new(0xfeed);
    let mut payload = vec![0u8; chunk];

    // -- write --
    let base = counters.snapshot();
    let t = Instant::now();
    let mut off = 0u64;
    while off < total {
        rng.fill(&mut payload);
        let n = fs.write(file, off, &payload).unwrap();
        assert_eq!(n, payload.len(), "short write at {}", off);
        off += n as u64;
    }
    fs.sync().unwrap();
    let wsecs = t.elapsed().as_secs_f64();
    println!(
        "    write  {:>8.2} MiB/s  ({:.3}s)",
        mib_s(total, wsecs),
        wsecs
    );
    report_io("write", counters.snapshot() - base);

    // Regenerate the full expected payload up front so verification does not
    // pollute the timed read region.
    let mut expected = vec![0u8; total as usize];
    let mut expect = Lcg::new(0xfeed);
    for blk in expected.chunks_mut(chunk) {
        expect.fill(blk);
    }

    // -- sequential read (small chunks: stresses extent-list scan) --
    let rchunk = 64 * 1024usize;
    let mut buf = vec![0u8; rchunk];
    let base = counters.snapshot();
    let t = Instant::now();
    let mut off = 0u64;
    while off < total {
        let want = (rchunk as u64).min(total - off) as usize;
        let n = fs.read(file, off, &mut buf[..want]).unwrap();
        assert!(n > 0, "zero read at {}", off);
        off += n as u64;
    }
    let rsecs = t.elapsed().as_secs_f64();
    // Verify after timing (cheap slice compare in a second read pass).
    let mut off = 0u64;
    while off < total {
        let want = (rchunk as u64).min(total - off) as usize;
        let n = fs.read(file, off, &mut buf[..want]).unwrap();
        assert_eq!(
            &buf[..n],
            &expected[off as usize..off as usize + n],
            "data mismatch at {}",
            off
        );
        off += n as u64;
    }
    println!(
        "    read   {:>8.2} MiB/s  ({:.3}s)",
        mib_s(total, rsecs),
        rsecs
    );
    report_io("read", counters.snapshot() - base);

    drop(fs);
    btrfs_check(&path);
    fs::remove_file(&path).ok();
}

// ----------------------------------------------------------------------------
// Scenario 1b: heavily fragmented file, sequential small-chunk read.
//
// Writing in scattered order forces many small extents that do not merge.
// Reading the whole file back in small chunks is the classic O(n^2) trap if
// the reader rescans the full extent list on every call.
// ----------------------------------------------------------------------------

#[test]
fn perf_fragmented_read() {
    let nblocks = 8000 * scale();
    let blk = 4096usize;
    let total = nblocks * blk as u64;
    println!(
        "\n[fragmented-read] {} scattered 4K extents ({} MiB), sequential read",
        nblocks,
        total / (1024 * 1024)
    );
    let (mut fs, counters, path) = fresh("frag", (total * 4).max(256 * 1024 * 1024));
    let root = fs.root_ino();
    let file = fs
        .create(root, "frag.bin", FileKind::Regular, 0o644, 0)
        .unwrap();

    // Write every 4K block, but in a scattered permutation so adjacent extents
    // are created out of order (defeats automatic extent coalescing).
    let mut order: Vec<u64> = (0..nblocks).collect();
    let mut rng = Lcg::new(0x7777);
    for i in (1..order.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    let mut buf = vec![0u8; blk];
    for &b in &order {
        // tag each block with its index so we can verify on read-back
        let tag = (b & 0xff) as u8;
        buf.iter_mut().for_each(|x| *x = tag);
        fs.write(file, b * blk as u64, &buf).unwrap();
    }
    fs.sync().unwrap();

    // Sequential read in small chunks.
    let base = counters.snapshot();
    let t = Instant::now();
    let mut off = 0u64;
    while off < total {
        let n = fs.read(file, off, &mut buf).unwrap();
        assert!(n > 0);
        let expect = ((off / blk as u64) & 0xff) as u8;
        assert!(
            buf[..n].iter().all(|&x| x == expect),
            "frag data mismatch at {}",
            off
        );
        off += n as u64;
    }
    let secs = t.elapsed().as_secs_f64();
    println!(
        "    read {:.2} MiB/s  ({:.3}s for {} chunks)",
        mib_s(total, secs),
        secs,
        nblocks
    );
    report_io("frag-read", counters.snapshot() - base);

    drop(fs);
    btrfs_check(&path);
    fs::remove_file(&path).ok();
}

// ----------------------------------------------------------------------------
// Scenario 2: large file, random 4 KiB read/write.
// ----------------------------------------------------------------------------

#[test]
fn perf_large_file_random() {
    let mib = 32 * scale();
    let total = mib * 1024 * 1024;
    let img = (total * 4).max(256 * 1024 * 1024);
    let blk = 4096usize;
    let nblocks = total / blk as u64;
    let nops = 4000 * scale();
    println!(
        "\n[large-file-random] file = {} MiB, {} random 4K ops",
        mib, nops
    );
    let (mut fs, counters, path) = fresh("rand", img);
    let root = fs.root_ino();
    let file = fs
        .create(root, "rand.bin", FileKind::Regular, 0o644, 0)
        .unwrap();

    // Preallocate by a single tail write so the file has full size.
    let zero = vec![0u8; blk];
    fs.write(file, total - blk as u64, &zero).unwrap();

    // Shadow model of what each block should contain (last byte tag).
    let mut model = vec![0u8; nblocks as usize];
    let mut rng = Lcg::new(0xabcd);
    let mut buf = vec![0u8; blk];

    let base = counters.snapshot();
    let t = Instant::now();
    for _ in 0..nops {
        let blkno = (rng.next_u64() % nblocks) as usize;
        let off = blkno as u64 * blk as u64;
        if rng.next_u64() & 1 == 0 {
            // write: tag every byte with a generation marker
            let tag = (rng.next_u64() & 0xff) as u8;
            buf.iter_mut().for_each(|b| *b = tag);
            let n = fs.write(file, off, &buf).unwrap();
            assert_eq!(n, blk);
            model[blkno] = tag;
        } else {
            let n = fs.read(file, off, &mut buf).unwrap();
            assert_eq!(n, blk);
            assert!(
                buf.iter().all(|&b| b == model[blkno]),
                "random block corrupt"
            );
        }
    }
    fs.sync().unwrap();
    let secs = t.elapsed().as_secs_f64();
    println!(
        "    {:.0} ops/s  ({:.3}s, {:.2} MiB/s effective)",
        nops as f64 / secs,
        secs,
        mib_s(nops * blk as u64, secs)
    );
    report_io("random-rw", counters.snapshot() - base);

    drop(fs);
    btrfs_check(&path);
    fs::remove_file(&path).ok();
}

// ----------------------------------------------------------------------------
// Scenario 3: many small files in a single directory.
// ----------------------------------------------------------------------------

#[test]
fn perf_many_files_one_dir() {
    let n = 8000 * scale();
    println!("\n[many-files-one-dir] {} files in one directory", n);
    let (mut fs, counters, path) = fresh("manyfiles", 512 * 1024 * 1024);
    let root = fs.root_ino();
    let dir = fs.create(root, "d", FileKind::Dir, 0o755, 0).unwrap();
    let body = b"small file contents\n";

    // -- create --
    let base = counters.snapshot();
    let t = Instant::now();
    for i in 0..n {
        let f = fs
            .create(dir, &format!("file-{:06}", i), FileKind::Regular, 0o644, 0)
            .unwrap();
        fs.write(f, 0, body).unwrap();
    }
    fs.sync().unwrap();
    let secs = t.elapsed().as_secs_f64();
    println!("    create {:.0} files/s  ({:.3}s)", n as f64 / secs, secs);
    report_io("create", counters.snapshot() - base);

    // -- random lookup (scaling: should be ~O(log n), not O(n)) --
    let mut rng = Lcg::new(0x1357);
    let lookups = 5000 * scale();
    let base = counters.snapshot();
    let t = Instant::now();
    for _ in 0..lookups {
        let i = rng.next_u64() % n;
        let ino = fs.lookup(dir, &format!("file-{:06}", i)).unwrap();
        assert!(ino != 0);
    }
    let secs = t.elapsed().as_secs_f64();
    println!(
        "    lookup {:.0} ops/s  ({:.3}s, {:.1} dev-reads/lookup)",
        lookups as f64 / secs,
        secs,
        (counters.snapshot() - base).reads as f64 / lookups as f64
    );

    // -- readdir --
    let base = counters.snapshot();
    let t = Instant::now();
    let entries = fs.readdir(dir).unwrap();
    let secs = t.elapsed().as_secs_f64();
    assert_eq!(entries.len() as u64, n, "readdir lost entries");
    println!("    readdir {} entries in {:.3}s", entries.len(), secs);
    report_io("readdir", counters.snapshot() - base);

    // -- unlink all --
    let base = counters.snapshot();
    let t = Instant::now();
    for i in 0..n {
        fs.unlink(dir, &format!("file-{:06}", i)).unwrap();
    }
    fs.sync().unwrap();
    let secs = t.elapsed().as_secs_f64();
    assert_eq!(fs.readdir(dir).unwrap().len(), 0);
    println!("    unlink {:.0} files/s  ({:.3}s)", n as f64 / secs, secs);
    report_io("unlink", counters.snapshot() - base);

    drop(fs);
    btrfs_check(&path);
    fs::remove_file(&path).ok();
}

// ----------------------------------------------------------------------------
// Scenario 4: lookup-cost scaling as a directory grows.
// ----------------------------------------------------------------------------

#[test]
fn perf_lookup_scaling() {
    println!("\n[lookup-scaling] avg dev-reads & time per lookup vs directory size");
    let (mut fs, counters, path) = fresh("lookupscale", 512 * 1024 * 1024);
    let root = fs.root_ino();
    let dir = fs.create(root, "d", FileKind::Dir, 0o755, 0).unwrap();

    let step = 2000u64;
    let rounds = 8u64 * scale().min(2);
    let mut created = 0u64;
    let mut rng = Lcg::new(0x2468);
    println!(
        "    {:>8}  {:>14}  {:>16}",
        "entries", "us/lookup", "dev-reads/lookup"
    );
    for _ in 0..rounds {
        for i in created..created + step {
            fs.create(dir, &format!("e{:07}", i), FileKind::Regular, 0o644, 0)
                .unwrap();
        }
        created += step;
        let probes = 2000u64;
        let base = counters.snapshot();
        let t = Instant::now();
        for _ in 0..probes {
            let i = rng.next_u64() % created;
            fs.lookup(dir, &format!("e{:07}", i)).unwrap();
        }
        let us = t.elapsed().as_secs_f64() * 1e6 / probes as f64;
        let rd = (counters.snapshot() - base).reads as f64 / probes as f64;
        println!("    {:>8}  {:>14.2}  {:>16.2}", created, us, rd);
    }
    drop(fs);
    fs::remove_file(&path).ok();
}

// ----------------------------------------------------------------------------
// Scenario 4b: free-space fragmentation churn.
//
// Allocating and freeing data extents in a scattered pattern builds up a large
// number of free fragments.  Every mutation re-checks free space (meta/data),
// so if those checks scan the whole free map the per-op cost grows linearly
// with fragmentation (an O(n^2) batch).  This scenario prints us/op as the
// fragment count grows; it must stay roughly flat.
// ----------------------------------------------------------------------------

#[test]
fn perf_fragmentation_scaling() {
    println!("\n[fragmentation-scaling] alloc throughput vs free-space fragmentation");
    let (mut fs, counters, path) = fresh("fragchurn", 1024 * 1024 * 1024);
    let root = fs.root_ino();
    let dir = fs.create(root, "d", FileKind::Dir, 0o755, 0).unwrap();
    let payload = vec![0xa5u8; 8 * 1024]; // 8 KiB -> real data extent, not inline

    let rounds = 8u64 * scale().min(2);
    let per_round = 1500u64;
    let mut counter = 0u64;
    let mut keep: Vec<(String, u64)> = Vec::new();
    println!(
        "    {:>10}  {:>12}  {:>16}",
        "live-files", "us/op", "dev-writes/op"
    );
    for _ in 0..rounds {
        let base = counters.snapshot();
        let t = Instant::now();
        for _ in 0..per_round {
            // create two, delete one => net growth + a freed hole each step.
            for _ in 0..2 {
                let name = format!("f{:07}", counter);
                counter += 1;
                let f = fs.create(dir, &name, FileKind::Regular, 0o644, 0).unwrap();
                fs.write(f, 0, &payload).unwrap();
                keep.push((name, f));
            }
            // free an old one near the front to scatter holes
            if keep.len() > 4 {
                let (name, _) = keep.remove(keep.len() / 3);
                fs.unlink(dir, &name).unwrap();
            }
        }
        fs.sync().unwrap();
        let us = t.elapsed().as_secs_f64() * 1e6 / (per_round * 2) as f64;
        let w = (counters.snapshot() - base).writes as f64 / (per_round * 2) as f64;
        println!("    {:>10}  {:>12.2}  {:>16.2}", keep.len(), us, w);
    }
    drop(fs);
    btrfs_check(&path);
    fs::remove_file(&path).ok();
}

// ----------------------------------------------------------------------------
// Scenario 4c: RangeMap query scaling (direct regression guard).
//
// `meta_free`/`data_free`/`largest_in` query free space inside a *single*
// block group, but the underlying map covers the whole device.  A query window
// at the top of the address space must not get slower as unrelated fragments
// pile up elsewhere.  Pre-fix these scanned the whole map up to `hi`, so cost
// grew linearly with total fragment count; this guard asserts it stays bounded.
// ----------------------------------------------------------------------------

#[test]
fn perf_rangemap_query_scaling() {
    use btrfs::alloc_ext::RangeMap;
    println!("\n[rangemap-query-scaling] per-query time at top window vs fragment count");
    println!("    {:>12}  {:>14}", "fragments", "ns/query");
    let mut last = 0f64;
    let mut first = 0f64;
    for (idx, &n) in [2_000u64, 20_000, 100_000].iter().enumerate() {
        // Scattered fragments: a free range every 8 KiB across the low region.
        let mut rm = RangeMap::default();
        for i in 0..n {
            rm.insert(i * 8192, 4096);
        }
        // Narrow query window sitting *above* every fragment (like a metadata
        // block group placed after a heavily-fragmented data region).
        let lo = n * 8192 + (1 << 20);
        let hi = lo + 64 * 1024 * 1024;
        rm.insert(lo, 32 * 1024 * 1024);

        let probes = 2000u64;
        let t = Instant::now();
        let mut acc = 0u64;
        for _ in 0..probes {
            acc = acc.wrapping_add(rm.total_free_in(lo, hi));
            acc = acc.wrapping_add(rm.largest_in(lo, hi).map_or(0, |(_, l)| l));
        }
        std::hint::black_box(acc);
        let ns = t.elapsed().as_secs_f64() * 1e9 / (probes * 2) as f64;
        println!("    {:>12}  {:>14.1}", n, ns);
        if idx == 0 {
            first = ns;
        }
        last = ns;
    }
    // With the bounded-range fix the top-window query is O(log n): a 50x growth
    // in fragments must not blow up the per-query cost.  Allow generous slack
    // for noise but catch a return to linear scanning.
    assert!(
        last < first * 8.0 + 200.0,
        "RangeMap query cost scales with unrelated fragments (O(n) regression): \
         {:.1}ns -> {:.1}ns",
        first,
        last
    );
}

// ----------------------------------------------------------------------------
// Scenario 5: deep directory tree.
// ----------------------------------------------------------------------------

#[test]
fn perf_deep_tree() {
    let depth = 200 * scale();
    println!(
        "\n[deep-tree] nest {} directories deep, file at each level",
        depth
    );
    let (mut fs, counters, path) = fresh("deeptree", 256 * 1024 * 1024);
    let mut cur = fs.root_ino();

    let base = counters.snapshot();
    let t = Instant::now();
    for d in 0..depth {
        let sub = fs.create(cur, "sub", FileKind::Dir, 0o755, 0).unwrap();
        let f = fs
            .create(cur, &format!("f{}", d), FileKind::Regular, 0o644, 0)
            .unwrap();
        fs.write(f, 0, format!("level {}\n", d).as_bytes()).unwrap();
        cur = sub;
    }
    fs.sync().unwrap();
    let secs = t.elapsed().as_secs_f64();
    println!(
        "    build  {:.0} dirs/s  ({:.3}s)",
        depth as f64 / secs,
        secs
    );
    report_io("build", counters.snapshot() - base);

    // Walk back down from the root verifying every level resolves.
    let base = counters.snapshot();
    let t = Instant::now();
    let mut cur = fs.root_ino();
    for d in 0..depth {
        let f = fs.lookup(cur, &format!("f{}", d)).unwrap();
        let st = fs.stat(f).unwrap();
        assert_eq!(st.kind, FileKind::Regular);
        cur = fs.lookup(cur, "sub").unwrap();
    }
    let secs = t.elapsed().as_secs_f64();
    println!(
        "    walk   {:.0} levels/s  ({:.3}s)",
        depth as f64 / secs,
        secs
    );
    report_io("walk", counters.snapshot() - base);

    drop(fs);
    btrfs_check(&path);
    fs::remove_file(&path).ok();
}

// ----------------------------------------------------------------------------
// Scenario 6: many medium files (throughput of small/medium writes).
// ----------------------------------------------------------------------------

#[test]
fn perf_small_files_throughput() {
    let n = 3000 * scale();
    let sz = 8 * 1024usize; // 8 KiB each -> spans inline boundary into extents
    println!("\n[small-files-throughput] {} files x {} KiB", n, sz / 1024);
    let (mut fs, counters, path) = fresh("smallthru", 512 * 1024 * 1024);
    let root = fs.root_ino();
    let dir = fs.create(root, "d", FileKind::Dir, 0o755, 0).unwrap();
    let mut rng = Lcg::new(0x99);
    let mut payload = vec![0u8; sz];

    let base = counters.snapshot();
    let t = Instant::now();
    for i in 0..n {
        rng.fill(&mut payload);
        let f = fs
            .create(dir, &format!("f{:06}", i), FileKind::Regular, 0o644, 0)
            .unwrap();
        let mut off = 0u64;
        while (off as usize) < sz {
            let n = fs.write(f, off, &payload[off as usize..]).unwrap();
            off += n as u64;
        }
    }
    fs.sync().unwrap();
    let secs = t.elapsed().as_secs_f64();
    let total = n * sz as u64;
    println!(
        "    {:.0} files/s, {:.2} MiB/s  ({:.3}s)",
        n as f64 / secs,
        mib_s(total, secs),
        secs
    );
    report_io("write", counters.snapshot() - base);

    // Read a sample back and verify the last-written payload round-trips.
    let mut buf = vec![0u8; sz];
    let f = fs.lookup(dir, &format!("f{:06}", n - 1)).unwrap();
    let mut got = 0usize;
    while got < sz {
        got += fs.read(f, got as u64, &mut buf[got..]).unwrap();
    }
    assert_eq!(buf, payload, "last file payload mismatch");

    drop(fs);
    btrfs_check(&path);
    fs::remove_file(&path).ok();
}

// ----------------------------------------------------------------------------
// Scenario 7: truncate / extend churn on a single file.
// ----------------------------------------------------------------------------

#[test]
fn perf_truncate_churn() {
    let iters = 2000 * scale();
    println!("\n[truncate-churn] {} grow/shrink cycles", iters);
    let (mut fs, counters, path) = fresh("trunc", 256 * 1024 * 1024);
    let root = fs.root_ino();
    let file = fs
        .create(root, "t.bin", FileKind::Regular, 0o644, 0)
        .unwrap();
    let seed = vec![0x5au8; 256 * 1024];
    fs.write(file, 0, &seed).unwrap();

    let mut rng = Lcg::new(0x424242);
    let base = counters.snapshot();
    let t = Instant::now();
    for _ in 0..iters {
        let sz = rng.next_u64() % (256 * 1024);
        fs.truncate(file, sz).unwrap();
        let st = fs.stat(file).unwrap();
        assert_eq!(st.size, sz);
    }
    fs.sync().unwrap();
    let secs = t.elapsed().as_secs_f64();
    println!("    {:.0} truncate/s  ({:.3}s)", iters as f64 / secs, secs);
    report_io("truncate", counters.snapshot() - base);

    // Extend with a hole then verify it reads back as zeros.
    fs.truncate(file, 1024 * 1024).unwrap();
    let mut buf = vec![0xffu8; 4096];
    fs.read(file, 900 * 1024, &mut buf).unwrap();
    assert!(
        buf.iter().all(|&b| b == 0),
        "hole did not read back as zero"
    );

    drop(fs);
    btrfs_check(&path);
    fs::remove_file(&path).ok();
}
