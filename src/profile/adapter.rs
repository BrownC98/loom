use std::ffi::OsString;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use portable_pty::CommandBuilder;

use super::process::Served;
use super::{ServerAdapter, VersionMismatch};
use crate::accounts::{AccountId, Site};
use crate::protocol::{
    LaunchError, ModelChoice, Prompt, Provider, ServerRecord, ServerState, Session,
};
use crate::screen::{Dialog, Grammar, Screen};
use crate::skills::Launch;
use crate::wsl::Inside;

pub struct Facts {
    pub provider: Option<Provider>,
    pub sleeps: bool,
    pub listens_at_start: bool,
    pub pinned: Option<&'static str>,
    pub usage_part: &'static str,
    pub waiting: &'static str,
    pub helper_shells: &'static [&'static str],
}

pub struct Command<'a> {
    pub plugin: &'a Path,
    pub launch: Launch,
    pub brief: &'a str,
}

pub struct Serve<'a> {
    pub cwd: &'a Path,
    pub inside: Option<&'a Inside>,
    pub env: &'a [(&'static str, OsString)],
    pub plugin: &'a Path,
    pub plugin_here: &'a Path,
    pub skills: &'a Path,
    pub launch: Launch,
    pub version: Option<&'a str>,
    pub node_ip: Option<IpAddr>,
}

pub struct InputSeen {
    pub empty: bool,
    pub turn_shown: bool,
}

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

pub enum Chosen {
    Sent,
    PromptChanged,
}

pub trait Adapter: Sync {
    fn facts(&self) -> Facts;
    fn leading_args(&self, resume: Option<&Session>, bypass: &[String]) -> Vec<String>;
    fn prepare(&self, at: &Serve<'_>) -> std::io::Result<()>;
    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>);
    fn serve<'a>(
        &'a self,
        at: &'a Serve<'_>,
        resume: Option<&'a Session>,
        bypass: &'a [String],
        cmd: &'a mut CommandBuilder,
    ) -> Option<BoxFuture<'a, Result<Arc<dyn Server>, LaunchError>>>;
    fn reconnect<'a>(
        &'a self,
        state: &'a ServerState,
        served: Served,
        session: Option<&'a Session>,
    ) -> Option<BoxFuture<'a, Result<Arc<dyn Server>, String>>>;
    fn choice_args(&self, choice: &ModelChoice) -> Option<Vec<String>>;
    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])>;
    fn input_seen(&self, screen: &Screen) -> Option<InputSeen>;
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

pub trait Server: Send + Sync {
    fn record(&self) -> ServerRecord;
    fn session(&self) -> Option<Session>;
    fn stop(&self);
    fn resume(&self) -> BoxFuture<'_, Result<(), String>>;
    fn session_file<'a>(&'a self, session: &'a Session) -> BoxFuture<'a, Option<String>>;
    fn choose<'a>(
        &'a self,
        prompt: &'a Prompt,
        option: usize,
    ) -> Option<BoxFuture<'a, Result<Chosen, String>>>;
    fn link(self: Arc<Self>) -> Box<dyn ServerAdapter>;
}
