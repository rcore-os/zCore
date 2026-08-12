//! 支持架构的定义。

use crate::{commands::wget, LinuxRootfs, XError, ARCHS, TARGET};
use os_xtask_utils::{dir, CommandExt, Tar};
use std::{path::PathBuf, str::FromStr};

/// 支持的 CPU 架构。
#[derive(Clone, Copy)]
pub(crate) enum Arch {
    Riscv64,
    X86_64,
    Aarch64,
}

impl Arch {
    /// Returns the name of Arch.
    #[inline]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Riscv64 => "riscv64",
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }

    /// Returns the path to store arch-dependent files from network.
    #[inline]
    pub fn origin(&self) -> PathBuf {
        ARCHS.join(self.name())
    }

    /// Returns the path to cache arch-dependent generated files durning processes.
    #[inline]
    pub fn target(&self) -> PathBuf {
        TARGET.join(self.name())
    }

    /// Returns the path to a linux musl toolchain.
    ///
    /// On macOS, uses the Homebrew-installed `musl-cross` toolchain (under
    /// `$(brew --prefix musl-cross)/libexec`), which has the same internal
    /// directory layout as the downloaded archive.
    ///
    /// On Linux, downloads the pre-built toolchain archive from GitHub.
    pub fn linux_musl_cross(&self) -> PathBuf {
        // On macOS, use the Homebrew-installed toolchain instead of
        // downloading a Linux ELF archive that cannot execute on Darwin.
        if cfg!(target_os = "macos") {
            let prefix = std::process::Command::new("brew")
                .args(["--prefix", "musl-cross"])
                .output()
                .expect("failed to run `brew --prefix musl-cross` — is Homebrew installed?");
            assert!(
                prefix.status.success(),
                "musl-cross is not installed via Homebrew. Run: \
                 brew install FiloSottile/musl-cross/musl-cross --with-{}",
                self.name()
            );
            let prefix = String::from_utf8(prefix.stdout)
                .expect("non-UTF-8 brew output")
                .trim()
                .to_string();
            let dir = PathBuf::from(prefix).join("libexec");
            assert!(
                dir.join("bin")
                    .join(format!("{}-linux-musl-gcc", self.name()))
                    .is_file(),
                "musl-cross is installed but missing {arch}-linux-musl-gcc. \
                 Reinstall with: brew install FiloSottile/musl-cross/musl-cross --with-{arch}",
                arch = self.name()
            );
            return dir;
        }

        let name = format!("{}-linux-musl-cross", self.name().to_lowercase());

        let origin = self.origin();
        let target = self.target();

        let tgz = origin.join(format!("{name}.tgz"));
        let dir = target.join(&name);

        dir::create_parent(&dir).unwrap();
        dir::rm(&dir).unwrap();

        wget(
            format!("https://github.com/YdrMaster/zCore/releases/download/musl-cache/{name}.tgz"),
            &tgz,
        );
        Tar::xf(&tgz, Some(target)).invoke();

        dir
    }
}

impl FromStr for Arch {
    type Err = XError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "riscv64" => Ok(Self::Riscv64),
            "x86_64" => Ok(Self::X86_64),
            "aarch64" => Ok(Self::Aarch64),
            _ => Err(XError::EnumParse {
                type_name: "Arch",
                value: s.into(),
            }),
        }
    }
}

#[derive(Clone, Copy, Args)]
pub(crate) struct ArchArg {
    /// Build architecture, `riscv64` or `x86_64`.
    #[clap(short, long)]
    pub arch: Arch,
}

impl ArchArg {
    /// Returns the [`LinuxRootfs`] object related to selected architecture.
    #[inline]
    pub fn linux_rootfs(&self) -> LinuxRootfs {
        LinuxRootfs::new(self.arch)
    }
}
