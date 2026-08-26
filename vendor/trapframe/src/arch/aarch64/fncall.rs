//! Switch context by function call within the same privilege level.
//!
//! # Assumption
//!
//! This module suppose you are running kernel on Linux with glibc,
//! and your user program is based on musl libc.
//!
//! Because we will store values in their pthread structure.

use super::UserContext;
use core::arch::global_asm;

global_asm!(include_str!("fncall.S"));

extern "C" {
    /// The syscall entry of function call.
    ///
    /// # Usage
    ///
    /// Replace `svc` instruction by a `bl` instruction.
    ///
    /// ```asm
    /// svc #0
    /// bl syscall_fn_entry
    /// ```
    pub fn syscall_fn_entry();

    fn syscall_fn_return(regs: &mut UserContext);
}

impl UserContext {
    /// Go to user context by function return, within the same privilege level.
    ///
    /// User program should call `syscall_fn_entry()` to return back.
    pub fn run_fncall(&mut self) {
        unsafe {
            syscall_fn_return(self);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::*;
    use core::arch::global_asm;

    // Mock user program to dump registers at stack.
    global_asm!(
        r#"
dump_registers:
    stp     x30, x0, [sp, #-16]!
    str     x29, [sp, #-16]!
    stp     x27, x28, [sp, #-16]!
    stp     x25, x26, [sp, #-16]!
    stp     x23, x24, [sp, #-16]!
    stp     x21, x22, [sp, #-16]!
    stp     x19, x20, [sp, #-16]!
    stp     x17, x18, [sp, #-16]!
    stp     x15, x16, [sp, #-16]!
    stp     x13, x14, [sp, #-16]!
    stp     x11, x12, [sp, #-16]!
    stp     x9, x10, [sp, #-16]!
    stp     x7, x8, [sp, #-16]!
    stp     x5, x6, [sp, #-16]!
    stp     x3, x4, [sp, #-16]!
    stp     x1, x2, [sp, #-16]!

    add     x0, x0, #100
    add     x1, x1, #100
    add     x2, x2, #100
    add     x3, x3, #100
    add     x4, x4, #100
    add     x5, x5, #100
    add     x6, x6, #100
    add     x7, x7, #100
    add     x8, x8, #100
    add     x9, x9, #100
    add     x10, x10, #100
    add     x11, x11, #100
    add     x12, x12, #100
    add     x13, x13, #100
    add     x14, x14, #100
    add     x15, x15, #100
    add     x16, x16, #100
    add     x17, x17, #100
    add     x18, x18, #100
    add     x19, x19, #100
    add     x20, x20, #100
    add     x21, x21, #100
    add     x22, x22, #100
    add     x23, x23, #100
    add     x24, x24, #100
    add     x25, x25, #100
    add     x26, x26, #100
    add     x27, x27, #100
    add     x28, x28, #100
    add     x29, x29, #100
    add     x30, x30, #100

    bl syscall_fn_entry

.global elr_location
elr_location:
"#
    );

    #[test]
    fn run_fncall() {
        extern "C" {
            fn dump_registers();
            fn elr_location();
        }
        let mut stack = [0u8; 0x1000];
        let general = GeneralRegs {
            x0: 0,
            x1: 1,
            x2: 2,
            x3: 3,
            x4: 4,
            x5: 5,
            x6: 6,
            x7: 7,
            x8: 8,
            x9: 9,
            x10: 10,
            x11: 11,
            x12: 12,
            x13: 13,
            x14: 14,
            x15: 15,
            x16: 16,
            x17: 17,
            x18: 18,
            x19: 19,
            x20: 20,
            x21: 21,
            x22: 22,
            x23: 23,
            x24: 24,
            x25: 25,
            x26: 26,
            x27: 27,
            x28: 28,
            x29: 29,
            x30: 30,
            ..Default::default()
        };
        let mut cx = UserContext {
            general,
            sp: stack.as_mut_ptr() as usize + 0x1000,
            elr: dump_registers as usize,
            ..Default::default()
        };
        cx.run_fncall();
        // check restored registers
        let general_dump = unsafe { *(cx.sp as *const GeneralRegs) };
        assert_eq!(
            general_dump,
            GeneralRegs {
                x30: dump_registers as usize,
                ..general
            }
        );
        // check saved registers
        assert_eq!(
            cx.general,
            GeneralRegs {
                x0: 100 + 0,
                x1: 100 + 1,
                x2: 100 + 2,
                x3: 100 + 3,
                x4: 100 + 4,
                x5: 100 + 5,
                x6: 100 + 6,
                x7: 100 + 7,
                x8: 100 + 8,
                x9: 100 + 9,
                x10: 100 + 10,
                x11: 100 + 11,
                x12: 100 + 12,
                x13: 100 + 13,
                x14: 100 + 14,
                x15: 100 + 15,
                x16: 100 + 16,
                x17: 100 + 17,
                x18: 100 + 18,
                x19: 100 + 19,
                x20: 100 + 20,
                x21: 100 + 21,
                x22: 100 + 22,
                x23: 100 + 23,
                x24: 100 + 24,
                x25: 100 + 25,
                x26: 100 + 26,
                x27: 100 + 27,
                x28: 100 + 28,
                x29: 100 + 29,
                x30: elr_location as usize,
                ..cx.general
            }
        );
        assert_eq!(cx.elr, elr_location as usize);
    }
}
