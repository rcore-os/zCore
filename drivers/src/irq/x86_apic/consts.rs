// TODO: configurable

pub const X86_INT_BASE: usize = 0x20;

pub const X86_INT_LOCAL_APIC_BASE: usize = 0xf0;
pub const X86_INT_APIC_SPURIOUS: usize = X86_INT_LOCAL_APIC_BASE;
pub const X86_INT_APIC_TIMER: usize = X86_INT_LOCAL_APIC_BASE + 0x1;
pub const X86_INT_APIC_ERROR: usize = X86_INT_LOCAL_APIC_BASE + 0x2;
