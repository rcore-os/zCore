use alloc::collections::BTreeMap;

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

pub fn run_tasks_forever() -> ! {
    loop {
        executor::run_until_idle();
        kernel_hal::interrupt::wait_for_interrupt();
    }
}
