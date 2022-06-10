//! 操作 git。

use super::{ext, CommandExt};
use std::{ffi::OsStr, path::PathBuf, process::Command};

pub(crate) struct Git(Command);

ext!(Git);

impl Git {
    fn new(sub: impl AsRef<OsStr>) -> Self {
        let mut git = Self(Command::new("git"));
        git.arg(sub);
        git
    }

    pub fn lfs() -> Self {
        Self::new("lfs")
    }

    pub fn config(global: bool) -> Self {
        let mut git = Self::new("config");
        if global {
            git.arg("--global");
        };
        git
    }

    pub fn clone(repo: impl AsRef<str>) -> GitCloneContext {
        GitCloneContext {
            repo: repo.as_ref().into(),
            dir: None,
            branch: None,
            single_branch: false,
        }
    }

    #[allow(unused)]
    pub fn pull() -> Self {
        Self::new("pull")
    }

    pub fn submodule_update(init: bool) -> Self {
        let mut git = Self::new("submodule");
        git.arg("update");
        if init {
            git.arg("--init");
        }
        git
    }
}

pub(crate) struct GitCloneContext {
    repo: String,
    dir: Option<PathBuf>,
    branch: Option<String>,
    single_branch: bool,
}

impl GitCloneContext {
    pub fn dir(mut self, path: PathBuf) -> Self {
        self.dir = Some(path);
        self
    }

    pub fn branch(mut self, branch: impl AsRef<str>) -> Self {
        self.branch = Some(branch.as_ref().into());
        self
    }

    pub fn single_branch(mut self) -> Self {
        self.single_branch = true;
        self
    }

    pub fn done(self) -> Git {
        let mut git = Git::new("clone");
        git.arg(self.repo);
        if let Some(dir) = self.dir {
            git.arg(dir);
        }
        if self.single_branch {
            git.arg("--single-branch");
        }
        if let Some(branch) = self.branch {
            git.args(&["--branch", &branch]);
        }
        git
    }
}
