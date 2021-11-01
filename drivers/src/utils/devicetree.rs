use core::ops::Range;

use device_tree::{DeviceTree as DeviceTreeInner, PropError};

use crate::{DeviceError, DeviceResult, PhysAddr, VirtAddr};

pub use device_tree::{util::StringList, Node};

/// A wrapper structure of `device_tree::DeviceTree`.
pub struct Devicetree(DeviceTreeInner);

/// Some properties inherited from ancestor nodes.
///
/// About the notion: cell, see <https://elinux.org/Device_Tree_Usage#How_Addressing_Works>.
#[derive(Clone, Copy, Debug, Default)]
pub struct InheritProps {
    /// The `#address-cells` property of its parent node.
    pub parent_address_cells: u32,
    /// The `#size-cells` property of its parent node.
    pub parent_size_cells: u32,
    /// The `interrupt-parent` property of the node. If don't have, inherit from
    /// its parent node.
    pub interrupt_parent: u32,
}

impl Devicetree {
    /// Load the device tree blob from the given virtual address.
    pub fn from(dtb_base_vaddr: VirtAddr) -> DeviceResult<Self> {
        match unsafe { DeviceTreeInner::load_from_raw_pointer(dtb_base_vaddr as *const _) } {
            Ok(dt) => Ok(Self(dt)),
            Err(err) => {
                warn!(
                    "device-tree: failed to load DTB @ {:#x}: {:?}",
                    dtb_base_vaddr, err
                );
                Err(DeviceError::InvalidParam)
            }
        }
    }

    fn walk_inner<F>(&self, node: &Node, props: InheritProps, device_node_op: &mut F)
    where
        F: FnMut(&Node, &StringList, &InheritProps),
    {
        let mut props = props;
        if let Ok(num) = node.prop_u32("interrupt-parent") {
            props.interrupt_parent = num;
        }
        if let Ok(comp) = node.prop_str_list("compatible") {
            device_node_op(node, &comp, &props);
        }

        props.parent_address_cells = node.prop_u32("#address-cells").unwrap_or(0);
        props.parent_size_cells = node.prop_u32("#size-cells").unwrap_or(0);

        // DFS
        for child in node.children.iter() {
            self.walk_inner(child, props, device_node_op);
        }
    }

    /// Traverse the tree from root by DFS, collect necessary properties, and
    /// apply the `device_node_op` to each node.
    pub fn walk<F>(&self, device_node_op: &mut F)
    where
        F: FnMut(&Node, &StringList, &InheritProps),
    {
        self.walk_inner(&self.0.root, InheritProps::default(), device_node_op)
    }

    /// Returns the `bootargs` property in the `/chosen` node, as the kernel
    /// command line.
    pub fn bootargs(&self) -> Option<&str> {
        self.0.find("/chosen")?.prop_str("bootargs").ok()
    }

    /// Returns the `linux,initrd-start` and `linux,initrd-end` properties in
    /// the `/chosen` node, as the init RAM disk address region.
    pub fn initrd_region(&self) -> Option<Range<PhysAddr>> {
        let chosen = self.0.find("/chosen")?;
        let start = chosen.prop_u32("linux,initrd-start").ok()? as _;
        let end = chosen.prop_u32("linux,initrd-end").ok()? as _;
        Some(start..end)
    }
}

impl From<PropError> for DeviceError {
    fn from(_err: PropError) -> Self {
        Self::InvalidParam
    }
}
