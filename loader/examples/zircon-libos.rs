use std::sync::Arc;
use zircon_object::object::{KernelObject, Signal};

#[async_std::main]
async fn main() {
    env_logger::init();
    kernel_hal::init();

    let args = std::env::args().collect::<Vec<_>>();
    if args.len() < 2 {
        println!("Usage: {} ZBI_FILE [CMDLINE]", args[0]);
        std::process::exit(-1);
    }

    let zbi = std::fs::read(&args[1]).expect("failed to read zbi file");
    let cmdline = args.get(2).map(String::as_str).unwrap_or_default();

    let proc: Arc<dyn KernelObject> = zcore_loader::zircon::run_userboot(zbi, cmdline);
    proc.wait_signal(Signal::USER_SIGNAL_0).await;
}
