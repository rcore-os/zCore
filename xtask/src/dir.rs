use std::path::{Path, PathBuf};

/// 删除指定路径。
///
/// 如果返回 `Ok(())`，`path` 将不存在。
pub fn rm(path: &(impl AsRef<Path> + ?Sized)) -> std::io::Result<()> {
    let path = path.as_ref();
    if !path.exists() {
        Ok(())
    } else if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// 清理 `path` 目录。
///
/// 如果返回 `Ok(())`，`path` 将是一个存在的空目录。
pub fn clear(path: &(impl AsRef<Path> + ?Sized)) -> std::io::Result<()> {
    rm(path)?;
    std::fs::create_dir_all(path)
}

/// 在 `dir` 目录中根据文件名前半部分 `prefix` 找到对应文件。
///
/// 不会递归查找。
pub fn detect(path: &(impl AsRef<Path> + ?Sized), prefix: &str) -> Option<PathBuf> {
    path.as_ref()
        .read_dir()
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map_or(false, |ty| ty.is_file()))
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|s| s.to_str())
                .map_or(false, |s| s.starts_with(prefix))
        })
}
