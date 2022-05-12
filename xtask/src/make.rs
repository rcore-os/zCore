use crate::CommandExt;
use std::process::Command;

pub(crate) struct Make(Command);

impl AsRef<Command> for Make {
    fn as_ref(&self) -> &Command {
        &self.0
    }
}

impl AsMut<Command> for Make {
    fn as_mut(&mut self) -> &mut Command {
        &mut self.0
    }
}

impl CommandExt for Make {}

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
