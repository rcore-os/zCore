#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct ContextData {
    // pc / sp
    pub ra: usize,
    pub sp: usize,
    // callee saved registers
    pub s: [usize; 12],
    // pg base register
    pub satp: usize,
}

impl ContextData {
    pub fn new(ra: usize, sp: usize, satp: usize) -> Self {
        Self {
            ra,
            sp,
            satp,
            ..ContextData::default()
        }
    }
}
