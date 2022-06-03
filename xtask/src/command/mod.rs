use m::ext;
use std::{
    ffi::{OsStr, OsString},
    path::Path,
    process::{Command, ExitStatus, Output},
};

mod cargo;
pub mod dir;
pub mod download;
mod git;
mod make;
mod qemu;
mod tar;

pub(crate) use cargo::Cargo;
pub(crate) use git::Git;
pub(crate) use make::Make;
pub(crate) use qemu::Qemu;
pub(crate) use tar::Tar;

pub(crate) trait CommandExt: AsRef<Command> + AsMut<Command> {
    fn arg(&mut self, s: impl AsRef<OsStr>) -> &mut Self {
        self.as_mut().arg(s);
        self
    }

    fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.arg(arg);
        }
        self
    }

    fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        self.as_mut().current_dir(dir);
        self
    }

    fn env(&mut self, key: impl AsRef<OsStr>, val: impl AsRef<OsStr>) -> &mut Self {
        self.as_mut().env(key, val);
        self
    }

    fn status(&mut self) -> ExitStatus {
        self.as_mut().status().unwrap()
    }

    fn info(&self) -> OsString {
        let cmd = self.as_ref();
        let mut msg = OsString::new();
        if let Some(dir) = cmd.get_current_dir() {
            msg.push("cd ");
            msg.push(dir);
            msg.push(" && ");
        }
        msg.push(cmd.get_program());
        for a in cmd.get_args() {
            msg.push(" ");
            msg.push(a);
        }
        for (k, v) in cmd.get_envs() {
            msg.push(" ");
            msg.push(k);
            if let Some(v) = v {
                msg.push("=");
                msg.push(v);
            }
        }
        msg
    }

    fn invoke(&mut self) {
        let status = self.status();
        if !status.success() {
            panic!(
                "Failed with code {}: {:?}",
                status.code().unwrap(),
                self.info()
            );
        }
    }

    fn output(&mut self) -> Output {
        let output = self.as_mut().output().unwrap();
        if !output.status.success() {
            panic!(
                "Failed with code {}: {:?}",
                output.status.code().unwrap(),
                self.info()
            );
        }
        output
    }
}

pub(crate) struct Ext(Command);

ext!(Ext);

impl Ext {
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self(Command::new(program))
    }
}

mod m {
    macro_rules! ext {
        ($ty:ty) => {
            impl AsRef<Command> for $ty {
                fn as_ref(&self) -> &Command {
                    &self.0
                }
            }

            impl AsMut<Command> for $ty {
                fn as_mut(&mut self) -> &mut Command {
                    &mut self.0
                }
            }

            impl super::CommandExt for $ty {}
        };
    }

    pub(crate) use ext;
}
