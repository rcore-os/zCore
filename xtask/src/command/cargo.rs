use super::{ext, CommandExt};
use std::{ffi::OsStr, process::Command};

ext!(def; Cargo);

impl Cargo {
    #[inline]
    fn new(sub: impl AsRef<OsStr>) -> Self {
        let mut git = Self(Command::new("cargo"));
        git.arg(sub);
        git
    }

    #[inline]
    pub fn update() -> Self {
        Self::new("update")
    }

    #[inline]
    pub fn fmt() -> Self {
        Self::new("fmt")
    }

    #[inline]
    pub fn clippy() -> Self {
        Self::new("clippy")
    }

    #[inline]
    pub fn doc() -> Self {
        Self::new("doc")
    }

    #[inline]
    pub fn build() -> Self {
        Self::new("build")
    }

    #[inline]
    pub fn run() -> Self {
        Self::new("run")
    }

    #[inline]
    pub fn install() -> Self {
        Self::new("install")
    }

    #[inline]
    pub fn all_features(&mut self) -> &mut Self {
        self.arg("--all-features");
        self
    }

    pub fn features<S, I>(&mut self, default: bool, feats: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        if !default {
            self.arg("--no-default-features");
        }

        let mut iter = feats.into_iter();
        if let Some(feat) = iter.next() {
            self.arg("--features");
            let mut feats = feat.as_ref().to_os_string();
            for feat in iter {
                feats.push(" ");
                feats.push(feat);
            }
            self.arg(feats);
        }

        self
    }

    #[inline]
    pub fn target(&mut self, target: impl AsRef<OsStr>) -> &mut Self {
        self.arg("--target").arg(target);
        self
    }

    #[inline]
    pub fn package(&mut self, package: impl AsRef<OsStr>) -> &mut Self {
        self.arg("--package").arg(package);
        self
    }

    #[inline]
    pub fn release(&mut self) -> &mut Self {
        self.arg("--release");
        self
    }
}
