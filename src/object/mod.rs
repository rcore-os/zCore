pub mod handle;
pub mod rights;

pub trait KernelObject {
    fn id(&self) -> KoID;
}

pub type KoID = u64;
