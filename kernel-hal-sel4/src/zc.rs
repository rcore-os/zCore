use crate::error::*;
use crate::types::*;
use crate::control;

pub fn zcore_main() -> ! {
    println!("Initializing zCore services.");
    for i in 0..1000 {
        control::sleep(1000000);
    }
    println!("Slept 1 ms for 1000 times.");
    loop {
        control::sleep(1000000 * 1000);
    }
}