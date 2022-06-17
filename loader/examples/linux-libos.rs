use std::env;
use std::path::Path;

#[async_std::main]
async fn main() {
    env_logger::init();
    kernel_hal::init();

    let args = std::env::args().collect::<Vec<_>>();
    if args.len() < 2 {
        println!("Usage: {} PROGRAM", args[0]);
        std::process::exit(-1);
    }

    let envs = vec!["PATH=/usr/sbin:/usr/bin:/sbin:/bin".into()];
    let rootfs_path = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("../rootfs/libos");
    let hostfs = rcore_fs_hostfs::HostFS::new(rootfs_path);

    let proc = zcore_loader::linux::run(args[1..].to_vec(), envs, hostfs);
    let code = proc.wait_for_exit().await;
    std::process::exit(code as i32);
}
