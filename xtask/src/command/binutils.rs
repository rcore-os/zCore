use super::{ext, Cargo, CommandExt};
use std::{ffi::OsStr, process::Command};

ext!(def; BinUtil);

impl BinUtil {
    fn new(which: impl AsRef<OsStr>) -> Self {
        let which = which.as_ref();
        let check = std::str::from_utf8(&Cargo::install().arg("--list").output().stdout)
            .unwrap()
            .lines()
            .any(|line| OsStr::new(line) == which);
        if !check {
            Cargo::install().arg("cargo-binutils").invoke();
        }
        Self(Command::new(which))
    }

    pub fn objcopy() -> Self {
        Self::new("rust-objcopy")
    }

    pub fn objdump() -> Self {
        Self::new("rust-objdump")
    }
}
