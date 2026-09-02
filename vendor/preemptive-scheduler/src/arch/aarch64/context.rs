#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct ContextData {
    // callee saved registers
    pub s: [usize; 11],
    // pc / sp
    pub lr: usize,
    pub sp: usize,
    // pg base register
    pub ttbr0: usize,
}

impl ContextData {
    pub fn new(lr: usize, sp: usize, ttbr0: usize) -> Self {
        Self {
            s: [0; 11],
            lr,
            sp,
            ttbr0,
        }
    }
}
