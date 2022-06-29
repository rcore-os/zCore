//! Package of [`device_tree`].

use crate::{DeviceError, DeviceResult, PhysAddr, VirtAddr};
use alloc::vec::Vec;
use core::ops::Range;
use device_tree::{DeviceTree as DeviceTreeInner, PropError};

pub use device_tree::{util::StringList, Node};

/// A unified representation of the `interrupts` and `interrupts_extended`
/// properties for any interrupt generating device.
pub type InterruptsProp = Vec<u32>;

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
}

/// Combine `cell_num` of 32-bit integers from `cells` into a 64-bit integer.
fn from_cells(cells: &[u32], cell_num: u32) -> DeviceResult<u64> {
    if cell_num as usize > cells.len() {
        return Err(DeviceError::InvalidParam);
    }
    let mut value = 0;
    for &c in &cells[..cell_num as usize] {
        value = value << 32 | c as u64;
    }
    Ok(value)
}

/// Parse the `reg` property, about `reg`: <https://elinux.org/Device_Tree_Usage#How_Addressing_Works>.
pub fn parse_reg(node: &Node, props: &InheritProps) -> DeviceResult<(u64, u64)> {
    let cells = node.prop_cells("reg")?;
    let addr = from_cells(&cells, props.parent_address_cells)?;
    let size = from_cells(
        &cells[props.parent_address_cells as usize..],
        props.parent_size_cells,
    )?;
    Ok((addr, size))
}

/// Returns a `Vec<u32>` according to the `interrupts` or `interrupts-extended`
/// property, the first element is the interrupt parent.
pub fn parse_interrupts(node: &Node, props: &InheritProps) -> DeviceResult<InterruptsProp> {
    if node.has_prop("interrupts-extended") {
        Ok(node.prop_cells("interrupts-extended")?)
    } else if node.has_prop("interrupts") && props.interrupt_parent > 0 {
        let mut ret = node.prop_cells("interrupts")?;
        ret.insert(0, props.interrupt_parent);
        Ok(ret)
    } else {
        Ok(Vec::new())
    }
}

impl From<PropError> for DeviceError {
    fn from(_err: PropError) -> Self {
        Self::InvalidParam
    }
}
