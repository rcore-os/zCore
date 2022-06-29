use crate::Arch;
use command_ext::ext;
use std::{ffi::OsStr, path::Path, process::Command};

macro_rules! fetch_online {
    ($dst:expr, $f:expr) => {{
        use command_ext::{dir, CommandExt};
        use std::{fs, path::PathBuf};

        dir::rm(&$dst).unwrap();
        let tmp: usize = rand::random();
        let tmp = PathBuf::from("/tmp").join(tmp.to_string());
        let mut ext = $f(tmp.clone());
        let status = ext.status();
        if status.success() {
            dir::create_parent(&$dst).unwrap();
            if tmp.is_dir() {
                dircpy::copy_dir(&tmp, &$dst).unwrap();
            } else {
                fs::copy(&tmp, &$dst).unwrap();
            }
            dir::rm(tmp).unwrap();
        } else {
            dir::rm(tmp).unwrap();
            panic!(
                "Failed with code {} from {:?}",
                status.code().unwrap(),
                ext.info()
            );
        }
    }};
}

pub(crate) use fetch_online;

pub(crate) fn wget(url: impl AsRef<OsStr>, dst: impl AsRef<Path>) {
    use command_ext::Ext;

    let dst = dst.as_ref();
    if dst.exists() {
        println!("{dst:?} already exist. You can delete it manually to re-download.");
        return;
    }

    fetch_online!(dst, |tmp| {
        let mut wget = Ext::new("wget");
        wget.arg(&url).arg("-O").arg(tmp);
        wget
    });
}

// pub(crate) fn git_clone(repo: impl AsRef<OsStr>, dst: impl AsRef<Path>, pull: bool) {
//     let dst = dst.as_ref();
//     if dst.is_dir() {
//         if pull {
//             let _ = Git::pull().current_dir(dst).status();
//         } else {
//             println!("{dst:?} already exist. You can delete it manually to re-clone.");
//         }
//         return;
//     }

//     fetch_online!(dst, |tmp| Git::clone(repo, Some(tmp)));
// }

ext!(def; Qemu);

impl Qemu {
    pub(crate) fn img() -> Self {
        Self(Command::new("qemu-img"))
    }

    pub(crate) fn system(arch: Arch) -> Self {
        Self(Command::new(format!("qemu-system-{}", arch.name())))
    }
}
