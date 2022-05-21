use crate::XError;
use std::str::FromStr;

#[derive(Clone, Copy)]
pub(crate) enum Arch {
    Riscv64,
    X86_64,
}

impl Arch {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Riscv64 => "riscv64",
            Self::X86_64 => "x86_64",
        }
    }
}

impl FromStr for Arch {
    type Err = XError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "riscv64" => Ok(Self::Riscv64),
            "x86_64" => Ok(Self::X86_64),
            _ => Err(XError::EnumParse {
                type_name: "Arch",
                value: s.into(),
            }),
        }
    }
}
