use std::env;
use std::path::Path;

#[async_std::main]
async fn main() {
    env_logger::init();
    kernel_hal::init();
    let args = env::args().skip(1).collect();
    let envs = vec!["PATH=/usr/sbin:/usr/bin:/sbin:/bin".into()];
    let rootfs_path = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("../rootfs");
    let hostfs = rcore_fs_hostfs::HostFS::new(rootfs_path);
    let proc = linux_loader::run(args, envs, hostfs);
    let code = proc.wait_for_exit().await;
    std::process::exit(code as i32);
}
