use crate::types::*;
use crate::error::*;
use crate::object::*;
use crate::sys;
use crate::futex::FMutex;
use alloc::collections::linked_list::LinkedList;

static ASID_POOLS: FMutex<LinkedList<AsidPool>> = FMutex::new(LinkedList::new());

struct AsidPoolBacking;
unsafe impl ObjectBacking for AsidPoolBacking {
    fn bits() -> u8 {
        unsafe {
            sys::L4BRIDGE_ASID_POOL_BITS as u8
        }
    }

    unsafe fn retype(untyped: CPtr, out: CPtr) -> KernelResult<()> {
        if sys::l4bridge_make_asid_pool_ts(untyped, out) != 0 {
            Err(KernelError::RetypeFailed)
        } else {
            Ok(())
        }
    }
}

type AsidPool = Object<AsidPoolBacking>;

pub fn assign(vspace: CPtr) -> KernelResult<()> {
    let mut pools = ASID_POOLS.lock();
    for pool in pools.iter().rev() {
        if unsafe {
            sys::l4bridge_assign_asid_ts(pool.object(), vspace)
        } == 0 {
            return Ok(());
        }
    }
    let new_pool = AsidPool::new()?;
    if unsafe {
        sys::l4bridge_assign_asid_ts(new_pool.object(), vspace)
    } != 0 {
        // Since we are allocating on a new pool there shouldn't be errors now
        panic!("cannot assign asid to vspace");
    }
    pools.push_back(new_pool);
    Ok(())
}