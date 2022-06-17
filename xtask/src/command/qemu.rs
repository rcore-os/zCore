use super::ext;
use crate::Arch;
use std::process::Command;

ext!(def; Qemu);

impl Qemu {
    pub fn img() -> Self {
        Self(Command::new("qemu-img"))
    }

    pub fn system(arch: Arch) -> Self {
        Self(Command::new(format!("qemu-system-{}", arch.name())))
    }
}
