cfg_if! {
    if #[cfg(any(feature = "board-fu740", feature = "board-c910light"))] {
mod entry64;
pub use entry64::consts;
    } else {
mod boot_page_table;
mod entry;
pub mod consts;
    }
}
