//! 操作 git。

use std::{ffi::OsStr, process::Command};

fn git(sub: &(impl AsRef<OsStr> + ?Sized)) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg(sub);
    cmd
}

/// git lfs ...
pub fn lfs() -> Command {
    git("lfs")
}

/// git config [--global] ...
pub fn config(global: bool) -> Command {
    let mut cmd = git("config");
    if global {
        cmd.arg("--global");
    };
    cmd
}

/// git clone [dir] ...
pub fn clone(
    repo: &(impl AsRef<OsStr> + ?Sized),
    dir: Option<&(impl AsRef<OsStr> + ?Sized)>,
) -> Command {
    let mut cmd = git("clone");
    cmd.arg(repo);
    if let Some(dir) = dir {
        cmd.arg(dir);
    }
    cmd
}

/// git pull ...
pub fn pull() -> Command {
    git("pull")
}

/// git submodule update --init.
pub fn submodule_update(init: bool) -> Command {
    let mut cmd = git("submodule");
    cmd.arg("update");
    if init {
        cmd.arg("--init");
    }
    cmd
}
