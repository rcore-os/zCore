#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct ContextData {
    // pg base register
    pub cr3: usize,
    // callee saved registers
    pub r15: usize,
    pub r14: usize,
    pub r13: usize,
    pub r12: usize,
    pub rbp: usize,
    pub rbx: usize,
    // pc
    pub rip: usize,
}

impl ContextData {
    pub fn new(rip: usize, _sp: usize, cr3: usize) -> Self {
        Self {
            rip,
            cr3,
            ..ContextData::default()
        }
    }
}
