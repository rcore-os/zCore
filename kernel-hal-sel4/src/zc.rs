use crate::error::*;
use crate::types::*;
use crate::control;
use crate::user::UserProcess;
use crate::kt;
use trapframe::UserContext;

pub fn zcore_main() -> ! {
    println!("Initializing zCore services.");
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

pub fn first_user_thread() {
    println!("Entering user mode.");
    let user_proc = UserProcess::new().expect("cannot create user process");
    let mut ut = user_proc.create_thread().expect("cannot create user thread");
    for _ in 0..10000 {
        let mut uctx = UserContext::default();
        let (entry_reason, next_ut) = ut.run(&mut uctx);
        ut = next_ut;
    }
    println!("end");
}