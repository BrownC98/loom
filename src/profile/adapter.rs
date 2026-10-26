use std::path::Path;

use portable_pty::CommandBuilder;

use super::{Remote, VersionMismatch};
use crate::protocol::{ModelChoice, Provider};
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

pub trait Adapter: Sync {
    fn facts(&self) -> Facts;
    fn leading_args(&self, resume: Option<&str>, bypass: &[String]) -> Vec<String>;
    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>) -> Result<(), NoServer>;
    fn choice_args(&self, choice: &ModelChoice) -> Option<Vec<String>>;
    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])>;
    fn install(&self, dir: &Path) -> std::io::Result<()>;
    fn version_warning(&self, version_output: &str) -> Option<VersionMismatch>;
}
