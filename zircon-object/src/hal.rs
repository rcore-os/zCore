//! Hardware Abstraction Layer

#[cfg(not(test))]
#[no_mangle]
#[linkage = "weak"]
fn hal_thread_spawn(entry: usize, stack: usize, arg1: usize, arg2: usize) -> ThreadId {
    unimplemented!()
}

#[cfg(not(test))]
#[no_mangle]
#[linkage = "weak"]
fn hal_thread_exit(tid: ThreadId) {
    unimplemented!()
}

type ThreadId = usize;

pub struct Thread {
    id: ThreadId,
}

impl Thread {
    pub fn spawn(entry: usize, stack: usize, arg1: usize, arg2: usize) -> Self {
        let id = hal_thread_spawn(entry, stack, arg1, arg2);
        Thread { id }
    }

    pub fn exit(&mut self) {
        hal_thread_exit(self.id);
    }
}

#[cfg(test)]
use self::tests::*;

#[cfg(test)]
mod tests {
    #![allow(unsafe_code)]
    use super::ThreadId;

    pub fn hal_thread_spawn(entry: usize, stack: usize, arg1: usize, arg2: usize) -> ThreadId {
        let handle = std::thread::spawn(move || {
            unsafe {
                asm!("jmp $0" :: "r"(entry), "{rsp}"(stack), "{rdi}"(arg1), "{rsi}"(arg2) :: "volatile" "intel");
            }
            unreachable!()
        });
        0
    }

    pub fn hal_thread_exit(tid: ThreadId) {
        unimplemented!()
    }
}
