use super::rights::Rights;
use super::*;
use alloc::sync::Arc;

pub struct Handle {
    object: Arc<dyn KernelObject>,
    rights: Rights,
}
