use super::CommandExt;
use std::{ffi::OsStr, process::Command};

pub(crate) struct Tar(Command);

impl AsRef<Command> for Tar {
    fn as_ref(&self) -> &Command {
        &self.0
    }
}

impl AsMut<Command> for Tar {
    fn as_mut(&mut self) -> &mut Command {
        &mut self.0
    }
}

impl CommandExt for Tar {}

impl Tar {
    pub fn xf(src: &impl AsRef<OsStr>, dst: Option<impl AsRef<OsStr>>) -> Self {
        let mut cmd = Command::new("tar");
        cmd.arg("xf").arg(src);
        if let Some(dst) = dst {
            cmd.arg("-C").arg(dst);
        }
        Self(cmd)
    }
}
