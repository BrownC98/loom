use std::path::{Path, PathBuf};

use futures_util::future::BoxFuture;
use portable_pty::CommandBuilder;

use super::{Remote, VersionMismatch};
use crate::accounts::{AccountId, Site};
use crate::protocol::{ModelChoice, Provider, Session};
use crate::screen::{Dialog, Grammar};
use crate::skills::Launch;

pub struct Facts {
    pub provider: Option<Provider>,
    pub hooked: bool,
    pub listens_at_start: bool,
    pub pinned: Option<&'static str>,
    pub usage_part: &'static str,
    pub waiting: &'static str,
}

pub struct Command<'a> {
    pub plugin: &'a Path,
    pub launch: Launch,
    pub brief: &'a str,
    pub remote: Option<Remote<'a>>,
}

pub struct NoServer;

pub struct Carry<'a> {
    pub site: &'a Site,
    pub from: Option<&'a AccountId>,
    pub to: Option<&'a AccountId>,
    pub cwd: &'a Path,
}

pub enum ResumeError {
    NoSession(String),
    Failed(String),
}

impl From<String> for ResumeError {
    fn from(error: String) -> Self {
        ResumeError::Failed(error)
    }
}

impl std::fmt::Display for ResumeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResumeError::NoSession(e) | ResumeError::Failed(e) => f.write_str(e),
        }
    }
}

pub trait Adapter: Sync {
    fn facts(&self) -> Facts;
    fn leading_args(&self, resume: Option<&Session>, bypass: &[String]) -> Vec<String>;
    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) -> Result<(), NoServer>;
    fn choice_args(&self, choice: &ModelChoice) -> Option<Vec<String>>;
    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])>;
    fn install(&self, dir: &Path) -> std::io::Result<()>;
    fn version_warning(&self, version_output: &str) -> Option<VersionMismatch>;
    fn carry(&self, session: &Session, to: &Carry<'_>) -> Result<(), ResumeError>;
    fn end_detached<'a>(
        &'a self,
        session: &'a Session,
        site: &'a Site,
        account: Option<&'a AccountId>,
    ) -> BoxFuture<'a, Result<(), String>>;
    fn record(&self, session: &Session) -> Option<PathBuf>;
}
