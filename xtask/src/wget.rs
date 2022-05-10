use crate::dir;
use std::{fs, path::Path, process::Command};

pub fn wget(url: &str, dst: &(impl AsRef<Path> + ?Sized)) {
    let dst = dst.as_ref();
    if dst.exists() {
        return;
    }

    let temp: usize = rand::random();
    let temp_name = format!("/tmp/{temp}");
    let temp_name = Path::new(&temp_name);
    let res = Command::new("wget")
        .arg(url)
        .arg("-O")
        .arg(temp_name)
        .status()
        .unwrap();
    if res.success() {
        fs::rename(temp_name, dst).unwrap();
    } else {
        dir::rm(dst).unwrap();
        panic!("FAILED: wget {url}");
    }
}
