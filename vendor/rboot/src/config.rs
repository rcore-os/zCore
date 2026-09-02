// TODO: use no_std serde crate to parse

use core::str::FromStr;

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
    pub resolution: Option<(usize, usize)>,
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
    resolution: None,
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
        let r10 = || u64::from_str(value).unwrap();
        let r16 = || u64::from_str_radix(&value[2..], 16).unwrap();
        match key {
            "kernel_stack_address" => self.kernel_stack_address = r16(),
            "kernel_stack_size" => self.kernel_stack_size = r10(),
            "physical_memory_offset" => {
                self.physical_memory_offset = r16();
            }
            "kernel_path" => self.kernel_path = value,
            "resolution" => {
                let mut iter = value.split('x');
                let x = iter.next().unwrap().parse::<usize>().unwrap();
                let y = iter.next().unwrap().parse::<usize>().unwrap();
                self.resolution = Some((x, y));
            }
            "initramfs" => self.initramfs = Some(value),
            "cmdline" => self.cmdline = value,
            _ => warn!("undefined config key: {}", key),
        }
    }
}
