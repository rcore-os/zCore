// TODO: use no_std serde crate to parse

use core::str::FromStr;
use log::warn;

/// Graphic-mode selection policy (the `resolution=` config key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Key absent: keep whatever mode the firmware already set.
    Keep,
    /// `resolution=auto`: pick the GOP mode matching the display's
    /// EDID-preferred timing; fall back to the largest offered mode. This is
    /// what a fixed value can't do portably — e.g. a 1366x768 TV stretched an
    /// exact `1024x768` (4:3 on 16:9) while its firmware offered better modes.
    Auto,
    /// `resolution=WxH`: request that exact mode (kept if unavailable).
    Exact(usize, usize),
}

/// Config for the bootloader
#[derive(Debug)]
pub struct Config<'a> {
    /// The address at which the kernel stack is placed
    pub kernel_stack_address: u64,
    /// The size of the kernel stack, given in number of 4KiB pages
    pub kernel_stack_size: u64,
    /// The offset into the virtual address space where the physical memory is mapped
    pub physical_memory_offset: u64,
    /// The path of kernel ELF
    pub kernel_path: &'a str,
    /// The resolution of graphic output
    pub resolution: Resolution,
    /// The path of initramfs
    pub initramfs: Option<&'a str>,
    /// Kernel command line
    pub cmdline: &'a str,
}

const DEFAULT_CONFIG: Config = Config {
    kernel_stack_address: 0xFFFF_FF01_0000_0000,
    kernel_stack_size: 512,
    physical_memory_offset: 0xFFFF_8000_0000_0000,
    kernel_path: "\\EFI\\rCore\\kernel.elf",
    resolution: Resolution::Keep,
    initramfs: None,
    cmdline: "",
};

impl<'a> Config<'a> {
    pub fn parse(content: &'a [u8]) -> Self {
        let content = core::str::from_utf8(content).expect("failed to parse config as utf8");
        let mut config = DEFAULT_CONFIG;
        for line in content.split('\n') {
            let line = line.trim();
            // skip empty and comment
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // parse 'key=value'
            let mut iter = line.splitn(2, '=');
            let key = iter.next().expect("failed to parse key");
            let value = iter.next().expect("failed to parse value");
            config.process(key, value);
        }
        config
    }

    fn process(&mut self, key: &str, value: &'a str) {
        // Tolerant numeric parsing: a malformed value keeps the default and
        // warns instead of panicking (same never-brick-boot rule as
        // `resolution` below; the old `&value[2..]` also sliced out of bounds
        // on short values).
        let r10 = || match u64::from_str(value) {
            Ok(v) => Some(v),
            Err(_) => {
                warn!("invalid number for {}: {:?}; keeping default", key, value);
                None
            }
        };
        let r16 = || {
            let digits = value.strip_prefix("0x").unwrap_or(value);
            match u64::from_str_radix(digits, 16) {
                Ok(v) => Some(v),
                Err(_) => {
                    warn!("invalid hex for {}: {:?}; keeping default", key, value);
                    None
                }
            }
        };
        match key {
            "kernel_stack_address" => {
                if let Some(v) = r16() {
                    self.kernel_stack_address = v;
                }
            }
            "kernel_stack_size" => {
                if let Some(v) = r10() {
                    self.kernel_stack_size = v;
                }
            }
            "physical_memory_offset" => {
                if let Some(v) = r16() {
                    self.physical_memory_offset = v;
                }
            }
            "kernel_path" => self.kernel_path = value,
            "resolution" => {
                // NEVER panic on a config value: an unparsable resolution once
                // bricked boot outright (an old bootloader binary reading a
                // newer conf's `auto` died with ParseIntError before drawing
                // anything). Unknown/malformed values degrade to Auto with a
                // warning — the machine always boots.
                if value.eq_ignore_ascii_case("auto") {
                    self.resolution = Resolution::Auto;
                } else {
                    let mut iter = value.split('x');
                    let x = iter.next().and_then(|v| usize::from_str(v).ok());
                    let y = iter.next().and_then(|v| usize::from_str(v).ok());
                    match (x, y) {
                        (Some(x), Some(y)) if x > 0 && y > 0 => {
                            self.resolution = Resolution::Exact(x, y);
                        }
                        _ => {
                            warn!("invalid resolution {:?}; using auto", value);
                            self.resolution = Resolution::Auto;
                        }
                    }
                }
            }
            "initramfs" => self.initramfs = Some(value),
            "cmdline" => self.cmdline = value,
            _ => warn!("undefined config key: {}", key),
        }
    }
}
