use super::ext;
use std::{ffi::OsStr, process::Command};

pub(crate) struct Tar(Command);

ext!(Tar);

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
