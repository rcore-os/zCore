use alloc::sync::Arc;

use rvm::Guest as GuestInner;

pub struct Guest {
    _inner: Arc<GuestInner>,
}

impl Guest {
    pub fn new() -> Self {
        Guest {
            _inner: GuestInner::new().unwrap(),
        }
    }
}
