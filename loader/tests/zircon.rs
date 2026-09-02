#[cfg(target_arch = "x86_64")]
#[async_std::test]
async fn userboot() {
    kernel_hal::init();
    let zbi = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../prebuilt/zircon/x64/core-tests.zbi"
    ))
    .expect("failed to read zbi file");
    let proc = zcore_loader::zircon::run_userboot(zbi, "");
    proc.wait_for_exit().await;
}
