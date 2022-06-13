#![allow(dead_code)]
#![allow(unused_variables)]

use alloc::{collections::BTreeMap, string::String, sync::Arc};
use zircon_object::object::KernelObject;
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

            let (cmdline, log_level) = if cfg!(feature = "zircon") {
                let cmdline = args.get(2).cloned().unwrap_or_default();
                let options = parse_cmdline(&cmdline);
                let log_level = String::from(*options.get("LOG").unwrap_or(&""));
                (cmdline, log_level)
            } else {
                (String::new(), std::env::var("LOG").unwrap_or_default())
            };
            BootOptions {
                cmdline,
                log_level,
                #[cfg(feature = "linux")]
                root_proc: args[1..].join("?"),
            }
        } else {
            use alloc::string::ToString;
            let cmdline = kernel_hal::boot::cmdline();
            let options = parse_cmdline(&cmdline);
            BootOptions {
                cmdline: cmdline.clone(),
                log_level: options.get("LOG").unwrap_or(&"").to_string(),
                #[cfg(feature = "linux")]
                root_proc: options.get("ROOTPROC").unwrap_or(&"/bin/busybox?sh").to_string(),
            }
        }
    }
}

fn check_exit_code(proc: Arc<Process>) -> i32 {
    let code = proc.exit_code().unwrap_or(-1);
    if code != 0 {
        error!(
            "process {:?}({}) exited with code {:?}",
            proc.name(),
            proc.id(),
            code
        );
    } else {
        info!(
            "process {:?}({}) exited with code 0",
            proc.name(),
            proc.id()
        )
    }
    code as i32
}

#[cfg(feature = "libos")]
pub fn wait_for_exit(proc: Option<Arc<Process>>) -> ! {
    let exit_code = if let Some(proc) = proc {
        let future = async move {
            use zircon_object::object::Signal;
            let object: Arc<dyn KernelObject> = proc.clone();
            let signal = if cfg!(any(feature = "linux", feature = "baremetal-test")) {
                Signal::PROCESS_TERMINATED
            } else {
                Signal::USER_SIGNAL_0
            };
            object.wait_signal(signal).await;
            check_exit_code(proc)
        };

        // If the graphic mode is on, run the process in another thread.
        #[cfg(feature = "graphic")]
        let future = {
            let handle = async_std::task::spawn(future);
            kernel_hal::libos::run_graphic_service();
            handle
        };

        async_std::task::block_on(future)
    } else {
        warn!("No process to run, exit!");
        0
    };
    std::process::exit(exit_code);
}

#[cfg(not(feature = "libos"))]
pub fn wait_for_exit(proc: Option<Arc<Process>>) -> ! {
    kernel_hal::timer::timer_enable();
    info!("executor run!");
    loop {
        let has_task = executor::run_until_idle();
        if !has_task && cfg!(feature = "baremetal-test") {
            proc.map(check_exit_code);
            kernel_hal::cpu::reset();
        }
        kernel_hal::interrupt::wait_for_interrupt();
    }
}
