#![cfg(feature = "std")]
//! Cross-validation against btrfs-progs (`btrfs check`, `mkfs.btrfs`,
//! `btrfs restore`). Tests are skipped when btrfs-progs is unavailable.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use btrfs::device::{BlockDevice, FileDevice};
use btrfs::{mkfs, Btrfs, FileKind};

fn have_progs() -> bool {
    Command::new("btrfs")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn tmpfile(name: &str, size: u64) -> PathBuf {
    let path = std::env::temp_dir().join(format!("btrfs-rs-test-{}-{}", std::process::id(), name));
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

fn open_dev(path: &Path) -> Arc<dyn BlockDevice> {
    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    Arc::new(FileDevice::open(f).unwrap())
}

fn opts() -> mkfs::MkfsOptions {
    // Fixed pseudo-random uuids for reproducibility.
    let mut seed = 0x1234_5678_9abc_def0u64;
    let mut uuid = || {
        let mut u = [0u8; 16];
        for b in u.iter_mut() {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *b = (seed >> 33) as u8;
        }
        // RFC4122-ish version/variant bits.
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

fn check(path: &Path) {
    let out = Command::new("btrfs")
        .args(["check", "--force"])
        .arg(path)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "btrfs check failed for {:?}\nstdout:\n{}\nstderr:\n{}",
        path,
        stdout,
        stderr
    );
}

#[test]
fn mkfs_passes_btrfs_check() {
    if !have_progs() {
        eprintln!("btrfs-progs not available; skipping");
        return;
    }
    let path = tmpfile("mkfs", 64 * 1024 * 1024);
    let dev = open_dev(&path);
    mkfs::format(&*dev, &opts()).unwrap();
    check(&path);

    // And it must be mountable by our own driver.
    let mut fs = Btrfs::mount(dev, false).unwrap();
    let root = fs.root_ino();
    assert_eq!(fs.readdir(root).unwrap().len(), 0);
    let st = fs.stat(root).unwrap();
    assert_eq!(st.kind, FileKind::Dir);
    fs::remove_file(&path).unwrap();
}

#[test]
fn populate_and_check() {
    if !have_progs() {
        eprintln!("btrfs-progs not available; skipping");
        return;
    }
    let path = tmpfile("populate", 64 * 1024 * 1024);
    let dev = open_dev(&path);
    mkfs::format(&*dev, &opts()).unwrap();
    let mut fs = Btrfs::mount(dev, false).unwrap();
    let root = fs.root_ino();

    // Directory tree with files of various sizes, symlinks and hard links.
    let bin = fs.create(root, "bin", FileKind::Dir, 0o755, 0).unwrap();
    let etc = fs.create(root, "etc", FileKind::Dir, 0o755, 0).unwrap();
    let busybox = fs
        .create(bin, "busybox", FileKind::Regular, 0o755, 0)
        .unwrap();
    // ~3 MiB pseudo-random file written in odd-sized chunks.
    let mut payload = Vec::new();
    let mut x = 1u32;
    while payload.len() < 3 * 1024 * 1024 + 123 {
        x = x.wrapping_mul(1664525).wrapping_add(1013904223);
        payload.push((x >> 24) as u8);
    }
    let mut off = 0;
    for chunk in payload.chunks(65521) {
        fs.write(busybox, off, chunk).unwrap();
        off += chunk.len() as u64;
    }
    let fstab = etc_file(&mut fs, etc, "fstab");
    fs.write(fstab, 0, b"/dev/root / btrfs defaults 0 1\n")
        .unwrap();
    fs.symlink(bin, "sh", b"busybox").unwrap();
    fs.link(bin, "ash", busybox).unwrap();
    // Lots of small files to force leaf splits.
    let many = fs.create(root, "many", FileKind::Dir, 0o755, 0).unwrap();
    for i in 0..500 {
        let f = fs
            .create(many, &format!("file-{:04}", i), FileKind::Regular, 0o644, 0)
            .unwrap();
        fs.write(f, 0, format!("contents of file {}\n", i).as_bytes())
            .unwrap();
    }
    // Deletions and renames.
    for i in 0..100 {
        fs.unlink(many, &format!("file-{:04}", i)).unwrap();
    }
    fs.rename(bin, "ash", root, "ash-moved").unwrap();
    fs.truncate(busybox, 1024 * 1024).unwrap();
    fs.truncate(busybox, 2 * 1024 * 1024).unwrap();
    fs.set_attr(busybox, Some(0o700), Some(123), Some(456), None, None)
        .unwrap();
    fs.sync().unwrap();

    // Verify our own view first.
    let got = read_all(&mut fs, busybox);
    assert_eq!(got.len(), 2 * 1024 * 1024);
    assert_eq!(&got[..1024 * 1024], &payload[..1024 * 1024]);
    assert!(got[1024 * 1024..].iter().all(|&b| b == 0));
    let sh = fs.lookup(bin, "sh").unwrap();
    assert_eq!(fs.read_link(sh).unwrap(), b"busybox");
    assert_eq!(fs.readdir(many).unwrap().len(), 400);
    drop(fs);

    check(&path);

    // btrfs restore must reproduce the file contents.
    let restore_dir = std::env::temp_dir().join(format!("btrfs-rs-restore-{}", std::process::id()));
    let _ = fs::remove_dir_all(&restore_dir);
    fs::create_dir_all(&restore_dir).unwrap();
    let out = Command::new("btrfs")
        .args(["restore", "-v"])
        .arg(&path)
        .arg(&restore_dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "btrfs restore failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let restored = fs::read(restore_dir.join("bin/busybox")).unwrap();
    assert_eq!(restored.len(), 2 * 1024 * 1024);
    assert_eq!(&restored[..1024 * 1024], &payload[..1024 * 1024]);
    let fstab = fs::read(restore_dir.join("etc/fstab")).unwrap();
    assert_eq!(fstab, b"/dev/root / btrfs defaults 0 1\n");
    fs::remove_dir_all(&restore_dir).unwrap();
    fs::remove_file(&path).unwrap();
}

fn etc_file(fs: &mut Btrfs, etc: u64, name: &str) -> u64 {
    fs.create(etc, name, FileKind::Regular, 0o644, 0).unwrap()
}

fn read_all(fs: &mut Btrfs, ino: u64) -> Vec<u8> {
    let st = fs.stat(ino).unwrap();
    let mut buf = vec![0u8; st.size as usize];
    let mut off = 0u64;
    while (off as usize) < buf.len() {
        let end = (off as usize + 70000).min(buf.len());
        let n = fs.read(ino, off, &mut buf[off as usize..end]).unwrap();
        assert!(n > 0);
        off += n as u64;
    }
    buf
}

#[test]
fn read_foreign_mkfs_image() {
    if !have_progs() {
        eprintln!("btrfs-progs not available; skipping");
        return;
    }
    // Build a rootdir-populated image with the real mkfs.btrfs and read it
    // back with our driver.
    let src_dir = std::env::temp_dir().join(format!("btrfs-rs-src-{}", std::process::id()));
    let _ = fs::remove_dir_all(&src_dir);
    fs::create_dir_all(src_dir.join("dir/sub")).unwrap();
    let mut big = Vec::new();
    for i in 0..200_000u32 {
        big.extend_from_slice(&i.to_le_bytes());
    }
    fs::write(src_dir.join("dir/big.bin"), &big).unwrap();
    fs::write(src_dir.join("dir/sub/hello.txt"), b"hello eclipse\n").unwrap();
    fs::write(src_dir.join("small.txt"), b"inline me\n").unwrap(); // becomes inline extent
    std::os::unix::fs::symlink("dir/sub/hello.txt", src_dir.join("link")).unwrap();

    let path = tmpfile("foreign", 128 * 1024 * 1024);
    let out = Command::new("mkfs.btrfs")
        .args(["-q", "-O", "^free-space-tree", "--rootdir"])
        .arg(&src_dir)
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "mkfs.btrfs failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let dev = open_dev(&path);
    let mut fs = Btrfs::mount(dev, false).unwrap();
    let root = fs.root_ino();
    let dir = fs.lookup(root, "dir").unwrap();
    let sub = fs.lookup(dir, "sub").unwrap();
    let hello = fs.lookup(sub, "hello.txt").unwrap();
    assert_eq!(read_all(&mut fs, hello), b"hello eclipse\n");
    let bigf = fs.lookup(dir, "big.bin").unwrap();
    assert_eq!(read_all(&mut fs, bigf), big);
    let small = fs.lookup(root, "small.txt").unwrap();
    assert_eq!(read_all(&mut fs, small), b"inline me\n");
    let link = fs.lookup(root, "link").unwrap();
    assert_eq!(fs.read_link(link).unwrap(), b"dir/sub/hello.txt");

    // Now mutate the foreign image (incl. growing a csummed file and writing
    // into an inline file) and ensure btrfs check stays happy.
    fs.write(hello, 0, b"HELLO").unwrap();
    fs.write(small, 10, b"more data beyond inline\n").unwrap();
    let newf = fs
        .create(dir, "added.txt", FileKind::Regular, 0o644, 0)
        .unwrap();
    fs.write(newf, 0, b"added after mkfs\n").unwrap();
    fs.unlink(dir, "big.bin").unwrap();
    fs.sync().unwrap();
    assert_eq!(
        read_all(&mut fs, small),
        b"inline me\nmore data beyond inline\n"
    );
    drop(fs);
    check(&path);

    fs::remove_dir_all(&src_dir).unwrap();
    fs::remove_file(&path).unwrap();
}

#[test]
fn grow_to_device_size() {
    if !have_progs() {
        eprintln!("btrfs-progs not available; skipping");
        return;
    }
    let path = tmpfile("grow", 64 * 1024 * 1024);
    let dev = open_dev(&path);
    mkfs::format(&*dev, &opts()).unwrap();
    drop(dev);
    // Simulate the installer copying the image onto a larger partition.
    let f = fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.set_len(512 * 1024 * 1024).unwrap();
    drop(f);
    let dev = open_dev(&path);
    let mut fs = Btrfs::mount(dev, false).unwrap();
    assert!(fs.grow_to_device().unwrap());
    let root = fs.root_ino();
    // Write enough data to require new chunks beyond the original 64 MiB.
    let blob = vec![0xabu8; 1024 * 1024];
    for i in 0..100 {
        let f = fs
            .create(root, &format!("blob-{:03}", i), FileKind::Regular, 0o644, 0)
            .unwrap();
        fs.write(f, 0, &blob).unwrap();
    }
    fs.sync().unwrap();
    let st = fs.fsinfo();
    assert!(st.total_bytes >= 512 * 1024 * 1024 - 4096);
    drop(fs);
    check(&path);
    fs::remove_file(&path).unwrap();
}

#[test]
fn stress_random_ops() {
    if !have_progs() {
        eprintln!("btrfs-progs not available; skipping");
        return;
    }
    let path = tmpfile("stress", 128 * 1024 * 1024);
    let dev = open_dev(&path);
    mkfs::format(&*dev, &opts()).unwrap();
    let mut fs = Btrfs::mount(dev, false).unwrap();
    let root = fs.root_ino();

    // Simple LCG for reproducible pseudo-random decisions.
    let mut seed = 0xdead_beef_u64;
    let mut rng = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };

    // live[i] = (dir_ino, name, ino)
    let mut live: Vec<(u64, String, u64)> = Vec::new();
    let mut dirs: Vec<u64> = vec![root];
    let mut counter = 0usize;

    for step in 0..4000 {
        match rng() % 10 {
            0 | 1 => {
                // mkdir
                let parent = dirs[rng() % dirs.len()];
                let name = format!("d{}", counter);
                counter += 1;
                let ino = fs.create(parent, &name, FileKind::Dir, 0o755, 0).unwrap();
                dirs.push(ino);
                live.push((parent, name, ino));
            }
            2..=5 => {
                // create file + write random-sized data
                let parent = dirs[rng() % dirs.len()];
                let name = format!("f{}", counter);
                counter += 1;
                let ino = fs
                    .create(parent, &name, FileKind::Regular, 0o644, 0)
                    .unwrap();
                let size = rng() % 200_000;
                let data: Vec<u8> = (0..size).map(|i| (i ^ step) as u8).collect();
                let mut off = 0u64;
                'chunks: for chunk in data.chunks(33333) {
                    // Disk-full is expected near the end of the run; the
                    // filesystem must stay consistent through it.
                    match fs.write(ino, off, chunk) {
                        Ok(n) => {
                            off += n as u64;
                            if n < chunk.len() {
                                break 'chunks;
                            }
                        }
                        Err(btrfs::Error::NoSpace) => break 'chunks,
                        Err(e) => panic!("write: {:?}", e),
                    }
                }
                live.push((parent, name, ino));
            }
            6 => {
                // symlink
                let parent = dirs[rng() % dirs.len()];
                let name = format!("s{}", counter);
                counter += 1;
                let ino = fs.symlink(parent, &name, b"../some/target").unwrap();
                live.push((parent, name, ino));
            }
            7 => {
                // truncate a random file
                if let Some(&(_, _, ino)) = live.get(rng() % live.len().max(1)) {
                    let st = fs.stat(ino).unwrap();
                    if st.kind == FileKind::Regular {
                        fs.truncate(ino, (rng() % 300_000) as u64).unwrap();
                    }
                }
            }
            8 => {
                // unlink a random non-dir entry
                if !live.is_empty() {
                    let i = rng() % live.len();
                    let (parent, name, ino) = live[i].clone();
                    let st = fs.stat(ino).unwrap();
                    if st.kind != FileKind::Dir {
                        fs.unlink(parent, &name).unwrap();
                        live.remove(i);
                    }
                }
            }
            _ => {
                // rename into another directory
                if !live.is_empty() {
                    let i = rng() % live.len();
                    let (parent, name, ino) = live[i].clone();
                    let st = fs.stat(ino).unwrap();
                    if st.kind != FileKind::Dir {
                        let new_parent = dirs[rng() % dirs.len()];
                        let new_name = format!("r{}", counter);
                        counter += 1;
                        fs.rename(parent, &name, new_parent, &new_name).unwrap();
                        live[i] = (new_parent, new_name, ino);
                    }
                }
            }
        }
    }
    fs.sync().unwrap();
    drop(fs);
    check(&path);
    fs::remove_file(&path).unwrap();
}
