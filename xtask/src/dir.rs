//! 操作目录。

use std::{
    fs,
    path::{Path, PathBuf},
};

/// 删除指定路径。
///
/// 如果返回 `Ok(())`，`path` 将不存在。
pub fn rm(path: impl AsRef<Path>) -> std::io::Result<()> {
    let path = path.as_ref();
    if !path.exists() {
        Ok(())
    } else if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// 创建 `path` 的父目录。
pub fn create_parent(path: impl AsRef<Path>) -> std::io::Result<()> {
    match path.as_ref().parent() {
        Some(parent) => fs::create_dir_all(parent),
        None => Ok(()),
    }
}

/// 清理 `path` 目录。
///
/// 如果返回 `Ok(())`，`path` 将是一个存在的空目录。
pub fn clear(path: impl AsRef<Path>) -> std::io::Result<()> {
    rm(&path)?;
    std::fs::create_dir_all(&path)
}

/// 在 `dir` 目录中根据文件名前半部分 `prefix` 找到对应文件。
///
/// 不会递归查找。
#[allow(unused)]
pub fn detect(path: impl AsRef<Path>, prefix: impl AsRef<str>) -> Option<PathBuf> {
    path.as_ref()
        .read_dir()
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map_or(false, |ty| ty.is_file()))
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|s| s.to_str())
                .map_or(false, |s| s.starts_with(prefix.as_ref()))
        })
}
