#[cfg(target_arch = "x86_64")]
#[async_std::test]
async fn userboot() {
    kernel_hal::init();
    let zbi = std::fs::read("../prebuilt/zircon/x64/bringup.zbi").expect("failed to read zbi file");
    let proc = zcore_loader::zircon::run_userboot(zbi, "");
    proc.wait_for_exit().await;
}
