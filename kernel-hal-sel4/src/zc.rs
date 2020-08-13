use crate::error::*;
use crate::types::*;
use crate::control;
use crate::user::UserProcess;
use crate::kt;
use trapframe::UserContext;

extern "C" {
    fn zircon_start();
}

pub fn zcore_main() -> ! {
    println!("Starting zCore services.");
    //crate::benchmark::run_benchmarks(1);
    unsafe {
        zircon_start();
    }
    control::idle();
    panic!("zircon_start returned");
    //force_stack_overflow();
    kt::spawn(first_user_thread).expect("cannot spawn user thread");
    loop {
        control::sleep(1000000 * 1000);
    }
}

/*
fn force_stack_overflow() {
    #[inline(never)]
    fn fib(n: i32) -> i32 {
        println!("{}", n);
        if n == 1 || n == 2 {
            1
        } else {
            fib(n - 1) + fib(n - 2)
        }
    }

    println!("Forcing stack overflow.");

    let result = fib(100000000);
    unsafe {
        llvm_asm!("" :: "r"(result) :: "volatile");
    }
}
*/

pub fn first_user_thread() {
    println!("end");
}