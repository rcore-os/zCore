cfg_if! {
    if #[cfg(feature = "board-fu740")] {
mod entry64;
pub use entry64::consts;
    } else {
mod boot_page_table;
mod entry;
pub mod consts;
    }
}
