use alloc::string::String;
use core::slice;

use device_tree::{DeviceTree, Node};

//use super::virtio_mmio::virtio_probe;
use super::virtio::virtio_probe;

const DEVICE_TREE_MAGIC: u32 = 0xd00dfeed;

fn walk_dt_node(dt: &Node, cmdline_out: &mut String) {
    if let Ok(compatible) = dt.prop_str("compatible") {
        // TODO: query this from table
        if compatible == "virtio,mmio" {
            virtio_probe(dt);
        }
        // TODO: initial other devices (16650, etc.)
    }
    if let Ok(bootargs) = dt.prop_str("bootargs") {
        if !bootargs.is_empty() {
            *cmdline_out = String::from(bootargs);
        }
    }
    for child in dt.children.iter() {
        walk_dt_node(child, cmdline_out);
    }
}

struct DtbHeader {
    magic: u32,
    size: u32,
}

/// Return cmdline.
pub fn init(dtb: usize) -> String {
    info!("DTB: {:#x}", dtb);
    let header = unsafe { &*(dtb as *const DtbHeader) };
    let magic = u32::from_be(header.magic);
    assert_eq!(magic, DEVICE_TREE_MAGIC, "invalid device tree magic number");
    let size = u32::from_be(header.size);
    let dtb_data = unsafe { slice::from_raw_parts(dtb as *const u8, size as usize) };
    let mut cmdline = String::new();
    if let Ok(dt) = DeviceTree::load(dtb_data) {
        //trace!("DTB: {:#x?}", dt);
        walk_dt_node(&dt.root, &mut cmdline);
    }
    cmdline
}
