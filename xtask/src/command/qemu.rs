use super::ext;
use crate::arch::Arch;
use std::process::Command;

pub(crate) struct Qemu(Command);

ext!(Qemu);

impl Qemu {
    pub fn img() -> Self {
        Self(Command::new("qemu-img"))
    }

    pub fn system(arch: Arch) -> Self {
        Self(Command::new(format!("qemu-system-{}", arch.as_str())))
    }
}
