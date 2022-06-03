use super::{dir, git::Git, CommandExt};
use std::{ffi::OsStr, fs, path::Path, process::Command};

pub(crate) fn wget(url: impl AsRef<OsStr>, dst: impl AsRef<Path>) {
    let dst = dst.as_ref();
    if dst.exists() {
        return;
    }

    let tmp: usize = rand::random();
    let tmp = format!("/tmp/{tmp}");
    let status = Command::new("wget")
        .arg(&url)
        .args(&["-O", &tmp])
        .status()
        .unwrap();
    if status.success() {
        dir::create_parent(&dst).unwrap();
        fs::copy(&tmp, dst).unwrap();
        dir::rm(tmp).unwrap();
    } else {
        dir::rm(tmp).unwrap();
        panic!(
            "Failed with code {}: wget {:?}",
            status.code().unwrap(),
            url.as_ref()
        );
    }
}

#[allow(unused)]
pub(crate) fn git_clone(repo: impl AsRef<OsStr>, dst: impl AsRef<Path>) {
    let dst = dst.as_ref();
    if dst.is_dir() {
        Git::pull().current_dir(dst).invoke();
        return;
    }

    let tmp: usize = rand::random();
    let tmp = format!("/tmp/{tmp}");
    let mut git = Git::clone(repo, Some(&tmp));
    let status = git.status();
    if status.success() {
        dir::create_parent(&dst).unwrap();
        dircpy::copy_dir(&tmp, dst).unwrap();
        dir::rm(tmp).unwrap();
    } else {
        dir::rm(tmp).unwrap();
        panic!(
            "Failed with code {}: {:?}",
            status.code().unwrap(),
            git.info()
        );
    }
}
