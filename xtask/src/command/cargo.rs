use super::{ext, CommandExt};
use std::{ffi::OsStr, process::Command};

pub(crate) struct Cargo(Command);

ext!(Cargo);

impl Cargo {
    fn new(sub: &(impl AsRef<OsStr> + ?Sized)) -> Self {
        let mut git = Self(Command::new("cargo"));
        git.arg(sub);
        git
    }

    pub fn update() -> Self {
        Self::new("update")
    }

    pub fn fmt() -> Self {
        Self::new("fmt")
    }

    pub fn clippy() -> Self {
        Self::new("clippy")
    }

    pub fn doc() -> Self {
        Self::new("doc")
    }

    pub fn build() -> Self {
        Self::new("build")
    }

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

    pub fn target(&mut self, target: impl AsRef<OsStr>) -> &mut Self {
        self.arg("--target").arg(target);
        self
    }

    pub fn package(&mut self, package: impl AsRef<OsStr>) -> &mut Self {
        self.arg("--package").arg(package);
        self
    }

    pub fn release(&mut self) -> &mut Self {
        self.arg("--release");
        self
    }
}
