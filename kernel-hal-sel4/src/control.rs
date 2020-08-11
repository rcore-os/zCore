use crate::types::*;
use crate::error::*;
use crate::kipc::KipcChannel;
use lazy_static::lazy_static;
use crate::kt::KernelThread;
use alloc::boxed::Box;

lazy_static! {
    static ref CONTROL: KipcChannel<ControlMessage> = KipcChannel::new().expect("kipc/CONTROL: init failed");
}

enum ControlMessage {
    ExitThread(KernelThread),
}

pub fn run() -> ! {
    loop {
        let (msg, reply) = CONTROL.recv();
        //println!("Got control message");
        match msg {
            ControlMessage::ExitThread(t) => {
                unsafe {
                    t.drop_from_control_thread();
                }

                // No need to reply to an exited thread
                reply.forget();
            }
        }
    }
}

pub fn exit_thread(kt: KernelThread) -> ! {
    drop(CONTROL.call(ControlMessage::ExitThread(kt)));
    unreachable!("exit_thread");
}