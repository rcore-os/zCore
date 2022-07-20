use crate::{Arch, ArchArg, PROJECT_DIR};
use command_ext::{dir, BinUtil, Cargo, CommandExt, Ext, Qemu};
use std::{fs, path::PathBuf};

#[derive(Clone, Args)]
pub(crate) struct BuildArgs {
    #[clap(flatten)]
    pub arch: ArchArg,
    /// Build as debug mode.
    #[clap(long)]
    pub debug: bool,
    #[clap(long)]
    pub features: Option<String>,
}

#[derive(Args)]
pub(crate) struct AsmArgs {
    #[clap(flatten)]
    build: BuildArgs,
    /// The file to save asm.
    #[clap(short, long)]
    output: Option<PathBuf>,
}

#[derive(Args)]
pub(crate) struct BinArgs {
    #[clap(flatten)]
    build: BuildArgs,
    /// The file to save asm.
    #[clap(short, long)]
    output: Option<PathBuf>,
}

#[derive(Args)]
pub(crate) struct QemuArgs {
    #[clap(flatten)]
    build: BuildArgs,
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

lazy_static::lazy_static! {
    static ref INNER: PathBuf = PROJECT_DIR.join("zCore");
}

impl BuildArgs {
    #[inline]
    fn arch(&self) -> Arch {
        self.arch.arch
    }

    fn target_file_path(&self) -> PathBuf {
        PROJECT_DIR
            .join("target")
            .join(self.arch.arch.name())
            .join(if self.debug { "debug" } else { "release" })
            .join("zcore")
    }

    pub fn invoke(&self, cargo: impl FnOnce() -> Cargo) {
        let features = self.features.clone().unwrap_or_else(|| "linux".into());
        let features = features.split_whitespace().collect::<Vec<_>>();
        // 如果需要链接 rootfs，自动递归
        if features.contains(&"link-user-img") {
            self.arch.linux_rootfs().image();
        }
        cargo()
            .package("zcore")
            .features(false, features)
            .target(INNER.join(format!("{}.json", self.arch().name())))
            .args(&["-Z", "build-std=core,alloc"])
            .args(&["-Z", "build-std-features=compiler-builtins-mem"])
            .conditional(!self.debug, |cargo| {
                cargo.release();
            })
            .invoke();
    }
}

impl AsmArgs {
    /// 打印 asm。
    pub fn asm(self) {
        let Self { build, output } = self;
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
}

impl BinArgs {
    /// 生成 bin 文件
    pub fn bin(self) -> PathBuf {
        let Self { build, output } = self;
        // 递归 build
        build.invoke(Cargo::build);
        // 确定目录
        let obj = build.target_file_path();
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

impl QemuArgs {
    /// 在 qemu 中启动。
    pub fn qemu(&self) {
        // 递归 image
        self.build.arch.linux_rootfs().image();
        // 递归 build
        self.build.invoke(Cargo::build);
        // 构造各种字符串
        let arch = self.build.arch();
        let arch_str = arch.name();
        let obj = self.build.target_file_path();
        let bin = BinArgs {
            build: self.build.clone(),
            output: None,
        }
        .bin();
        // 设置 Qemu 参数
        let mut qemu = Qemu::system(arch_str);
        qemu.args(&["-m", "1G"])
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
                    .arg("-bios")
                    .arg(rustsbi_qemu())
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

/// 下载 rustsbi。
fn rustsbi_qemu() -> PathBuf {
    // https://github.com/opencv/opencv/archive/refs/heads/4.x.zip
    // const NAME: &str = "rustsbi-qemu-release";

    // let origin = Arch::Riscv64.origin();
    // let target = Arch::Riscv64.target();

    // let zip = origin.join(format!("{NAME}.zip"));
    // let dir = target.join(NAME);
    // let url =
    //     format!("https://github.com/rustsbi/rustsbi-qemu/releases/download/v0.1.1/{NAME}.zip");

    // dir::rm(&dir).unwrap();
    // wget(url, &zip);
    // Ext::new("unzip").arg("-d").arg(&dir).arg(zip).invoke();

    // dir.join("rustsbi-qemu.bin")
    PathBuf::from("default")
}
