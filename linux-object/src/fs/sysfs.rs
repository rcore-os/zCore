//! Minimal sysfs implementation for Linux userland compatibility.

use alloc::{string::String, sync::Arc, vec::Vec};
use core::any::Any;

use kernel_hal::drivers;
use kernel_hal::net::get_net_device;
use lazy_static::lazy_static;
use lock::Mutex;
use rcore_fs::vfs::{
    FileSystem, FileType, FsError, FsInfo, INode, Metadata, PollStatus, Result, Timespec,
};

use crate::fs::pseudo::Pseudo;

pub struct SysFS;

impl SysFS {
    pub fn new() -> Self {
        Self
    }
}

impl FileSystem for SysFS {
    fn sync(&self) -> Result<()> {
        Ok(())
    }

    fn root_inode(&self) -> Arc<dyn INode> {
        SYS_ROOT.clone()
    }

    fn info(&self) -> FsInfo {
        FsInfo {
            bsize: 4096,
            frsize: 4096,
            blocks: 0,
            bfree: 0,
            bavail: 0,
            files: 0,
            ffree: 0,
            namemax: 255,
        }
    }
}

fn dir_metadata(inode: usize) -> Metadata {
    Metadata {
        dev: 0,
        inode,
        size: 0,
        blk_size: 0,
        blocks: 0,
        atime: Timespec { sec: 0, nsec: 0 },
        mtime: Timespec { sec: 0, nsec: 0 },
        ctime: Timespec { sec: 0, nsec: 0 },
        type_: FileType::Dir,
        mode: 0o555,
        nlinks: 2,
        uid: 0,
        gid: 0,
        rdev: 0,
    }
}

fn list_block_devices() -> Vec<String> {
    let blocks = drivers::all_block().as_vec();
    let mut names = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        let name = block.name();
        let fname = if name.starts_with("nvme") {
            let nvme_idx = blocks[..i]
                .iter()
                .filter(|b| b.name().starts_with("nvme"))
                .count();
            format!("nvme{}n1", nvme_idx)
        } else if name.starts_with("virtio") {
            let virtio_idx = blocks[..i]
                .iter()
                .filter(|b| b.name().starts_with("virtio"))
                .count();
            let name_char = (b'a' + (virtio_idx % 26) as u8) as char;
            format!("vd{}", name_char)
        } else {
            let other_idx = blocks[..i]
                .iter()
                .filter(|b| !b.name().starts_with("nvme") && !b.name().starts_with("virtio"))
                .count();
            let name_char = (b'a' + (other_idx % 26) as u8) as char;
            format!("sd{}", name_char)
        };
        names.push(fname);
    }
    names
}

fn block_index_by_name(name: &str) -> Option<usize> {
    list_block_devices().iter().position(|n| n.as_str() == name)
}

fn block_size_sectors(index: usize) -> Option<usize> {
    drivers::all_block()
        .as_vec()
        .get(index)
        .map(|b| b.block_count())
}

struct SysRootINode;

impl SysRootINode {
    fn entries() -> [&'static str; 6] {
        ["class", "block", "bus", "dev", "devices", "power"]
    }
}

impl INode for SysRootINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }

    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }

    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(10))
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }

    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." | ".." => Ok(SYS_ROOT.clone()),
            "class" => Ok(Arc::new(SysClassINode)),
            "block" => Ok(Arc::new(SysBlockDirINode)),
            "bus" => Ok(Arc::new(SysBusDirINode)),
            "dev" => Ok(Arc::new(SysDevDirINode)),
            "devices" => Ok(Arc::new(SysDevicesDirINode)),
            "power" => Ok(Arc::new(SysPowerDirINode)),
            _ => Err(FsError::EntryNotFound),
        }
    }

    fn get_entry(&self, id: usize) -> Result<String> {
        let entries = Self::entries();
        if id >= entries.len() {
            return Err(FsError::EntryNotFound);
        }
        Ok(entries[id].into())
    }
}

struct SysClassINode;

impl SysClassINode {
    fn entries() -> [&'static str; 6] {
        ["block", "drm", "input", "net", "power_supply", "thermal"]
    }
}

impl INode for SysClassINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }

    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }

    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(20))
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }

    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysClassINode)),
            ".." => Ok(Arc::new(SysRootINode)),
            "block" => Ok(Arc::new(SysBlockDirINode)),
            "drm" => Ok(Arc::new(SysClassDrmDirINode)),
            "input" => Ok(Arc::new(SysClassInputDirINode)),
            "net" => Ok(Arc::new(SysClassNetDirINode)),
            "power_supply" => Ok(Arc::new(SysClassPowerSupplyDirINode)),
            "thermal" => Ok(Arc::new(SysClassThermalDirINode)),
            _ => Err(FsError::EntryNotFound),
        }
    }

    fn get_entry(&self, id: usize) -> Result<String> {
        let entries = Self::entries();
        if id >= entries.len() {
            return Err(FsError::EntryNotFound);
        }
        Ok(entries[id].into())
    }
}

struct SysBlockDirINode;

impl INode for SysBlockDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }

    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }

    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(30))
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }

    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysBlockDirINode)),
            ".." => Ok(Arc::new(SysRootINode)),
            _ => {
                if let Some(index) = block_index_by_name(name) {
                    Ok(Arc::new(SysBlockDevINode { index }))
                } else {
                    Err(FsError::EntryNotFound)
                }
            }
        }
    }

    fn get_entry(&self, id: usize) -> Result<String> {
        let entries = list_block_devices();
        if id >= entries.len() {
            return Err(FsError::EntryNotFound);
        }
        Ok(entries[id].clone())
    }
}

struct SysBlockDevINode {
    index: usize,
}

impl SysBlockDevINode {
    fn entries() -> [&'static str; 2] {
        ["size", "removable"]
    }
}

impl INode for SysBlockDevINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }

    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }

    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(40 + self.index))
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }

    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysBlockDevINode { index: self.index })),
            ".." => Ok(Arc::new(SysBlockDirINode)),
            "size" => {
                let sectors = block_size_sectors(self.index).ok_or(FsError::EntryNotFound)?;
                Ok(Arc::new(Pseudo::new(
                    &format!("{}\n", sectors),
                    FileType::File,
                )))
            }
            "removable" => Ok(Arc::new(Pseudo::new("0\n", FileType::File))),
            _ => Err(FsError::EntryNotFound),
        }
    }

    fn get_entry(&self, id: usize) -> Result<String> {
        let entries = Self::entries();
        if id >= entries.len() {
            return Err(FsError::EntryNotFound);
        }
        Ok(entries[id].into())
    }
}

struct SysBusDirINode;

impl INode for SysBusDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(50))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysBusDirINode)),
            ".." => Ok(Arc::new(SysRootINode)),
            "pci" => Ok(Arc::new(SysBusPciDirINode)),
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        if id == 0 {
            Ok("pci".into())
        } else {
            Err(FsError::EntryNotFound)
        }
    }
}

struct SysBusPciDirINode;

impl INode for SysBusPciDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(60))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysBusPciDirINode)),
            ".." => Ok(Arc::new(SysBusDirINode)),
            "devices" => Ok(Arc::new(SysPciDevicesDirINode)),
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        if id == 0 {
            Ok("devices".into())
        } else {
            Err(FsError::EntryNotFound)
        }
    }
}

struct SysDevicesDirINode;

impl INode for SysDevicesDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(70))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysDevicesDirINode)),
            ".." => Ok(Arc::new(SysRootINode)),
            "pci0000:00" => Ok(Arc::new(SysDevicesPciBusDirINode)),
            "system" => Ok(Arc::new(SysDevicesSystemDirINode)),
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        match id {
            0 => Ok("pci0000:00".into()),
            1 => Ok("system".into()),
            _ => Err(FsError::EntryNotFound),
        }
    }
}

struct SysDevicesSystemDirINode;

impl INode for SysDevicesSystemDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(110))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysDevicesSystemDirINode)),
            ".." => Ok(Arc::new(SysDevicesDirINode)),
            "node" => Ok(Arc::new(SysDevicesSystemNodeDirINode)),
            "cpu" => Ok(Arc::new(SysDevicesSystemCpuDirINode)),
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        match id {
            0 => Ok("node".into()),
            1 => Ok("cpu".into()),
            _ => Err(FsError::EntryNotFound),
        }
    }
}

struct SysDevicesSystemNodeDirINode;

impl INode for SysDevicesSystemNodeDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(120))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysDevicesSystemNodeDirINode)),
            ".." => Ok(Arc::new(SysDevicesSystemDirINode)),
            "node0" => Ok(Arc::new(SysDevicesSystemNode0DirINode)),
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        if id == 0 {
            Ok("node0".into())
        } else {
            Err(FsError::EntryNotFound)
        }
    }
}

struct SysDevicesSystemNode0DirINode;

impl SysDevicesSystemNode0DirINode {
    fn entries() -> [&'static str; 2] {
        ["cpulist", "cpumap"]
    }
}

impl INode for SysDevicesSystemNode0DirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(130))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysDevicesSystemNode0DirINode)),
            ".." => Ok(Arc::new(SysDevicesSystemNodeDirINode)),
            "cpulist" => {
                let cpu_count = kernel_hal::cpu::cpu_count() as usize;
                let cpulist = if cpu_count <= 1 {
                    String::from("0\n")
                } else {
                    format!("0-{}\n", cpu_count - 1)
                };
                Ok(Arc::new(Pseudo::new(&cpulist, FileType::File)))
            }
            "cpumap" => {
                let cpu_count = kernel_hal::cpu::cpu_count() as usize;
                let mut cpumap = String::new();
                let num_groups = cpu_count.div_ceil(32);
                for g in (0..num_groups).rev() {
                    let mut group_val = 0u32;
                    for i in 0..32 {
                        let cpu_idx = g * 32 + i;
                        if cpu_idx < cpu_count {
                            group_val |= 1 << i;
                        }
                    }
                    if !cpumap.is_empty() {
                        cpumap.push(',');
                    }
                    cpumap.push_str(&format!("{:08x}", group_val));
                }
                cpumap.push('\n');
                Ok(Arc::new(Pseudo::new(&cpumap, FileType::File)))
            }
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        let entries = Self::entries();
        if id >= entries.len() {
            return Err(FsError::EntryNotFound);
        }
        Ok(entries[id].into())
    }
}

struct SysDevicesPciBusDirINode;

impl INode for SysDevicesPciBusDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(80))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        log::trace!("SysDevicesPciBusDirINode::find name={}", name);
        if name == "." {
            return Ok(Arc::new(SysDevicesPciBusDirINode));
        }
        if name == ".." {
            return Ok(Arc::new(SysDevicesDirINode));
        }
        let devices = get_pci_devices();
        if let Some((idx, dev)) = devices.iter().enumerate().find(|(_, d)| d.name == name) {
            log::trace!("SysDevicesPciBusDirINode::find name={} -> found", name);
            Ok(Arc::new(SysPciDevDirINode {
                index: idx,
                name: dev.name.clone(),
                vendor: dev.vendor.clone(),
                device: dev.device.clone(),
                class: dev.class.clone(),
            }))
        } else {
            log::trace!(
                "SysDevicesPciBusDirINode::find name={} -> EntryNotFound",
                name
            );
            Err(FsError::EntryNotFound)
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        let devices = get_pci_devices();
        if id >= devices.len() {
            return Err(FsError::EntryNotFound);
        }
        Ok(devices[id].name.clone())
    }
}

struct SysPciDevicesDirINode;

impl INode for SysPciDevicesDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(90))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        log::trace!("SysPciDevicesDirINode::find name={}", name);
        if name == "." {
            return Ok(Arc::new(SysPciDevicesDirINode));
        }
        if name == ".." {
            return Ok(Arc::new(SysBusPciDirINode));
        }
        let devices = get_pci_devices();
        if let Some(dev) = devices.iter().find(|d| d.name == name) {
            let target = format!("../../../devices/pci0000:00/{}", dev.name);
            log::trace!(
                "SysPciDevicesDirINode::find name={} -> target={}",
                name,
                target
            );
            Ok(Arc::new(Pseudo::new(&target, FileType::SymLink)))
        } else {
            log::trace!("SysPciDevicesDirINode::find name={} -> EntryNotFound", name);
            Err(FsError::EntryNotFound)
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        let devices = get_pci_devices();
        if id >= devices.len() {
            return Err(FsError::EntryNotFound);
        }
        Ok(devices[id].name.clone())
    }
}

struct SysPciDevDirINode {
    index: usize,
    name: String,
    vendor: String,
    device: String,
    class: String,
}

impl INode for SysPciDevDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(100 + self.index))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        log::trace!(
            "SysPciDevDirINode::find name={} self.name={}",
            name,
            self.name
        );
        match name {
            "." => Ok(Arc::new(SysPciDevDirINode {
                index: self.index,
                name: self.name.clone(),
                vendor: self.vendor.clone(),
                device: self.device.clone(),
                class: self.class.clone(),
            })),
            ".." => Ok(Arc::new(SysDevicesPciBusDirINode)),
            "vendor" => Ok(Arc::new(Pseudo::new(
                &format!("{}\n", self.vendor),
                FileType::File,
            ))),
            "device" => Ok(Arc::new(Pseudo::new(
                &format!("{}\n", self.device),
                FileType::File,
            ))),
            "class" => Ok(Arc::new(Pseudo::new(
                &format!("{}\n", self.class),
                FileType::File,
            ))),
            "uevent" => {
                let vendor_hex = self.vendor.trim_start_matches("0x");
                let device_hex = self.device.trim_start_matches("0x");
                let class_hex = self.class.trim_start_matches("0x");
                let uevent_content = format!(
                    "PCI_CLASS={}\nPCI_ID={}:{}\nPCI_SUBSYS_ID=0000:0000\nPCI_SLOT_NAME={}\n",
                    class_hex, vendor_hex, device_hex, self.name
                );
                Ok(Arc::new(Pseudo::new(&uevent_content, FileType::File)))
            }
            "config" => {
                let mut cfg = [0u8; 256];
                let v = u16::from_str_radix(self.vendor.trim_start_matches("0x"), 16).unwrap_or(0);
                let d = u16::from_str_radix(self.device.trim_start_matches("0x"), 16).unwrap_or(0);
                let c = u32::from_str_radix(self.class.trim_start_matches("0x"), 16).unwrap_or(0);

                cfg[0..2].copy_from_slice(&v.to_le_bytes());
                cfg[2..4].copy_from_slice(&d.to_le_bytes());

                cfg[9] = (c & 0xff) as u8;
                cfg[10] = ((c >> 8) & 0xff) as u8;
                cfg[11] = ((c >> 16) & 0xff) as u8;

                Ok(Arc::new(Pseudo::new_bytes(cfg.to_vec(), FileType::File)))
            }
            "modalias" => Ok(Arc::new(Pseudo::new(
                &pci_modalias(&self.vendor, &self.device, &self.class),
                FileType::File,
            ))),
            // libdrm's drmParseSubsystemType() readlink()s `<dev>/subsystem`
            // and takes the basename ("pci") to classify the bus; without it
            // drmGetDevice2() can't determine the device type.
            "subsystem" => Ok(Arc::new(Pseudo::new("../../../bus/pci", FileType::SymLink))),
            "revision" => Ok(Arc::new(Pseudo::new("0x00\n", FileType::File))),
            "subsystem_vendor" => Ok(Arc::new(Pseudo::new("0x0000\n", FileType::File))),
            "subsystem_device" => Ok(Arc::new(Pseudo::new("0x0000\n", FileType::File))),
            // `<dev>/drm/` lists this device's DRM nodes (card0/renderD128);
            // libdrm's drmGetRenderDeviceNameFromFd() scans it for the render
            // node. Reached via the card's `device` symlink that points here.
            "drm" => Ok(Arc::new(SysDrmDeviceDrmDirINode {
                pci_index: self.index,
            })),
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        let entries = [
            "vendor",
            "device",
            "class",
            "config",
            "uevent",
            "modalias",
            "subsystem",
            "revision",
            "subsystem_vendor",
            "subsystem_device",
            "drm",
        ];
        if id >= entries.len() {
            return Err(FsError::EntryNotFound);
        }
        Ok(entries[id].into())
    }
}

#[derive(Clone)]
struct PciDevInfo {
    name: String,
    vendor: String,
    device: String,
    class: String,
}

fn pci_modalias(vendor: &str, device: &str, class: &str) -> String {
    let v = u32::from_str_radix(vendor.trim_start_matches("0x"), 16).unwrap_or(0);
    let d = u32::from_str_radix(device.trim_start_matches("0x"), 16).unwrap_or(0);
    let c = class.trim_start_matches("0x");
    let (bc, sc, pi) = if c.len() >= 6 {
        (&c[0..2], &c[2..4], &c[4..6])
    } else {
        ("00", "00", "00")
    };
    format!(
        "pci:v{v:08x}d{d:08x}sv00000000sd00000000bc{bc}sc{sc}i{pi}\n",
        v = v,
        d = d,
        bc = bc,
        sc = sc,
        pi = pi
    )
}

fn display_pci_index() -> Option<usize> {
    let devs = get_pci_devices();
    // With the nouveau uAPI enabled there is a real NVIDIA GPU the driver bound
    // to, and the DRM node (card0/renderD128) MUST back onto that GPU: its
    // sysfs `device` symlink, `vendor`, `config` etc. are what libdrm reads to
    // identify the device. The plain "first display-class device" scan below is
    // wrong on any board that also carries an integrated GPU — the iGPU usually
    // enumerates first, so sysfs would report the iGPU's 0x8086/0x1002 vendor
    // for our render node. NVK filters candidate DRM devices by PCI vendor
    // 0x10de *before* opening them (it never reaches nouveau_ws_device_new, so
    // not one nouveau ioctl is issued); a non-NVIDIA vendor there makes NVK skip
    // the node entirely and vkEnumeratePhysicalDevices returns 0 GPUs. Prefer
    // the NVIDIA display-class device so the render node advertises the RTX.
    // The node's identity must describe the SAME GPU that serves its ioctls.
    // On a multi-GPU box those can disagree: the ioctl target is
    // `devfs::drm::get_primary_driver()`, while this function used to pick the
    // first display-class device in PCI scan order. libdrm feeds NVK the sysfs
    // side (bus info from uevent's PCI_SLOT_NAME, ids from `config`) while
    // every GETPARAM/NVIF answer comes from the driver side, so a mismatch
    // reports one card's PCI id with another card's VRAM size and topology.
    // (Mesa does assert the two device_ids agree, but Alpine builds with
    // `-Db_ndebug=true`, so that check is compiled out and the inconsistency
    // is silent.)
    //
    // Match on the BDF NUMBERS, never on the driver's display name: names
    // render the bus in DECIMAL ("nvidia-gpu-101:0.0") while sysfs paths use
    // HEX ("0000:65:00.0").
    if let Some((_dom, bus, dev, func)) = crate::fs::devfs::drm::get_primary_driver()
        .and_then(|d| d.pci_bdf())
    {
        let want = alloc::format!("0000:{:02x}:{:02x}.{:x}", bus, dev, func);
        if let Some(idx) = devs.iter().position(|d| d.name == want) {
            return Some(idx);
        }
        log::warn!(
            "[drm] primary DRM driver reports PCI {} but the sysfs PCI scan has no such device -- \
             falling back to scan order",
            want
        );
    }
    if zcore_drivers::display::nouveau_uapi_enabled() {
        if let Some(idx) = devs
            .iter()
            .position(|d| d.class.starts_with("0x03") && d.vendor == "0x10de")
        {
            return Some(idx);
        }
    }
    devs.iter()
        .position(|d| d.class.starts_with("0x03"))
        .or_else(|| (!devs.is_empty()).then_some(0))
}

/// Whether `/sys/class/drm/card0` should be exposed, and which PCI device (if
/// any) backs it. card0 exists whenever `/dev/dri/card0` does — i.e. there is a
/// real PCI GPU, a registered framebuffer display (UEFI GOP has no PCI GPU
/// node), or a DRM driver. The PCI index is best-effort, used only for the
/// `device`/`modalias` attributes.
fn drm_card0_pci_index() -> Option<usize> {
    if let Some(idx) = display_pci_index() {
        return Some(idx);
    }
    let have_fb =
        drivers::all_display().first().is_some() || !drivers::all_drm().as_vec().is_empty();
    have_fb.then_some(0)
}

/// One-shot boot diagnostic, only meaningful with the nouveau uAPI enabled:
/// dump the PCI inventory and which device the DRM render node backs onto.
/// On real hardware this is the fastest way to catch the render node pointing
/// at the wrong GPU (e.g. an integrated GPU winning the display-class scan),
/// which makes NVK's PCI-vendor filter skip our node before any nouveau ioctl.
/// Logged at `warn` so it survives the default boot log level.
pub(crate) fn log_drm_pci_backing() {
    let devs = get_pci_devices();
    // klog_info!, not log::warn!: real-hardware builds default to LOG=error,
    // which drops warn -- and this whole diagnostic is only reachable on the
    // opt-in nouveau experiment anyway, so make it survive the quiet level the
    // same way the "graphics: drm[N]" inventory line does.
    for (i, d) in devs.iter().enumerate() {
        kernel_hal::klog_info!(
            "[drm-probe] PCI[{}] {} vendor={} class={}",
            i,
            d.name,
            d.vendor,
            d.class
        );
    }
    // Print the ioctl target next to the sysfs identity: on a multi-GPU box
    // these must name the SAME card, and that is exactly what a single boot
    // log can now confirm.
    match crate::fs::devfs::drm::get_primary_driver() {
        Some(d) => kernel_hal::klog_info!(
            "[drm-probe] ioctl target (primary driver) = {:?} pci_bdf={:x?} console_gpu={}",
            d.name(),
            d.pci_bdf(),
            d.is_console_gpu()
        ),
        None => kernel_hal::klog_info!("[drm-probe] no primary DRM driver registered"),
    }
    let idx = drm_card0_pci_index();
    match idx.and_then(|i| devs.get(i)) {
        Some(d) => kernel_hal::klog_info!(
            "[drm-probe] render node backed by PCI[{:?}] {} vendor={} (NVK requires vendor=0x10de)",
            idx,
            d.name,
            d.vendor
        ),
        None => kernel_hal::klog_info!("[drm-probe] render node has NO PCI backing (idx={:?})", idx),
    }
    // Actively resolve the EXACT sysfs chain libdrm's drmGetDevices2 walks, so
    // one boot tells us whether it resolves at runtime and what it reports --
    // no userspace probe needed. A dangling `device` symlink (the PCI scan
    // missed the GPU) or a vendor != 0x10de is what makes NVK skip the node
    // before it issues a single ioctl.
    fn read_small(n: &Arc<dyn INode>) -> String {
        let mut b = [0u8; 64];
        match n.read_at(0, &mut b) {
            Ok(l) => String::from_utf8_lossy(&b[..l]).trim().into(),
            Err(_) => "<read-err>".into(),
        }
    }
    match SYS_ROOT.lookup_follow("dev/char/226:128/device", 40) {
        Ok(pcidir) => {
            let vendor = pcidir
                .find("vendor")
                .map(|n| read_small(&n))
                .unwrap_or_else(|_| "<no-vendor>".into());
            let subsystem = pcidir
                .find("subsystem")
                .map(|n| read_small(&n))
                .unwrap_or_else(|_| "<no-subsystem>".into());
            kernel_hal::klog_info!(
                "[drm-probe] sysfs chain resolves: renderD128/device -> vendor={} subsystem={} (libdrm CAN identify the node; needs vendor=0x10de + subsystem .../bus/pci)",
                vendor,
                subsystem
            );
        }
        Err(e) => kernel_hal::klog_info!(
            "[drm-probe] sysfs chain BROKEN: /sys/dev/char/226:128/device does not resolve ({:?}) -- libdrm cannot read vendor/subsystem, so NVK skips the node",
            e
        ),
    }
}

fn list_net_ifnames() -> Vec<String> {
    let ifaces = get_net_device();
    if ifaces.is_empty() {
        vec!["lo".into()]
    } else {
        ifaces.iter().map(|i| i.get_ifname()).collect()
    }
}

struct SysClassDrmDirINode;

impl INode for SysClassDrmDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(21))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysClassDrmDirINode)),
            ".." => Ok(Arc::new(SysClassINode)),
            "card0" => drm_card0_pci_index()
                .map(|idx| Arc::new(SysDrmNodeINode::card(idx)) as Arc<dyn INode>)
                .ok_or(FsError::EntryNotFound),
            "renderD128" => drm_card0_pci_index()
                .map(|idx| Arc::new(SysDrmNodeINode::render(idx)) as Arc<dyn INode>)
                .ok_or(FsError::EntryNotFound),
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        if drm_card0_pci_index().is_none() {
            return Err(FsError::EntryNotFound);
        }
        match id {
            0 => Ok("card0".into()),
            1 => Ok("renderD128".into()),
            _ => Err(FsError::EntryNotFound),
        }
    }
}

/// A DRM device node in sysfs: the primary node `card0` (minor 0) or the render
/// node `renderD128` (minor 128). Both share the same backing PCI device.
struct SysDrmNodeINode {
    pci_index: usize,
    minor: u32,
    devname: &'static str,
}

impl SysDrmNodeINode {
    fn card(pci_index: usize) -> Self {
        Self {
            pci_index,
            minor: 0,
            devname: "card0",
        }
    }
    fn render(pci_index: usize) -> Self {
        Self {
            pci_index,
            minor: 128,
            devname: "renderD128",
        }
    }
    fn entries() -> [&'static str; 4] {
        ["dev", "uevent", "device", "subsystem"]
    }
}

impl INode for SysDrmNodeINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(22))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysDrmNodeINode {
                pci_index: self.pci_index,
                minor: self.minor,
                devname: self.devname,
            })),
            ".." => Ok(Arc::new(SysClassDrmDirINode)),
            // libdrm/libudev read `dev` (major:minor) and `uevent`
            // (DEVNAME/MAJOR/MINOR). DRM major is 226.
            "dev" => Ok(Arc::new(Pseudo::new(
                &format!("226:{}\n", self.minor),
                FileType::File,
            ))),
            "uevent" => Ok(Arc::new(Pseudo::new(
                &format!(
                    "MAJOR=226\nMINOR={}\nDEVNAME=dri/{}\n",
                    self.minor, self.devname
                ),
                FileType::File,
            ))),
            "subsystem" => Ok(Arc::new(Pseudo::new(
                "../../../class/drm",
                FileType::SymLink,
            ))),
            // `<card>/device` must resolve (via realpath) to a PCI-BDF-named
            // directory so libdrm's drmGetDevice2() can parse the bus info —
            // otherwise Mesa/wlroots fail with "failed to retrieve device
            // information" / "Failed to get DRM device". Point it at the real
            // PCI device node under /sys/devices, which carries
            // vendor/device/config/subsystem. Both /sys/class/drm/card0 and
            // /sys/dev/char/226:N are three levels under /sys, so the same
            // relative target works from either.
            "device" => {
                let bdf = get_pci_devices()
                    .get(self.pci_index)
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| "0000:00:00.0".into());
                Ok(Arc::new(Pseudo::new(
                    &format!("../../../devices/pci0000:00/{}", bdf),
                    FileType::SymLink,
                )))
            }
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        let entries = Self::entries();
        if id >= entries.len() {
            return Err(FsError::EntryNotFound);
        }
        Ok(entries[id].into())
    }
}

/// `<pci-dev>/device/drm/` — lists this device's DRM nodes (`card0`,
/// `renderD128`). libdrm's drmGetRenderDeviceNameFromFd() scans it for the
/// `renderD*` entry to resolve the render node path.
struct SysDrmDeviceDrmDirINode {
    pci_index: usize,
}

impl INode for SysDrmDeviceDrmDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(29))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." | ".." => Ok(Arc::new(SysDrmDeviceDrmDirINode {
                pci_index: self.pci_index,
            })),
            "card0" => Ok(Arc::new(SysDrmNodeINode::card(self.pci_index))),
            "renderD128" => Ok(Arc::new(SysDrmNodeINode::render(self.pci_index))),
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        match id {
            0 => Ok("card0".into()),
            1 => Ok("renderD128".into()),
            _ => Err(FsError::EntryNotFound),
        }
    }
}

// ---------------------------------------------------------------------------
// `/sys/dev/char/<major>:<minor>` — the reverse map from a device number to its
// sysfs node. libdrm's drmGetDeviceNameFromFd2() fstat()s the card fd and reads
// `/sys/dev/char/226:0/uevent` for DEVNAME; without this it fails with ENOENT
// ("drmGetDeviceNameFromFd2() failed: No such file or directory") and wlroots
// cannot create the DRM backend. We map the relevant device numbers onto the
// existing class nodes (which already carry uevent/dev/subsystem).
// ---------------------------------------------------------------------------

struct SysDevDirINode;

impl INode for SysDevDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(160))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." | ".." => Ok(Arc::new(SysDevDirINode)),
            "char" => Ok(Arc::new(SysDevCharDirINode)),
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        if id == 0 {
            Ok("char".into())
        } else {
            Err(FsError::EntryNotFound)
        }
    }
}

struct SysDevCharDirINode;

impl INode for SysDevCharDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(161))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        if name == "." || name == ".." {
            return Ok(Arc::new(SysDevCharDirINode));
        }
        // DRM nodes: 226:0 -> card0, 226:128 -> renderD128.
        if name == "226:0" {
            if let Some(idx) = drm_card0_pci_index() {
                return Ok(Arc::new(SysDrmNodeINode::card(idx)));
            }
        }
        if name == "226:128" {
            if let Some(idx) = drm_card0_pci_index() {
                return Ok(Arc::new(SysDrmNodeINode::render(idx)));
            }
        }
        // evdev: 13:<64+N> -> a *symlink* to /sys/class/input/eventN.
        //
        // libinput's evdev_device_have_same_syspath() builds a udev device from
        // the opened fd's device number (reading this `/sys/dev/char/13:N`
        // path) and compares its canonical syspath to the syspath of the
        // enumerated device (`/sys/class/input/eventN`). If they differ it
        // closes the fd with no ioctl and rejects the device. Returning the
        // SysInputEventINode directory here gave the canonical path
        // `/sys/dev/char/13:N`, which never equals `/sys/class/input/eventN`,
        // so every input device was rejected. A symlink makes realpath() of
        // both resolve to the same `/sys/class/input/eventN`.
        if let Some(rest) = name.strip_prefix("13:") {
            if let Ok(minor) = rest.parse::<usize>() {
                if minor >= EVDEV_EVENT_MINOR_BASE {
                    let id = minor - EVDEV_EVENT_MINOR_BASE;
                    if id < input_event_count() {
                        return Ok(Arc::new(Pseudo::new(
                            &format!("../../class/input/event{}", id),
                            FileType::SymLink,
                        )));
                    }
                }
            }
        }
        Err(FsError::EntryNotFound)
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        // 226:0 (card0) and 226:128 (renderD128) first, then evdev 13:64..
        let have_card = drm_card0_pci_index().is_some();
        let base = if have_card { 2 } else { 0 };
        if have_card {
            if id == 0 {
                return Ok("226:0".into());
            }
            if id == 1 {
                return Ok("226:128".into());
            }
        }
        let ev = id - base;
        if ev < input_event_count() {
            return Ok(format!("13:{}", EVDEV_EVENT_MINOR_BASE + ev));
        }
        Err(FsError::EntryNotFound)
    }
}

// ---------------------------------------------------------------------------
// `/sys/class/input` — make evdev nodes discoverable by libinput's udev
// backend (used by wlroots/labwc). Linux's evdev nodes are major 13, with the
// event devices at minor 64+. libinput's udev backend ignores a device unless
// it carries the `ID_INPUT*` properties that udevd's `input_id` builtin would
// normally add; we synthesize those into the `uevent` file (libudev exposes
// uevent keys as device properties), so input works without a running udevd.
// ---------------------------------------------------------------------------

const EVDEV_MAJOR: usize = 13;
const EVDEV_EVENT_MINOR_BASE: usize = 64;

/// Number of input event devices (one `eventN` per registered input device),
/// matching the `/dev/input/eventN` numbering in `create_root_fs`.
fn input_event_count() -> usize {
    drivers::all_input().as_vec().len()
}

/// `ID_INPUT*` udev properties for input device `id`, derived from its evdev
/// capability bitmaps (the same classification udev's `input_id` performs).
fn input_id_props(id: usize) -> String {
    use kernel_hal::drivers::prelude::CapabilityType;
    let devs = drivers::all_input().as_vec();
    let Some(dev) = devs.get(id) else {
        return String::from("ID_INPUT=1\n");
    };
    let key = dev.capability(CapabilityType::Key);
    let rel = dev.capability(CapabilityType::RelAxis);
    let abs = dev.capability(CapabilityType::AbsAxis);

    // Linux input-event-codes: REL_X=0 REL_Y=1; BTN_LEFT=0x110 BTN_TOUCH=0x14a;
    // KEY_ESC=1 KEY_SPACE=57; ABS_X=0.
    let is_mouse = rel.contains(0) || rel.contains(1) || key.contains(0x110);
    let is_touch = abs.contains(0) && key.contains(0x14a);
    let is_keyboard = key.contains(1) && key.contains(57);

    let mut s = String::from("ID_INPUT=1\n");
    if is_keyboard {
        s.push_str("ID_INPUT_KEYBOARD=1\n");
    }
    if is_mouse {
        s.push_str("ID_INPUT_MOUSE=1\n");
    }
    if is_touch {
        s.push_str("ID_INPUT_TOUCHSCREEN=1\n");
    }
    // If nothing matched but the device has keys, mark it a key device so
    // libinput still assigns it a capability instead of ignoring it.
    if !is_keyboard && !is_mouse && !is_touch && key.contains(1) {
        s.push_str("ID_INPUT_KEY=1\n");
    }
    s
}

struct SysClassInputDirINode;

impl INode for SysClassInputDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(27))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysClassInputDirINode)),
            ".." => Ok(Arc::new(SysClassINode)),
            _ => {
                if let Some(id) = name
                    .strip_prefix("event")
                    .and_then(|n| n.parse::<usize>().ok())
                {
                    if id < input_event_count() {
                        return Ok(Arc::new(SysInputEventINode { id }));
                    }
                }
                Err(FsError::EntryNotFound)
            }
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        if id >= input_event_count() {
            return Err(FsError::EntryNotFound);
        }
        Ok(format!("event{}", id))
    }
}

struct SysInputEventINode {
    id: usize,
}

impl SysInputEventINode {
    fn entries() -> [&'static str; 3] {
        ["dev", "uevent", "subsystem"]
    }
}

impl INode for SysInputEventINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(28))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        let minor = EVDEV_EVENT_MINOR_BASE + self.id;
        match name {
            "." => Ok(Arc::new(SysInputEventINode { id: self.id })),
            ".." => Ok(Arc::new(SysClassInputDirINode)),
            "dev" => Ok(Arc::new(Pseudo::new(
                &format!("{}:{}\n", EVDEV_MAJOR, minor),
                FileType::File,
            ))),
            "uevent" => {
                let content = format!(
                    "MAJOR={}\nMINOR={}\nDEVNAME=input/event{}\n{}",
                    EVDEV_MAJOR,
                    minor,
                    self.id,
                    input_id_props(self.id),
                );
                Ok(Arc::new(Pseudo::new(&content, FileType::File)))
            }
            "subsystem" => Ok(Arc::new(Pseudo::new(
                "../../../class/input",
                FileType::SymLink,
            ))),
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        let entries = Self::entries();
        if id >= entries.len() {
            return Err(FsError::EntryNotFound);
        }
        Ok(entries[id].into())
    }
}

struct SysClassNetDirINode;

impl INode for SysClassNetDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(24))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysClassNetDirINode)),
            ".." => Ok(Arc::new(SysClassINode)),
            name => {
                if list_net_ifnames().iter().any(|n| n.as_str() == name) {
                    Ok(Arc::new(SysNetIfaceINode { name: name.into() }))
                } else {
                    Err(FsError::EntryNotFound)
                }
            }
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        let names = list_net_ifnames();
        if id >= names.len() {
            return Err(FsError::EntryNotFound);
        }
        Ok(names[id].clone())
    }
}

struct SysNetIfaceINode {
    name: String,
}

impl SysNetIfaceINode {
    fn entries() -> [&'static str; 3] {
        ["address", "operstate", "carrier"]
    }
}

impl INode for SysNetIfaceINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(25))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysNetIfaceINode {
                name: self.name.clone(),
            })),
            ".." => Ok(Arc::new(SysClassNetDirINode)),
            "operstate" => {
                let state = if self.name == "lo" || self.name == "loopback" {
                    "unknown"
                } else {
                    "up"
                };
                Ok(Arc::new(Pseudo::new(
                    &format!("{}\n", state),
                    FileType::File,
                )))
            }
            "carrier" => Ok(Arc::new(Pseudo::new("1\n", FileType::File))),
            "address" => {
                let mac = get_net_device()
                    .iter()
                    .find(|i| i.get_ifname() == self.name)
                    .map(|i| i.get_mac())
                    .unwrap_or_default();
                let bytes = mac.as_bytes();
                let content = format!(
                    "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\n",
                    bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]
                );
                Ok(Arc::new(Pseudo::new(&content, FileType::File)))
            }
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        let entries = Self::entries();
        if id >= entries.len() {
            return Err(FsError::EntryNotFound);
        }
        Ok(entries[id].into())
    }
}

struct SysClassPowerSupplyDirINode;

impl INode for SysClassPowerSupplyDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(26))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." | ".." => Ok(Arc::new(SysClassPowerSupplyDirINode)),
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, _id: usize) -> Result<String> {
        Err(FsError::EntryNotFound)
    }
}

// ---------------------------------------------------------------------------
// `/sys/class/thermal` — minimal thermal zone + cooling device interface.
//
// Models a single CPU package thermal zone (`thermal_zone0`, type
// "x86_pkg_temp") with two user-configurable trip points, plus one cooling
// device (`cooling_device0`, type "Processor"). This mirrors the ABI described
// in Documentation/driver-api/thermal/{sysfs-api,x86_pkg_temperature_thermal}.rst
// so userspace thermal tooling can probe and configure trip points / policy.
// ---------------------------------------------------------------------------

/// Static zone type reported by `thermal_zone0/type`.
const THERMAL_ZONE_TYPE: &str = "x86_pkg_temp";

/// Current CPU temperature in milli-degrees Celsius: the real digital thermal
/// sensor when the hardware exposes it (bare metal Intel), else the static
/// placeholder so VMs / unsupported parts still present a plausible value.
pub(crate) fn current_temp_mc() -> i32 {
    kernel_hal::cpu::cpu_temperature_mc().unwrap_or_else(|| THERMAL.lock().temp_mc)
}
/// Trip-point types: a passive (throttling) trip and a critical (shutdown) trip.
const THERMAL_TRIP_TYPES: [&str; 2] = ["passive", "critical"];
/// Maximum cooling-device state advertised by `cooling_device0/max_state`.
const COOLING_MAX_STATE: u32 = 10;

/// Mutable state shared by all (stateless) thermal sysfs INodes. `find()` mints
/// fresh INodes on every lookup, so the configurable values must live here for
/// writes to persist across reopen.
struct ThermalState {
    /// Current package temperature, in milli-degrees Celsius.
    temp_mc: i32,
    /// Active governor policy (`thermal_zone0/policy`).
    policy: String,
    /// Trip-point temperatures in milli-degrees Celsius; 0 disables the trip.
    trip_temp: [i32; 2],
    /// Current cooling-device state (`cooling_device0/cur_state`).
    cooling_cur: u32,
}

lazy_static! {
    static ref THERMAL: Mutex<ThermalState> = Mutex::new(ThermalState {
        temp_mc: 45000,
        policy: String::from("step_wise"),
        trip_temp: [0, 0],
        cooling_cur: 0,
    });
}

/// One writable thermal attribute. Read renders the current value; write parses
/// and stores it in [`THERMAL`].
#[derive(Clone, Copy)]
enum ThermalAttr {
    Policy,
    Trip0,
    Trip1,
    CoolingCur,
}

struct ThermalAttrINode {
    attr: ThermalAttr,
}

impl ThermalAttrINode {
    fn value(&self) -> String {
        let t = THERMAL.lock();
        match self.attr {
            ThermalAttr::Policy => format!("{}\n", t.policy),
            ThermalAttr::Trip0 => format!("{}\n", t.trip_temp[0]),
            ThermalAttr::Trip1 => format!("{}\n", t.trip_temp[1]),
            ThermalAttr::CoolingCur => format!("{}\n", t.cooling_cur),
        }
    }
}

impl INode for ThermalAttrINode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        let content = self.value();
        let bytes = content.as_bytes();
        if offset >= bytes.len() {
            return Ok(0);
        }
        let len = (bytes.len() - offset).min(buf.len());
        buf[..len].copy_from_slice(&bytes[offset..offset + len]);
        Ok(len)
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        let s = core::str::from_utf8(buf)
            .map_err(|_| FsError::InvalidParam)?
            .trim();
        let mut t = THERMAL.lock();
        match self.attr {
            ThermalAttr::Policy => t.policy = String::from(s),
            ThermalAttr::Trip0 => t.trip_temp[0] = s.parse().map_err(|_| FsError::InvalidParam)?,
            ThermalAttr::Trip1 => t.trip_temp[1] = s.parse().map_err(|_| FsError::InvalidParam)?,
            ThermalAttr::CoolingCur => {
                let v: u32 = s.parse().map_err(|_| FsError::InvalidParam)?;
                t.cooling_cur = v.min(COOLING_MAX_STATE);
            }
        }
        // Report the whole buffer consumed so the writer doesn't loop.
        Ok(buf.len())
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: true,
            error: false,
        })
    }

    fn metadata(&self) -> Result<Metadata> {
        Ok(Metadata {
            dev: 0,
            inode: 0,
            size: self.value().len(),
            blk_size: 0,
            blocks: 0,
            atime: Timespec { sec: 0, nsec: 0 },
            mtime: Timespec { sec: 0, nsec: 0 },
            ctime: Timespec { sec: 0, nsec: 0 },
            type_: FileType::File,
            mode: 0o644,
            nlinks: 1,
            uid: 0,
            gid: 0,
            rdev: 0,
        })
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
}

/// Read-only static file helper for thermal attributes.
fn thermal_ro(content: &str) -> Arc<dyn INode> {
    Arc::new(Pseudo::new(content, FileType::File))
}

struct SysClassThermalDirINode;

impl SysClassThermalDirINode {
    fn entries() -> [&'static str; 2] {
        ["thermal_zone0", "cooling_device0"]
    }
}

impl INode for SysClassThermalDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(40))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysClassThermalDirINode)),
            ".." => Ok(Arc::new(SysClassINode)),
            "thermal_zone0" => Ok(Arc::new(SysThermalZoneDirINode)),
            "cooling_device0" => Ok(Arc::new(SysThermalCoolingDirINode)),
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        let entries = Self::entries();
        if id >= entries.len() {
            return Err(FsError::EntryNotFound);
        }
        Ok(entries[id].into())
    }
}

struct SysThermalZoneDirINode;

impl SysThermalZoneDirINode {
    fn entries() -> [&'static str; 10] {
        [
            "type",
            "temp",
            "policy",
            "available_policies",
            "mode",
            "trip_point_0_temp",
            "trip_point_0_type",
            "trip_point_1_temp",
            "trip_point_1_type",
            "uevent",
        ]
    }
}

impl INode for SysThermalZoneDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(41))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysThermalZoneDirINode)),
            ".." => Ok(Arc::new(SysClassThermalDirINode)),
            "type" => Ok(thermal_ro(&format!("{}\n", THERMAL_ZONE_TYPE))),
            "temp" => Ok(thermal_ro(&format!("{}\n", current_temp_mc()))),
            "policy" => Ok(Arc::new(ThermalAttrINode {
                attr: ThermalAttr::Policy,
            })),
            "available_policies" => Ok(thermal_ro("step_wise user_space\n")),
            "mode" => Ok(thermal_ro("enabled\n")),
            "trip_point_0_temp" => Ok(Arc::new(ThermalAttrINode {
                attr: ThermalAttr::Trip0,
            })),
            "trip_point_0_type" => Ok(thermal_ro(&format!("{}\n", THERMAL_TRIP_TYPES[0]))),
            "trip_point_1_temp" => Ok(Arc::new(ThermalAttrINode {
                attr: ThermalAttr::Trip1,
            })),
            "trip_point_1_type" => Ok(thermal_ro(&format!("{}\n", THERMAL_TRIP_TYPES[1]))),
            "uevent" => Ok(thermal_ro("")),
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        let entries = Self::entries();
        if id >= entries.len() {
            return Err(FsError::EntryNotFound);
        }
        Ok(entries[id].into())
    }
}

struct SysThermalCoolingDirINode;

impl SysThermalCoolingDirINode {
    fn entries() -> [&'static str; 4] {
        ["type", "max_state", "cur_state", "uevent"]
    }
}

impl INode for SysThermalCoolingDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(42))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysThermalCoolingDirINode)),
            ".." => Ok(Arc::new(SysClassThermalDirINode)),
            "type" => Ok(thermal_ro("Processor\n")),
            "max_state" => Ok(thermal_ro(&format!("{}\n", COOLING_MAX_STATE))),
            "cur_state" => Ok(Arc::new(ThermalAttrINode {
                attr: ThermalAttr::CoolingCur,
            })),
            "uevent" => Ok(thermal_ro("")),
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        let entries = Self::entries();
        if id >= entries.len() {
            return Err(FsError::EntryNotFound);
        }
        Ok(entries[id].into())
    }
}

// ---------------------------------------------------------------------------
// System power management: `/sys/power` (system sleep) and
// `/sys/devices/system/cpu` (CPU hotplug).
//
// These mirror the userspace ABI described in
// Documentation/{driver-api/pm,power} and the suspend/CPU-hotplug docs: a
// thermal/power manager writes "mem"/"disk" to /sys/power/state and toggles
// CPUs via /sys/devices/system/cpu/cpuN/online. eclipse does not actually
// enter ACPI sleep states or park CPUs, so writes are validated and recorded
// (a compatibility shim) rather than driving real hardware transitions.
// ---------------------------------------------------------------------------

/// System sleep states advertised by `/sys/power/state`.
const POWER_STATES: &str = "freeze mem disk";

/// Mutable PM state shared by the (stateless) sysfs INodes.
struct PmState {
    /// Per-CPU online bitmask (bit N set ⇒ CPU N online). Boot CPU stays online.
    cpu_online: u64,
    /// Hibernation mode reported by `/sys/power/disk`.
    disk_mode: String,
}

lazy_static! {
    static ref PM: Mutex<PmState> = Mutex::new(PmState {
        cpu_online: u64::MAX,
        disk_mode: String::from("platform"),
    });
}

/// Comma-separated list of currently-online CPUs (e.g. "0,1,3").
fn online_cpu_list(count: usize) -> String {
    let mask = PM.lock().cpu_online;
    let mut parts: Vec<String> = Vec::new();
    for i in 0..count.min(64) {
        if mask & (1u64 << i) != 0 {
            parts.push(format!("{}", i));
        }
    }
    format!("{}\n", parts.join(","))
}

/// `0` or `0-(count-1)` range string used by present/possible CPU masks.
fn cpu_range(count: usize) -> String {
    if count <= 1 {
        String::from("0\n")
    } else {
        format!("0-{}\n", count - 1)
    }
}

/// A writable power-management sysfs attribute.
#[derive(Clone, Copy)]
enum PmAttr {
    /// `/sys/power/state`.
    PowerState,
    /// `/sys/power/disk`.
    PowerDisk,
    /// `/sys/devices/system/cpu/cpuN/online` for the given CPU index.
    CpuOnline(usize),
}

struct PmAttrINode {
    attr: PmAttr,
}

impl PmAttrINode {
    fn value(&self) -> String {
        match self.attr {
            PmAttr::PowerState => format!("{}\n", POWER_STATES),
            PmAttr::PowerDisk => format!("{}\n", PM.lock().disk_mode),
            PmAttr::CpuOnline(i) => {
                let online = PM.lock().cpu_online & (1u64 << (i.min(63))) != 0;
                format!("{}\n", if online { 1 } else { 0 })
            }
        }
    }
}

impl INode for PmAttrINode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        let content = self.value();
        let bytes = content.as_bytes();
        if offset >= bytes.len() {
            return Ok(0);
        }
        let len = (bytes.len() - offset).min(buf.len());
        buf[..len].copy_from_slice(&bytes[offset..offset + len]);
        Ok(len)
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> Result<usize> {
        let s = core::str::from_utf8(buf)
            .map_err(|_| FsError::InvalidParam)?
            .trim();
        match self.attr {
            PmAttr::PowerState => {
                // Validate against the advertised states; we don't actually
                // suspend, so a successful write is a logged no-op.
                if POWER_STATES.split_whitespace().any(|st| st == s) || s == "standby" {
                    warn!(
                        "/sys/power/state: '{}' requested (suspend not implemented)",
                        s
                    );
                } else {
                    return Err(FsError::InvalidParam);
                }
            }
            PmAttr::PowerDisk => match s {
                "platform" | "shutdown" | "reboot" | "suspend" => {
                    PM.lock().disk_mode = String::from(s)
                }
                _ => return Err(FsError::InvalidParam),
            },
            PmAttr::CpuOnline(i) => {
                let on: u32 = s.parse().map_err(|_| FsError::InvalidParam)?;
                if i == 0 && on == 0 {
                    // The boot CPU cannot be taken offline.
                    return Err(FsError::NotSupported);
                }
                if i >= 64 {
                    return Err(FsError::InvalidParam);
                }
                let mut pm = PM.lock();
                if on != 0 {
                    pm.cpu_online |= 1u64 << i;
                } else {
                    pm.cpu_online &= !(1u64 << i);
                }
            }
        }
        Ok(buf.len())
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: true,
            error: false,
        })
    }

    fn metadata(&self) -> Result<Metadata> {
        Ok(Metadata {
            dev: 0,
            inode: 0,
            size: self.value().len(),
            blk_size: 0,
            blocks: 0,
            atime: Timespec { sec: 0, nsec: 0 },
            mtime: Timespec { sec: 0, nsec: 0 },
            ctime: Timespec { sec: 0, nsec: 0 },
            type_: FileType::File,
            mode: 0o644,
            nlinks: 1,
            uid: 0,
            gid: 0,
            rdev: 0,
        })
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }

    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
}

struct SysPowerDirINode;

impl SysPowerDirINode {
    fn entries() -> [&'static str; 3] {
        ["state", "disk", "wakeup_count"]
    }
}

impl INode for SysPowerDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(140))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysPowerDirINode)),
            ".." => Ok(SYS_ROOT.clone()),
            "state" => Ok(Arc::new(PmAttrINode {
                attr: PmAttr::PowerState,
            })),
            "disk" => Ok(Arc::new(PmAttrINode {
                attr: PmAttr::PowerDisk,
            })),
            "wakeup_count" => Ok(Arc::new(Pseudo::new("0\n", FileType::File))),
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        let entries = Self::entries();
        if id >= entries.len() {
            return Err(FsError::EntryNotFound);
        }
        Ok(entries[id].into())
    }
}

struct SysDevicesSystemCpuDirINode;

impl INode for SysDevicesSystemCpuDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(150))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        let count = kernel_hal::cpu::cpu_count() as usize;
        match name {
            "." => Ok(Arc::new(SysDevicesSystemCpuDirINode)),
            ".." => Ok(Arc::new(SysDevicesSystemDirINode)),
            "online" => Ok(Arc::new(Pseudo::new(
                &online_cpu_list(count),
                FileType::File,
            ))),
            "present" | "possible" => Ok(Arc::new(Pseudo::new(&cpu_range(count), FileType::File))),
            "kernel_max" => Ok(Arc::new(Pseudo::new(
                &format!("{}\n", count.saturating_sub(1)),
                FileType::File,
            ))),
            _ => {
                // cpuN directories.
                if let Some(idx) = name
                    .strip_prefix("cpu")
                    .and_then(|n| n.parse::<usize>().ok())
                {
                    if idx < count {
                        return Ok(Arc::new(SysCpuNDirINode { cpu: idx }));
                    }
                }
                Err(FsError::EntryNotFound)
            }
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        let count = kernel_hal::cpu::cpu_count() as usize;
        // Aggregate files first, then one entry per CPU.
        const AGG: [&str; 4] = ["online", "present", "possible", "kernel_max"];
        if id < AGG.len() {
            return Ok(AGG[id].into());
        }
        let cpu_idx = id - AGG.len();
        if cpu_idx < count {
            return Ok(format!("cpu{}", cpu_idx));
        }
        Err(FsError::EntryNotFound)
    }
}

struct SysCpuNDirINode {
    cpu: usize,
}

impl INode for SysCpuNDirINode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> Result<usize> {
        Err(FsError::NotSupported)
    }
    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            read: true,
            write: false,
            error: false,
        })
    }
    fn metadata(&self) -> Result<Metadata> {
        Ok(dir_metadata(151))
    }
    fn as_any_ref(&self) -> &dyn Any {
        self
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(SysFS)
    }
    fn find(&self, name: &str) -> Result<Arc<dyn INode>> {
        match name {
            "." => Ok(Arc::new(SysCpuNDirINode { cpu: self.cpu })),
            ".." => Ok(Arc::new(SysDevicesSystemCpuDirINode)),
            "online" => Ok(Arc::new(PmAttrINode {
                attr: PmAttr::CpuOnline(self.cpu),
            })),
            _ => Err(FsError::EntryNotFound),
        }
    }
    fn get_entry(&self, id: usize) -> Result<String> {
        // The boot CPU has no `online` toggle in Linux, but exposing it for all
        // CPUs keeps the shim uniform and simple.
        if id == 0 {
            Ok("online".into())
        } else {
            Err(FsError::EntryNotFound)
        }
    }
}

/// PCI device list for `/sys`, scanned once and cached.
///
/// `scan_bus` probes every (bus, device, function) with config-space port I/O.
/// Every trap is cheap on real hardware but murderous under QEMU's TCG (each
/// `in`/`out` exits to the emulator), and this was re-run on **every** `/sys`
/// path-component resolution — resolving one `card0/device` symlink triggered
/// two full bus scans, and libdrm walks these paths dozens of times while
/// enumerating the GPU. That was the whole multi-second-per-`readlink` sysfs
/// stall (`readlink` 3.3s, `stat` 1.9s, `open` 0.6s) that kept the GL path from
/// ever finishing GPU init — so the compositor never rendered. PCI topology is
/// fixed after boot, so a scan-once cache is correct and collapses each of those
/// syscalls to a small `Vec` clone.
fn get_pci_devices() -> Vec<PciDevInfo> {
    PCI_DEVICES.clone()
}

lazy_static! {
    static ref PCI_DEVICES: Vec<PciDevInfo> = scan_pci_devices();
}

fn scan_pci_devices() -> Vec<PciDevInfo> {
    #[cfg(any(target_arch = "x86_64", target_arch = "riscv64"))]
    {
        let mut devs = Vec::new();
        let ops = &zcore_drivers::bus::pci::PortOpsImpl;
        let am = zcore_drivers::bus::pci::PCI_ACCESS;
        let pci_iter = unsafe { pci::scan_bus(ops, am) };
        for dev in pci_iter {
            let name = format!(
                "0000:{:02x}:{:02x}.{:x}",
                dev.loc.bus, dev.loc.device, dev.loc.function
            );
            let vendor = format!("{:#06x}", dev.id.vendor_id);
            let device = format!("{:#06x}", dev.id.device_id);
            let class = format!(
                "0x{:02x}{:02x}{:02x}",
                dev.id.class, dev.id.subclass, dev.id.prog_if
            );
            devs.push(PciDevInfo {
                name,
                vendor,
                device,
                class,
            });
        }
        devs
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "riscv64")))]
    {
        Vec::new()
    }
}

/// Resolve an absolute `/sys/...` path without walking the ext2 backing store.
pub(crate) fn lookup_path(path: &str, follow_times: usize) -> Result<Arc<dyn INode>> {
    let path = path.trim_end_matches('/');
    if path == "/sys" {
        return Ok(SYS_ROOT.clone());
    }
    let rest = path.strip_prefix("/sys/").ok_or(FsError::EntryNotFound)?;
    if rest.is_empty() {
        return Ok(SYS_ROOT.clone());
    }
    SYS_ROOT.lookup_follow(rest, follow_times)
}

lazy_static! {
    static ref SYS_ROOT: Arc<dyn INode> = Arc::new(SysRootINode);
}
