use crate::error::*;
use crate::types::*;
use crate::control;
use crate::user::UserProcess;
use crate::kt;
use trapframe::UserContext;

pub fn zcore_main() -> ! {
    println!("Initializing zCore services.");
    crate::benchmark::run_benchmarks(core::u64::MAX);
    //force_stack_overflow();
    /*
    for i in 0..1000 {
        control::sleep(1000000);
    }
    println!("Slept 1 ms for 1000 times.");
    */
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