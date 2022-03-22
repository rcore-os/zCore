#![allow(dead_code)]
#![allow(unused_variables)]

use alloc::{collections::BTreeMap, string::String, sync::Arc};
use zircon_object::task::Process;

#[derive(Debug)]
pub struct BootOptions {
    pub cmdline: String,
    pub log_level: String,
    #[cfg(feature = "linux")]
    pub root_proc: String,
}

fn parse_cmdline(cmdline: &str) -> BTreeMap<&str, &str> {
    let mut options = BTreeMap::new();
    for opt in cmdline.split(':') {
        // parse "key=value"
        let mut iter = opt.trim().splitn(2, '=');
        if let Some(key) = iter.next() {
            if let Some(value) = iter.next() {
                options.insert(key.trim(), value.trim());
            }
        }
    }
    options
}

pub fn boot_options() -> BootOptions {
    cfg_if! {
        if #[cfg(feature = "libos")] {
            let args = std::env::args().collect::<Vec<_>>();
            if args.len() < 2 {
                #[cfg(feature = "linux")]
                println!("Usage: {} PROGRAM", args[0]);
                #[cfg(feature = "zircon")]
                println!("Usage: {} ZBI_FILE [CMDLINE]", args[0]);
                std::process::exit(-1);
            }

            let log_level = std::env::var("LOG").unwrap_or_default();
            let cmdline = if cfg!(feature = "zircon") {
                args.get(2).cloned().unwrap_or_default()
            } else {
                String::new()
            };
            BootOptions {
                cmdline,
                log_level,
                #[cfg(feature = "linux")]
                root_proc: args[1..].join("?"),
            }
        } else {
            let cmdline = kernel_hal::boot::cmdline();
            let options = parse_cmdline(&cmdline);
            BootOptions {
                cmdline: cmdline.clone(),
                log_level: String::from(*options.get("LOG").unwrap_or(&"")),
                #[cfg(feature = "linux")]
                root_proc: String::from(*options.get("ROOTPROC").unwrap_or(&"/bin/busybox?sh")),
            }
        }
    }
}

pub fn wait_for_exit(proc: Option<Arc<Process>>) -> ! {
    #[cfg(feature = "libos")]
    if let Some(proc) = proc {
        let future = async move {
            use zircon_object::object::{KernelObject, Signal};
            let object: Arc<dyn KernelObject> = proc.clone();
            let signal = if cfg!(feature = "zircon") {
                Signal::USER_SIGNAL_0
            } else {
                Signal::PROCESS_TERMINATED
            };
            object.wait_signal(signal).await;
            let code = proc.exit_code().unwrap_or(-1);
            info!(
                "process {:?}({}) exited with code {:?}",
                proc.name(),
                proc.id(),
                code
            );
            code
        };

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
        // kernel_hal::interrupt::wait_for_interrupt();
    }
}
