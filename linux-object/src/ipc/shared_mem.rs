//!
use alloc::{collections::BTreeMap, sync::Arc, sync::Weak};
use lazy_static::lazy_static;
use spin::RwLock;
use zircon_object::vm::*;

lazy_static! {
    static ref KEY2SHM: RwLock<BTreeMap<usize, Weak<spin::Mutex<Arc<VmObject>>>>> =
        RwLock::new(BTreeMap::new());
}

///
#[derive(Clone)]
pub struct ShmIdentifier {
    ///
    pub addr: usize,
    ///
    pub shared_guard: Arc<spin::Mutex<Arc<VmObject>>>,
}

impl ShmIdentifier {
    ///
    pub fn set_addr(&mut self, addr: usize) {
        self.addr = addr;
    }

    ///
    pub fn new_shared_guard(key: usize, memsize: usize) -> Arc<spin::Mutex<Arc<VmObject>>> {
        let mut key2shm = KEY2SHM.write();

        // found in the map
        if let Some(weak_guard) = key2shm.get(&key) {
            if let Some(guard) = weak_guard.upgrade() {
                return guard;
            }
        }
        let shared_guard = Arc::new(spin::Mutex::new(VmObject::new_paged(pages(memsize))));
        // insert to global map
        key2shm.insert(key, Arc::downgrade(&shared_guard));
        shared_guard
    }
}
