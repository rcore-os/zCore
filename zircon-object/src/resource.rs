use crate::object::*;
use alloc::string::String;
use alloc::sync::Arc;

pub struct Resource {
    base: KObjectBase,
    name: String,
    kind: u32,
}

impl_kobject!(Resource);

impl Resource {
    pub fn create(name: &str, kind: u32) -> ZxResult<Arc<Self>> {
        Ok(Arc::new(Resource {
            base: KObjectBase::new(),
            name: String::from(name),
            kind: kind,
        }))
    }

    pub fn validate(&self, kind: u32) -> ZxResult<()> {
        return if self.kind == kind {
            Ok(())
        } else {
            Err(ZxError::WRONG_TYPE)
        };
    }
}
