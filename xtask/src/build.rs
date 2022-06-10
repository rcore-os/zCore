﻿use crate::{
    command::{Cargo, CommandExt, Ext, Qemu},
    Arch, ArchArg,
};
use std::{fs, path::PathBuf};

#[derive(Args)]
pub(crate) struct BuildArgs {
    #[clap(flatten)]
    arch: ArchArg,
    /// Build as debug mode.
    #[clap(long)]
    debug: bool,
}

#[derive(Args)]
pub(crate) struct AsmArgs {
    #[clap(flatten)]
    build: BuildArgs,
    /// The file to save asm.
    #[clap(short, long)]
    output: PathBuf,
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

impl BuildArgs {
    #[inline]
    fn arch(&self) -> Arch {
        self.arch.arch
    }

    fn dir(&self) -> String {
        format!(
            "target/{arch}/{mode}",
            arch = self.arch().name(),
            mode = if self.debug { "debug" } else { "release" }
        )
    }

    fn build(&self) {
        let mut cargo = Cargo::build();
        cargo
            .package("zcore")
            .features(false, &["linux", "board-qemu"])
            .target(format!("zCore/{arch}.json", arch = self.arch().name()))
            .args(&["-Z", "build-std=core,alloc"])
            .args(&["-Z", "build-std-features=compiler-builtins-mem"]);
        if !self.debug {
            cargo.release();
        }
        cargo.invoke();
    }
}

impl AsmArgs {
    /// 打印 asm。
    pub fn asm(&self) {
        // 递归 build
        self.build.build();
        let out = Ext::new("rust-objdump")
            .arg(format!("{dir}/zcore", dir = self.build.dir()))
            .arg("-d")
            .output()
            .stdout;
        fs::write(&self.output, out).unwrap();
    }
}

impl QemuArgs {
    /// 在 qemu 中启动。
    pub fn qemu(&self) {
        // 递归 image
        self.build.arch.linux_rootfs().image();
        // 递归 build
        self.build.build();
        // 构造各种字符串
        let arch = self.build.arch();
        let arch_str = arch.name();
        let dir = self.build.dir();
        let obj = format!("{dir}/zcore");
        let bin = format!("{dir}/zcore.bin");
        // 裁剪内核二进制文件
        Ext::new("rust-objcopy")
            .arg(format!("--binary-architecture={arch_str}"))
            .arg(obj.clone())
            .arg("--strip-all")
            .args(&["-O", "binary", &bin])
            .invoke();
        // 设置 Qemu 参数
        let mut qemu = Qemu::system(arch);
        qemu.args(&["-m", "512M"])
            .args(&["-kernel", &bin])
            .args(&["-initrd", &format!("zCore/{arch_str}.img")])
            .args(&["-append", "\"LOG=warn\""])
            .args(&["-display", "none"])
            .arg("-no-reboot")
            .arg("-nographic");
        if let Some(smp) = self.smp {
            qemu.args(&["-smp", &smp.to_string()]);
        }
        match arch {
            Arch::Riscv64 => {
                qemu.args(&["-machine", "virt"])
                    .arg("-bios")
                    .arg(rustsbi_qemu())
                    .args(&["-serial", "mon:stdio"]);
            }
            Arch::X86_64 => todo!(),
            Arch::Aarch64 => {
                fs::create_dir_all("zCore/disk/EFI/Boot").unwrap();
                fs::copy(
                    "prebuilt/firmware/aarch64/aarch64_uefi.efi",
                    "zCore/disk/EFI/Boot/bootaa64.efi",
                )
                .unwrap();
                fs::copy(
                    "prebuilt/firmware/aarch64/Boot.json",
                    "zCore/disk/EFI/Boot/Boot.json",
                )
                .unwrap();
                fs::copy(obj, "zCore/disk/os").unwrap();
                qemu.args(&["-machine", "virt"])
                    .args(&["-cpu", "cortex-a72"])
                    .args(&["-m", "1G"])
                    .args(&["-bios", "prebuilt/firmware/aarch64/QEMU_EFI.fd"])
                    .args(&["-hda", "fat:rw:zCore/disk"])
                    .args(&["-drive", "file=zCore/aarch64.img,if=none,format=raw,id=x0"])
                    .args(&[
                        "-device",
                        "virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0",
                    ]);
            }
        }
        if let Some(port) = self.gdb {
            qemu.args(&["-S", "-gdb", &format!("tcp::{port}")]);
        }
        qemu.invoke();
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
    // PathBuf::from("default")
    PathBuf::from("../rustsbi-qemu/target/riscv64imac-unknown-none-elf/release/rustsbi-qemu.bin")
}
