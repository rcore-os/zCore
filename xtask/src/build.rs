use crate::{
    arch::ArchArg,
    command::{Cargo, CommandExt, Ext, Qemu},
    Arch,
};
use std::{fs, path::PathBuf};

#[derive(Args)]
pub(crate) struct BuildArgs {
    #[clap(flatten)]
    arch: ArchArg,
    #[clap(long)]
    debug: bool,
}

#[derive(Args)]
pub(crate) struct AsmArgs {
    #[clap(flatten)]
    build: BuildArgs,
    #[clap(short, long)]
    output: PathBuf,
}

impl BuildArgs {
    fn build(&self) {
        let mut cargo = Cargo::build();
        cargo
            .package("zcore")
            .features(false, &["linux", "board-qemu"])
            .target(format!("zCore/{arch}.json", arch = self.arch.arch.as_str()))
            .args(&["-Z", "build-std=core,alloc"])
            .args(&["-Z", "build-std-features=compiler-builtins-mem"]);
        if !self.debug {
            cargo.release();
        }
        cargo.invoke();
    }

    fn dir(&self) -> String {
        format!(
            "target/{arch}/{mode}",
            arch = self.arch.arch.as_str(),
            mode = if self.debug { "debug" } else { "release" }
        )
    }

    /// 在 qemu 中启动。
    pub fn qemu(&self) {
        // 递归 image
        self.arch.image();
        // 递归 build
        self.build();
        match self.arch.arch {
            Arch::Riscv64 => {
                Ext::new("rust-objcopy")
                    .arg("--binary-architecture=riscv64")
                    .arg("target/riscv64/release/zcore")
                    .arg("--strip-all")
                    .args(&["-O", "binary", "target/riscv64/release/zcore.bin"])
                    .invoke();
                Qemu::system(self.arch.arch)
                    .args(&["-smp", "1"])
                    .args(&["-machine", "virt"])
                    .arg("-bios")
                    .arg(rustsbi_qemu())
                    .args(&["-m", "512M"])
                    .args(&["-serial", "mon:stdio"])
                    .args(&["-kernel", "target/riscv64/release/zcore.bin"])
                    .args(&["-initrd", "zCore/riscv64.img"])
                    .args(&["-append", "\"LOG=warn\""])
                    .args(&["-display", "none"])
                    .arg("-no-reboot")
                    .arg("-nographic")
                    .invoke();
            }
            Arch::X86_64 => todo!(),
        }
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

/// 下载 rustsbi。
fn rustsbi_qemu() -> PathBuf {
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
