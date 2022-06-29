//! 支持架构的定义。

use crate::{commands::wget, LinuxRootfs, XError, ARCHS, TARGET};
use command_ext::{dir, CommandExt, Tar};
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

    /// Downloads linux musl toolchain, and returns its path.
    pub fn linux_musl_cross(&self) -> PathBuf {
        let name = format!("{}-linux-musl-cross", self.name().to_lowercase());

        let origin = self.origin();
        let target = self.target();

        let tgz = origin.join(format!("{name}.tgz"));
        let dir = target.join(&name);

        dir::create_parent(&dir).unwrap();
        dir::rm(&dir).unwrap();
        wget(format!("https://musl.cc/{name}.tgz"), &tgz);
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

#[derive(Args)]
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
