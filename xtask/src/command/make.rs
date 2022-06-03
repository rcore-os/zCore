use super::{ext, CommandExt};
use std::process::Command;

pub(crate) struct Make(Command);

ext!(Make);

impl Make {
    pub fn new(j: Option<usize>) -> Self {
        let mut make = Self(Command::new("make"));
        match j {
            Some(0) => {}
            Some(j) => {
                make.arg(format!("-j{j}"));
            }
            None => {
                make.arg("-j");
            }
        }
        make
    }
}
