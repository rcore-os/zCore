//! Generación de imágenes btrfs (rootfs y plantilla de HOME) usando el
//! crate `btrfs` del propio árbol — sin depender de btrfs-progs en el host.

use btrfs::device::{BlockDevice, FileDevice};
use btrfs::{mkfs, Btrfs, FileKind};
use rand::RngCore;
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn random_uuid() -> [u8; 16] {
    let mut u = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut u);
    // Bits de versión (4) y variante RFC 4122, como uuidgen.
    u[6] = (u[6] & 0x0f) | 0x40;
    u[8] = (u[8] & 0x3f) | 0x80;
    u
}

fn mkfs_options(label: &str) -> mkfs::MkfsOptions {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    mkfs::MkfsOptions {
        label: label.into(),
        fsid: random_uuid(),
        chunk_uuid: random_uuid(),
        dev_uuid: random_uuid(),
        subvol_uuid: random_uuid(),
        now: (now.as_secs(), now.subsec_nanos()),
    }
}

/// Crea `image` (de `size` bytes) con un btrfs etiquetado `label` y, si se
/// indica, lo puebla con el contenido de `rootdir`.
pub fn make_btrfs_image(image: &Path, size: u64, label: &str, rootdir: Option<&Path>) {
    let _ = fs::remove_file(image);
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(image)
        .expect("no se pudo crear la imagen btrfs");
    file.set_len(size)
        .expect("no se pudo dimensionar la imagen");
    let dev: Arc<dyn BlockDevice> = Arc::new(FileDevice::open(file).unwrap());
    mkfs::format(&*dev, &mkfs_options(label)).expect("mkfs btrfs falló");
    let mut fs = Btrfs::mount(dev, false).expect("no se pudo montar la imagen recién formateada");
    if let Some(dir) = rootdir {
        let root = fs.root_ino();
        populate(&mut fs, root, dir);
    }
    fs.sync().expect("sync de la imagen btrfs falló");
}

fn populate(fs: &mut Btrfs, dir_ino: u64, dir: &Path) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {:?}: {}", dir, e))
        .map(|e| e.unwrap())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_str().expect("nombre de archivo no UTF-8");
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).unwrap();
        let mode = meta.permissions().mode() & 0o7777;
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&path).unwrap();
            let target = target
                .as_os_str()
                .to_str()
                .expect("destino de symlink no UTF-8");
            fs.symlink(dir_ino, name, target.as_bytes())
                .unwrap_or_else(|e| panic!("symlink {:?}: {:?}", path, e));
        } else if meta.is_dir() {
            let ino = fs
                .create(dir_ino, name, FileKind::Dir, mode, 0)
                .unwrap_or_else(|e| panic!("mkdir {:?}: {:?}", path, e));
            populate(fs, ino, &path);
        } else if meta.is_file() {
            let ino = fs
                .create(dir_ino, name, FileKind::Regular, mode, 0)
                .unwrap_or_else(|e| panic!("create {:?}: {:?}", path, e));
            let data = fs::read(&path).unwrap();
            let mut off = 0u64;
            for chunk in data.chunks(1024 * 1024) {
                fs.write(ino, off, chunk)
                    .unwrap_or_else(|e| panic!("write {:?}: {:?}", path, e));
                off += chunk.len() as u64;
            }
        } else {
            // Nodos de dispositivo / FIFOs en el rootfs de build: poco
            // habituales; se preservan con su rdev.
            let kind = if meta.file_type().is_fifo() {
                FileKind::Fifo
            } else if meta.file_type().is_block_device() {
                FileKind::BlockDevice
            } else if meta.file_type().is_char_device() {
                FileKind::CharDevice
            } else {
                continue;
            };
            fs.create(dir_ino, name, kind, mode, meta.rdev())
                .unwrap_or_else(|e| panic!("mknod {:?}: {:?}", path, e));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn imagen_btrfs_valida() {
        if !Command::new("btrfs")
            .arg("version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            eprintln!("btrfs-progs no disponible; test omitido");
            return;
        }
        let base = std::env::temp_dir().join(format!("xtask-btrfs-{}", std::process::id()));
        let src = base.join("rootfs");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(src.join("bin")).unwrap();
        fs::create_dir_all(src.join("etc")).unwrap();
        fs::write(src.join("bin/busybox"), vec![0x7fu8; 2 * 1024 * 1024]).unwrap();
        fs::set_permissions(src.join("bin/busybox"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("busybox", src.join("bin/sh")).unwrap();
        fs::write(src.join("etc/fstab"), "# fstab\n").unwrap();

        let img = base.join("rootfs.btrfs");
        make_btrfs_image(&img, 96 * 1024 * 1024, "ECLIPSE", Some(&src));

        let out = Command::new("btrfs")
            .args(["check", "--force"])
            .arg(&img)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "btrfs check falló:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );

        let restored = base.join("restored");
        fs::create_dir_all(&restored).unwrap();
        let out = Command::new("btrfs")
            .args(["restore", "-v"])
            .arg(&img)
            .arg(&restored)
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            fs::read(restored.join("bin/busybox")).unwrap(),
            vec![0x7fu8; 2 * 1024 * 1024]
        );
        assert_eq!(fs::read(restored.join("etc/fstab")).unwrap(), b"# fstab\n");

        // Plantilla HOME vacía.
        let home = base.join("home.btrfs");
        make_btrfs_image(&home, 32 * 1024 * 1024, "HOME", None);
        let out = Command::new("btrfs")
            .args(["check", "--force"])
            .arg(&home)
            .output()
            .unwrap();
        assert!(out.status.success());

        fs::remove_dir_all(&base).unwrap();
    }
}
