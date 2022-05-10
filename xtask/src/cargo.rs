use crate::CommandExt;
use std::{ffi::OsStr, process::Command};

pub(crate) struct Cargo {
    cmd: Command,
}

impl AsRef<Command> for Cargo {
    fn as_ref(&self) -> &Command {
        &self.cmd
    }
}

impl AsMut<Command> for Cargo {
    fn as_mut(&mut self) -> &mut Command {
        &mut self.cmd
    }
}

impl CommandExt for Cargo {}

impl Cargo {
    fn new(sub: &(impl AsRef<OsStr> + ?Sized)) -> Self {
        let mut git = Self {
            cmd: Command::new("cargo"),
        };
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
}
