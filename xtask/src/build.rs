use crate::{linux::LinuxRootfs, Arch, ArchArg, PROJECT_DIR};
use once_cell::sync::Lazy;
use os_xtask_utils::{dir, BinUtil, Cargo, CommandExt, Ext, Qemu};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    fs,
    path::PathBuf,
    str::FromStr,
};
use z_config::MachineConfig;

#[derive(Clone, Args)]
pub(crate) struct BuildArgs {
    /// Which machine is build for.
    #[clap(long, short)]
    pub machine: String,
    /// Build as debug mode.
    #[clap(long)]
    pub debug: bool,
}

#[derive(Args)]
pub(crate) struct OutArgs {
    #[clap(flatten)]
    build: BuildArgs,
    /// The file to save asm.
    #[clap(short, long)]
    output: Option<PathBuf>,
}

#[derive(Args)]
pub(crate) struct QemuArgs {
    #[clap(flatten)]
    arch: ArchArg,
    /// Build as debug mode.
    #[clap(long)]
    debug: bool,
    /// Number of hart (SMP for Symmetrical Multiple Processor).
    #[clap(long)]
    smp: Option<u8>,
    /// Port for gdb to connect. If set, qemu will block and wait gdb to connect.
    #[clap(long)]
    gdb: Option<u16>,
}

#[derive(Args)]
pub(crate) struct GdbArgs {
    #[clap(flatten)]
    arch: ArchArg,
    #[clap(long)]
    port: u16,
}

static INNER: Lazy<PathBuf> = Lazy::new(|| PROJECT_DIR.join("zCore"));

pub(crate) struct BuildConfig {
    arch: Arch,
    debug: bool,
    env: HashMap<OsString, OsString>,
    features: HashSet<String>,
}

impl BuildConfig {
    pub fn from_args(args: BuildArgs) -> Self {
        let machine = MachineConfig::select(args.machine).expect("Unknown target machine");
        let mut features = HashSet::from_iter(machine.features.iter().cloned());
        let mut env = HashMap::new();
        let arch = Arch::from_str(&machine.arch)
            .unwrap_or_else(|_| panic!("Unknown arch {} for machine", machine.arch));
        // 递归 image
        if let Some(path) = &machine.user_img {
            features.insert("link-user-img".into());
            env.insert(
                "USER_IMG".into(),
                if path.is_absolute() {
                    path.as_os_str().to_os_string()
                } else {
                    PROJECT_DIR.join(path).as_os_str().to_os_string()
                },
            );
            LinuxRootfs::new(arch).image();
        }
        // 不支持 pci
        if !machine.pci_support {
            features.insert("no-pci".into());
        }
        if !features.contains("zircon") {
            features.insert("linux".into());
        }
        Self {
            arch,
            debug: args.debug,
            env,
            features,
        }
    }

    #[inline]
    fn target_file_path(&self) -> PathBuf {
        PROJECT_DIR
            .join("target")
            .join(self.arch.name())
            .join(if self.debug { "debug" } else { "release" })
            .join("zcore")
    }

    pub fn invoke(&self, cargo: impl FnOnce() -> Cargo) {
        let mut cargo = cargo();
        cargo
            .package("zcore")
            .features(false, &self.features)
            .target(INNER.join(format!("{}.json", self.arch.name())))
            .args(&["-Z", "build-std=core,alloc"])
            .args(&["-Z", "build-std-features=compiler-builtins-mem"])
            .conditional(!self.debug, |cargo| {
                cargo.release();
            });
        for (key, val) in &self.env {
            println!("set build env: {key:?} : {val:?}");
            cargo.env(key, val);
        }
        cargo.invoke();
    }

    pub fn bin(&self, output: Option<PathBuf>) -> PathBuf {
        // 递归 build
        self.invoke(Cargo::build);
        // 确定目录
        let obj = self.target_file_path();
        let out = output.unwrap_or_else(|| obj.with_extension("bin"));
        // 生成
        println!("strip zcore to {}", out.display());
        dir::create_parent(&out).unwrap();
        BinUtil::objcopy()
            .arg("--binary-architecture=riscv64")
            .arg(obj)
            .args(["--strip-all", "-O", "binary"])
            .arg(&out)
            .invoke();
        out
    }
}

impl OutArgs {
    /// 打印 asm。
    pub fn asm(self) {
        let Self { build, output } = self;
        let build = BuildConfig::from_args(build);
        // 递归 build
        build.invoke(Cargo::build);
        // 确定目录
        let obj = build.target_file_path();
        let out = output.unwrap_or_else(|| PROJECT_DIR.join("target/zcore.asm"));
        // 生成
        println!("Asm file dumps to '{}'.", out.display());
        dir::create_parent(&out).unwrap();
        fs::write(out, BinUtil::objdump().arg(obj).arg("-d").output().stdout).unwrap();
    }

    /// 生成 bin 文件。
    #[inline]
    pub fn bin(self) -> PathBuf {
        let Self { build, output } = self;
        BuildConfig::from_args(build).bin(output)
    }
}

impl QemuArgs {
    /// 在 qemu 中启动。
    pub fn qemu(self) {
        // 递归 image
        self.arch.linux_rootfs().image();
        // 构造各种字符串
        let arch = self.arch.arch;
        let arch_str = arch.name();
        let obj = PROJECT_DIR
            .join("target")
            .join(self.arch.arch.name())
            .join(if self.debug { "debug" } else { "release" })
            .join("zcore");
        // 递归生成内核二进制
        let bin = BuildConfig::from_args(BuildArgs {
            machine: format!("virt-{}", self.arch.arch.name()),
            debug: self.debug,
        })
        .bin(None);
        // 设置 Qemu 参数
        let mut qemu = Qemu::system(arch_str);
        qemu.args(&["-m", "2G"])
            .arg("-kernel")
            .arg(&bin)
            .arg("-initrd")
            .arg(INNER.join(format!("{arch_str}.img")))
            .args(&["-append", "\"LOG=warn\""])
            .args(&["-display", "none"])
            .arg("-no-reboot")
            .arg("-nographic")
            .optional(&self.smp, |qemu, smp| {
                qemu.args(&["-smp", &smp.to_string()]);
            });
        match arch {
            Arch::Riscv64 => {
                qemu.args(&["-machine", "virt"])
                    .args(&["-bios", "default"])
                    .args(&["-serial", "mon:stdio"]);
            }
            Arch::X86_64 => todo!(),
            Arch::Aarch64 => {
                fs::copy(obj, INNER.join("disk").join("os")).unwrap();
                qemu.args(&["-machine", "virt"])
                    .args(&["-cpu", "cortex-a72"])
                    .arg("-bios")
                    .arg(arch.target().join("firmware").join("QEMU_EFI.fd"))
                    .args(&["-hda", &format!("fat:rw:{}/disk", INNER.display())])
                    .args(&[
                        "-drive",
                        &format!(
                            "file={}/aarch64.img,if=none,format=raw,id=x0",
                            INNER.display()
                        ),
                    ])
                    .args(&[
                        "-device",
                        "virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0",
                    ]);
            }
        }
        qemu.optional(&self.gdb, |qemu, port| {
            qemu.args(&["-S", "-gdb", &format!("tcp::{port}")]);
        })
        .invoke();
    }
}

impl GdbArgs {
    pub fn gdb(&self) {
        match self.arch.arch {
            Arch::Riscv64 => {
                Ext::new("riscv64-unknown-elf-gdb")
                    .args(&["-ex", &format!("target remote localhost:{}", self.port)])
                    .invoke();
            }
            Arch::Aarch64 => {
                Ext::new("aarch64-none-linux-gnu-gdb")
                    .args(&["-ex", &format!("target remote localhost:{}", self.port)])
                    .invoke();
            }
            Arch::X86_64 => todo!(),
        }
    }
}
