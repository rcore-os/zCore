use crate::{dir, git::Git, CommandExt};
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
        fs::rename(&tmp, dst).unwrap();
    } else {
        dir::rm(&tmp).unwrap();
        panic!(
            "Failed with code {}: wget {:?}",
            status.code().unwrap(),
            url.as_ref()
        );
    }
}

pub(crate) fn git_clone(repo: impl AsRef<OsStr>, dst: impl AsRef<Path>) {
    let dst = dst.as_ref();
    if dst.is_dir() {
        Git::pull().current_dir(dst).join();
        return;
    }

    let tmp: usize = rand::random();
    let tmp = format!("/tmp/{tmp}");
    let mut git = Git::clone(repo, Some(&tmp));
    let status = git.status();
    if status.success() {
        dir::clear(dst).unwrap();
        fs::rename(&tmp, dst).unwrap();
    } else {
        dir::rm(&tmp).unwrap();
        panic!(
            "Failed with code {}: {:?}",
            status.code().unwrap(),
            git.info()
        );
    }
}
