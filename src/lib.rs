#![no_std]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate bitflags;

mod error;
pub mod io;
pub mod ipc;
pub mod memory;
pub mod object;
pub mod task;

pub use self::error::*;


#[cfg(test)]
mod tests {
    #[test]
    fn is_work() {
        use crate::ipc::channel::create;
        use crate::object::handle::Handle;
        use crate::ipc::channel::Channel;
        use crate::error::*;
        let (handle0, handle1) = create();
        handle0.do_mut(|ch: &mut Channel|{
            assert_eq!(0u64, ch.id());
            ZxError::OK
        });
        handle1.do_mut(|ch: &mut Channel|{
            assert_eq!(1u64, ch.id());
            ZxError::OK
        });
    }
}
