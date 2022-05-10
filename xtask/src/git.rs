//! 操作 git。

use crate::CommandExt;
use std::{ffi::OsStr, process::Command};

pub(super) struct Git {
    cmd: Command,
}

impl AsRef<Command> for Git {
    fn as_ref(&self) -> &Command {
        &self.cmd
    }
}

impl AsMut<Command> for Git {
    fn as_mut(&mut self) -> &mut Command {
        &mut self.cmd
    }
}

impl CommandExt for Git {}

impl Git {
    fn new(sub: impl AsRef<OsStr>) -> Self {
        let mut git = Self {
            cmd: Command::new("git"),
        };
        git.arg(sub);
        git
    }

    pub fn lfs() -> Self {
        Self::new("lfs")
    }

    pub fn config(global: bool) -> Self {
        let mut git = Self::new("config");
        if global {
            git.arg("--global");
        };
        git
    }

    pub fn clone(repo: impl AsRef<OsStr>, dir: Option<impl AsRef<OsStr>>) -> Self {
        let mut git = Self::new("clone");
        git.arg(repo);
        if let Some(dir) = dir {
            git.arg(dir);
        }
        git
    }

    pub fn pull() -> Self {
        Self::new("pull")
    }

    pub fn submodule_update(init: bool) -> Self {
        let mut git = Self::new("submodule");
        git.arg("update");
        if init {
            git.arg("--init");
        }
        git
    }
}
