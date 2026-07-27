//! FAT12/16/32 (vfat) mount support via fatfs 0.4.

use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::any::Any;
use core::cmp::min;
use core::sync::atomic::{AtomicU64, Ordering};

/// Asigna un identificador de dispositivo único a cada FS FAT montado.
/// Empieza alto para no colisionar con los ids de otros sistemas de archivos.
static FAT_DEV_COUNTER: AtomicU64 = AtomicU64::new(0xFA70_0000);

/// Hash FNV-1a de 64 bits del path, usado como número de inodo estable.
/// Evita que dos rutas distintas (o la misma ruta en montajes distintos)
/// compartan identidad (st_dev, st_ino) y confundan a herramientas como `cp`.
fn fnv1a_path(path: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

use fatfs::{FileSystem, FsOptions, IoBase, Read, Seek, SeekFrom, Write};
use lock::Mutex;
use rcore_fs::vfs::{
    FileSystem as VfsFileSystem, FileType, FsError, FsInfo, INode, Metadata, PollStatus, Timespec,
};

use super::block_mount::MountBackend;

struct FatDisk {
    backend: MountBackend,
    pos: u64,
}

impl FatDisk {
    fn len(&self) -> u64 {
        match &self.backend {
            MountBackend::Block(block) => block.block_count() as u64 * 512,
            MountBackend::File(file) => file.metadata().map(|m| m.size as u64).unwrap_or(0),
        }
    }

    fn read_block_bytes(&self, offset: usize, buf: &mut [u8]) -> core::result::Result<usize, ()> {
        match &self.backend {
            MountBackend::Block(block) => {
                let block_size = 512;
                let mut done = 0;
                while done < buf.len() {
                    let abs = offset + done;
                    let block_id = abs / block_size;
                    let block_off = abs % block_size;
                    let take = min(buf.len() - done, block_size - block_off);
                    let mut temp = [0u8; 512];
                    block.read_block(block_id, &mut temp).map_err(|_| ())?;
                    buf[done..done + take].copy_from_slice(&temp[block_off..block_off + take]);
                    done += take;
                }
                Ok(done)
            }
            MountBackend::File(file) => {
                let len = file.metadata().map(|m| m.size).unwrap_or(0);
                if offset >= len {
                    return Ok(0);
                }
                let take = min(buf.len(), len - offset);
                file.read_at(offset, &mut buf[..take]).map_err(|_| ())
            }
        }
    }

    fn write_block_bytes(&self, offset: usize, buf: &[u8]) -> core::result::Result<usize, ()> {
        match &self.backend {
            MountBackend::Block(block) => {
                let block_size = 512;
                let mut done = 0;
                while done < buf.len() {
                    let abs = offset + done;
                    let block_id = abs / block_size;
                    let block_off = abs % block_size;
                    let take = min(buf.len() - done, block_size - block_off);
                    let mut temp = [0u8; 512];
                    if block_off != 0 || take != block_size {
                        block.read_block(block_id, &mut temp).map_err(|_| ())?;
                    }
                    temp[block_off..block_off + take].copy_from_slice(&buf[done..done + take]);
                    block.write_block(block_id, &temp).map_err(|_| ())?;
                    done += take;
                }
                Ok(done)
            }
            MountBackend::File(file) => {
                let len = file.metadata().map(|m| m.size).unwrap_or(0);
                if offset >= len {
                    return Ok(0);
                }
                let take = min(buf.len(), len - offset);
                file.write_at(offset, &buf[..take]).map_err(|_| ())
            }
        }
    }
}

impl IoBase for FatDisk {
    type Error = ();
}

impl Read for FatDisk {
    fn read(&mut self, buf: &mut [u8]) -> core::result::Result<usize, Self::Error> {
        let n = self.read_block_bytes(self.pos as usize, buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Write for FatDisk {
    fn write(&mut self, buf: &[u8]) -> core::result::Result<usize, Self::Error> {
        let n = self.write_block_bytes(self.pos as usize, buf)?;
        self.pos += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> core::result::Result<(), Self::Error> {
        Ok(())
    }
}

impl Seek for FatDisk {
    fn seek(&mut self, pos: SeekFrom) -> core::result::Result<u64, Self::Error> {
        let len = self.len();
        self.pos = match pos {
            SeekFrom::Start(s) => s,
            SeekFrom::Current(off) => (self.pos as i64 + off) as u64,
            SeekFrom::End(off) => (len as i64 + off) as u64,
        };
        Ok(self.pos)
    }
}

pub struct FatMountFs {
    inner: Mutex<FileSystem<FatDisk>>,
    this: Mutex<Weak<Self>>,
    /// Identificador de dispositivo único de este montaje (para st_dev).
    dev: u64,
}

impl FatMountFs {
    pub fn open(backend: MountBackend) -> rcore_fs::vfs::Result<Arc<Self>> {
        let disk = FatDisk { backend, pos: 0 };
        let fs = FileSystem::new(disk, FsOptions::new()).map_err(|_| FsError::DeviceError)?;
        let arc = Arc::new(Self {
            inner: Mutex::new(fs),
            this: Mutex::new(Weak::new()),
            dev: FAT_DEV_COUNTER.fetch_add(1, Ordering::Relaxed),
        });
        *arc.this.lock() = Arc::downgrade(&arc);
        Ok(arc)
    }

    fn arc(&self) -> Arc<Self> {
        self.this.lock().upgrade().expect("FatMountFs dropped")
    }
}

impl VfsFileSystem for FatMountFs {
    fn sync(&self) -> rcore_fs::vfs::Result<()> {
        Ok(())
    }

    fn root_inode(&self) -> Arc<dyn INode> {
        Arc::new(FatMountINode {
            fs: self.arc(),
            path: String::new(),
            is_dir: true,
        })
    }

    fn info(&self) -> FsInfo {
        let fs = self.inner.lock();
        if let Ok(stats) = fs.stats() {
            let cluster_size = stats.cluster_size() as usize;
            FsInfo {
                bsize: cluster_size,
                frsize: cluster_size,
                blocks: stats.total_clusters() as usize,
                bfree: stats.free_clusters() as usize,
                bavail: stats.free_clusters() as usize,
                files: 0,
                ffree: 0,
                namemax: 255,
            }
        } else {
            FsInfo {
                bsize: 512,
                frsize: 512,
                blocks: 0,
                bfree: 0,
                bavail: 0,
                files: 0,
                ffree: 0,
                namemax: 255,
            }
        }
    }
}

struct FatMountINode {
    fs: Arc<FatMountFs>,
    path: String,
    is_dir: bool,
}

impl FatMountINode {
    fn child_path(&self, name: &str) -> String {
        if self.path.is_empty() {
            name.to_string()
        } else {
            alloc::format!("{}/{}", self.path, name)
        }
    }
}

impl INode for FatMountINode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> rcore_fs::vfs::Result<usize> {
        if self.is_dir {
            return Err(FsError::IsDir);
        }
        let fs = self.fs.inner.lock();
        let mut file = fs
            .root_dir()
            .open_file(&self.path)
            .map_err(|_| FsError::EntryNotFound)?;
        file.seek(SeekFrom::Start(offset as u64))
            .map_err(|_| FsError::DeviceError)?;
        file.read(buf).map_err(|_| FsError::DeviceError)
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> rcore_fs::vfs::Result<usize> {
        if self.is_dir {
            return Err(FsError::IsDir);
        }
        let fs = self.fs.inner.lock();
        let mut file = fs
            .root_dir()
            .open_file(&self.path)
            .map_err(|_| FsError::EntryNotFound)?;
        file.seek(SeekFrom::Start(offset as u64))
            .map_err(|_| FsError::DeviceError)?;
        file.write(buf).map_err(|_| FsError::DeviceError)
    }

    fn poll(&self) -> rcore_fs::vfs::Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: !self.is_dir,
            error: false,
        })
    }

    fn metadata(&self) -> rcore_fs::vfs::Result<Metadata> {
        let size = if self.is_dir {
            0
        } else {
            let fs = self.fs.inner.lock();
            let mut file = fs
                .root_dir()
                .open_file(&self.path)
                .map_err(|_| FsError::EntryNotFound)?;
            file.seek(SeekFrom::End(0))
                .map_err(|_| FsError::DeviceError)? as usize
        };
        Ok(Metadata {
            dev: self.fs.dev as usize,
            inode: fnv1a_path(&self.path) as usize,
            size,
            blk_size: 512,
            blocks: size.div_ceil(512),
            atime: Timespec { sec: 0, nsec: 0 },
            mtime: Timespec { sec: 0, nsec: 0 },
            ctime: Timespec { sec: 0, nsec: 0 },
            type_: if self.is_dir {
                FileType::Dir
            } else {
                FileType::File
            },
            mode: if self.is_dir { 0o755 } else { 0o644 },
            nlinks: 1,
            uid: 0,
            gid: 0,
            rdev: 0,
        })
    }

    /// FAT no persiste uid/gid/mode; aceptar la llamada como no-op.
    ///
    /// `open(O_CREAT)` invoca `set_metadata` justo después de crear el fichero
    /// (initialize_created_metadata). Con el default del trait (NotSupported →
    /// ENOSYS) cualquier creación de fichero sobre vfat fallaba con
    /// "Function not implemented".
    fn set_metadata(&self, _metadata: &Metadata) -> rcore_fs::vfs::Result<()> {
        Ok(())
    }

    fn find(&self, name: &str) -> rcore_fs::vfs::Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(FatMountINode {
                fs: self.fs.clone(),
                path: self.path.clone(),
                is_dir: self.is_dir,
            })),
            ".." => Err(FsError::EntryNotFound),
            name => {
                let fs = self.fs.inner.lock();
                let dir = if self.path.is_empty() {
                    fs.root_dir()
                } else {
                    fs.root_dir()
                        .open_dir(&self.path)
                        .map_err(|_| FsError::EntryNotFound)?
                };
                for entry in dir.iter() {
                    let entry = entry.map_err(|_| FsError::DeviceError)?;
                    // FAT es case-insensitive: una entrada 8.3 puede estar
                    // almacenada como "BOOTX64.EFI" y buscarse "BootX64.efi".
                    // Con comparación exacta el lookup fallaba (EntryNotFound)
                    // y open(O_CREAT) intentaba re-crear el fichero existente.
                    let entry_name = entry.file_name();
                    if entry_name.eq_ignore_ascii_case(name) {
                        return Ok(Arc::new(FatMountINode {
                            fs: self.fs.clone(),
                            // Usar el nombre tal como está en el directorio para
                            // que open_file/open_dir posteriores lo encuentren.
                            path: self.child_path(&entry_name),
                            is_dir: entry.is_dir(),
                        }));
                    }
                }
                Err(FsError::EntryNotFound)
            }
        }
    }

    fn get_entry(&self, id: usize) -> rcore_fs::vfs::Result<String> {
        match id {
            0 => Ok(String::from(".")),
            1 => Ok(String::from("..")),
            i => {
                let fs = self.fs.inner.lock();
                let dir = if self.path.is_empty() {
                    fs.root_dir()
                } else {
                    fs.root_dir()
                        .open_dir(&self.path)
                        .map_err(|_| FsError::EntryNotFound)?
                };
                let mut names = Vec::new();
                for entry in dir.iter() {
                    let entry = entry.map_err(|_| FsError::DeviceError)?;
                    let name = entry.file_name();
                    if !name.is_empty() {
                        names.push(name);
                    }
                }
                names.get(i - 2).cloned().ok_or(FsError::EntryNotFound)
            }
        }
    }

    fn create(
        &self,
        name: &str,
        type_: rcore_fs::vfs::FileType,
        _mode: u32,
    ) -> rcore_fs::vfs::Result<Arc<dyn INode>> {
        if !self.is_dir {
            return Err(FsError::NotDir);
        }
        let fs = self.fs.inner.lock();
        let dir = if self.path.is_empty() {
            fs.root_dir()
        } else {
            fs.root_dir()
                .open_dir(&self.path)
                .map_err(|_| FsError::EntryNotFound)?
        };
        match type_ {
            rcore_fs::vfs::FileType::File => {
                let _ = dir.create_file(name).map_err(|_| FsError::NoDeviceSpace)?;
            }
            rcore_fs::vfs::FileType::Dir => {
                let _ = dir.create_dir(name).map_err(|_| FsError::NoDeviceSpace)?;
            }
            _ => return Err(FsError::NotSupported),
        }
        Ok(Arc::new(FatMountINode {
            fs: self.fs.clone(),
            path: self.child_path(name),
            is_dir: type_ == rcore_fs::vfs::FileType::Dir,
        }))
    }

    fn unlink(&self, name: &str) -> rcore_fs::vfs::Result<()> {
        if !self.is_dir {
            return Err(FsError::NotDir);
        }
        let fs = self.fs.inner.lock();
        let dir = if self.path.is_empty() {
            fs.root_dir()
        } else {
            fs.root_dir()
                .open_dir(&self.path)
                .map_err(|_| FsError::EntryNotFound)?
        };
        dir.remove(name).map_err(|e| match e {
            fatfs::Error::DirectoryIsNotEmpty => FsError::DirNotEmpty,
            fatfs::Error::NotFound => FsError::EntryNotFound,
            _ => FsError::DeviceError,
        })
    }

    fn resize(&self, len: usize) -> rcore_fs::vfs::Result<()> {
        if self.is_dir {
            return Err(FsError::IsDir);
        }
        let fs = self.fs.inner.lock();
        let mut file = fs
            .root_dir()
            .open_file(&self.path)
            .map_err(|_| FsError::EntryNotFound)?;
        let cur = file
            .seek(SeekFrom::End(0))
            .map_err(|_| FsError::DeviceError)? as usize;
        if len > cur {
            if len > 0 {
                file.seek(SeekFrom::Start((len - 1) as u64))
                    .map_err(|_| FsError::DeviceError)?;
                file.write(&[0]).map_err(|_| FsError::DeviceError)?;
            }
        } else if len < cur {
            file.seek(SeekFrom::Start(len as u64))
                .map_err(|_| FsError::DeviceError)?;
            file.truncate().map_err(|_| FsError::DeviceError)?;
        }
        Ok(())
    }

    fn sync_all(&self) -> rcore_fs::vfs::Result<()> {
        Ok(())
    }

    fn fs(&self) -> Arc<dyn VfsFileSystem> {
        self.fs.clone()
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}

pub fn open_fat(backend: MountBackend) -> rcore_fs::vfs::Result<Arc<dyn VfsFileSystem>> {
    FatMountFs::open(backend).map(|fs| fs as Arc<dyn VfsFileSystem>)
}
