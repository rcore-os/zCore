use super::{ext, CommandExt};
use std::process::Command;

ext!(def; Make);

impl Make {
    pub fn new() -> Self {
        Self(Command::new("make"))
    }

    pub fn install() -> Self {
        let mut make = Self::new();
        make.arg("install");
        make
    }

    pub fn j(&mut self, j: usize) -> &mut Self {
        match j {
            usize::MAX => self.arg("-j"),
            j => self.arg(format!("-j{j}")),
        }
    }
}
