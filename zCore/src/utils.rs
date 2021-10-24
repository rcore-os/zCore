use alloc::{collections::BTreeMap, sync::Arc};
use zircon_object::task::Process;

pub type BootOptions<'a> = BTreeMap<&'a str, &'a str>;

pub fn parse_cmdline(cmdline: &str) -> BootOptions {
    let mut args = BootOptions::new();
    for opt in cmdline.split(':') {
        // parse "key=value"
        let mut iter = opt.trim().splitn(2, '=');
        if let Some(key) = iter.next() {
            if let Some(value) = iter.next() {
                args.insert(key.trim(), value.trim());
            }
        }
    }
    args
}

#[allow(unused_variables)]
pub fn wait_for_exit(proc: Option<Arc<Process>>) -> ! {
    #[cfg(feature = "libos")]
    if let Some(proc) = proc {
        let future = async move { proc.wait_for_exit().await };

        // If the graphic mode is on, run the process in another thread.
        #[cfg(feature = "graphic")]
        let future = {
            let handle = async_std::task::spawn(future);
            kernel_hal::libos::run_graphic_service();
            handle
        };

        let code = async_std::task::block_on(future);
        std::process::exit(code as i32);
    }
    loop {
        #[cfg(not(feature = "libos"))]
        executor::run_until_idle();
        kernel_hal::interrupt::wait_for_interrupt();
    }
}
