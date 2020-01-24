//! Objects for IPC.

mod channel;
mod fifo;
mod socket;

pub use self::{channel::*, fifo::*, socket::*};
