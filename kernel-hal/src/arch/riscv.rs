#[repr(C)]
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub struct GeneralRegs {}

impl GeneralRegs {
    pub fn new_fn(_entry: usize, _sp: usize, _arg1: usize, _arg2: usize) -> Self {
        unimplemented!()
    }

    pub fn clone(&self, _newsp: usize, _newtls: usize) -> Self {
        unimplemented!()
    }

    pub fn fork(&self) -> Self {
        unimplemented!()
    }
}
