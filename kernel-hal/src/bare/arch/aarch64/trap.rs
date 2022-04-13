#![allow(dead_code)]
#![allow(clippy::identity_op)]

use trapframe::TrapFrame;

fn breakpoint() {
    panic!("\nEXCEPTION: Breakpoint");
}

#[no_mangle]
pub extern "C" fn trap_handler(_tf: &mut TrapFrame) {
    unimplemented!()
}
