use super::{dir, git::Git, CommandExt, Ext};
use std::{
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::ExitStatus,
};

pub(crate) fn wget(url: impl AsRef<OsStr>, dst: impl AsRef<Path>) {
    let dst = dst.as_ref();
    if dst.exists() {
        return;
    }

    mv_dir(dst, |tmp| {
        let mut wget = Ext::new("wget");
        let status = wget.arg(&url).arg("-O").arg(tmp).status();
        let info = wget.info();
        (info, status)
    });
}

pub(crate) fn git_clone(repo: impl AsRef<OsStr>, dst: impl AsRef<Path>) {
    let dst = dst.as_ref();
    if dst.is_dir() {
        let _ = Git::pull().current_dir(dst).status();
        return;
    }

    mv_dir(dst, |tmp| {
        let mut git = Git::clone(repo, Some(tmp));
        let status = git.status();
        let info = git.info();
        (info, status)
    });
}

/// 先下载文件到随机位置，成功后再拷贝到指定位置。
fn mv_dir(dst: impl AsRef<Path>, get: impl FnOnce(&OsStr) -> (OsString, ExitStatus)) {
    let tmp: usize = rand::random();
    let tmp = PathBuf::from("/tmp").join(tmp.to_string());
    let (info, status) = get(tmp.as_os_str());
    if status.success() {
        dir::create_parent(&dst).unwrap();
        if tmp.is_dir() {
            dircpy::copy_dir(&tmp, dst).unwrap();
        } else {
            fs::copy(&tmp, dst).unwrap();
        }
        dir::rm(tmp).unwrap();
    } else {
        dir::rm(tmp).unwrap();
        panic!("Failed with code {} from {info:?}", status.code().unwrap(),);
    }
}
