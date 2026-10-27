use std::ffi::OsString;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use portable_pty::CommandBuilder;

use super::VersionMismatch;
use crate::accounts::{AccountId, Cli, Site};
use crate::distro::Distro;
use crate::protocol::{
    AdapterRecord, Delivery, IdleReply, IdleRequest, Kind, LaunchError, MessageId, ModelChoice,
    Prompt, Refusal, Report, Session, TakeRequest, Taken,
};
use crate::screen::{Dialog, Grammar, Screen};
use crate::skills::Launch;
use crate::usage::Usage;
use crate::wsl::Inside;

pub struct Facts {
    pub logins: Logins,
    pub sleeps: bool,
    pub listens_at_start: bool,
    pub pinned: Option<&'static str>,
    pub usage_part: &'static str,
    pub waiting: &'static str,
    pub helper_shells: &'static [&'static str],
}

pub enum Logins {
    None,
    One(&'static dyn Meter),
    Many(&'static Cli, &'static dyn Meter),
}

pub trait Meter: Sync {
    fn credentials(&self, site: &Site, id: Option<&AccountId>) -> Option<PathBuf>;
    fn read(&self, site: &Site, id: Option<&AccountId>) -> Usage;
}

pub struct Command<'a> {
    pub plugin: &'a Path,
    pub launch: Launch,
    pub brief: &'a str,
}

pub struct LaunchAt<'a> {
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

#[derive(Default)]
pub struct Started {
    pub record: Option<AdapterRecord>,
    pub session: Option<Session>,
    pub before_start: Option<BoxFuture<'static, Result<(), String>>>,
}

pub struct Record {
    pub state: Option<AdapterRecord>,
    pub session: Option<Session>,
    pub account: Option<AccountId>,
    pub distro: Option<Distro>,
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
    OnScreen,
}

pub trait Port: Send + Sync {
    fn take(&self, req: TakeRequest) -> Result<Taken, Refusal>;
    fn idle(&self, req: IdleRequest) -> BoxFuture<'_, Result<IdleReply, String>>;
    fn next_mail(&self, kinds: &'static [Kind]) -> BoxFuture<'_, Result<Vec<Delivery>, Refusal>>;
    fn ack(&self, ids: &[MessageId]) -> Result<(), Refusal>;
    fn release(&self, ids: &[MessageId]) -> Result<(), Refusal>;
    fn report(&self, report: Report);
    fn ended(&self) -> BoxFuture<'_, ()>;
}

pub trait Adapter: Sync {
    fn facts(&self) -> Facts;
    fn leading_args(&self, resume: Option<&Session>, bypass: &[String]) -> Vec<String>;
    fn prepare(&self, at: &LaunchAt<'_>) -> std::io::Result<()>;
    fn command(&self, cmd: &mut CommandBuilder, at: Command<'_>);
    fn launch<'a>(
        &'a self,
        at: &'a LaunchAt<'_>,
        resume: Option<&'a Session>,
        bypass: &'a [String],
        cmd: &'a mut CommandBuilder,
        port: Arc<dyn Port>,
    ) -> BoxFuture<'a, Result<Started, LaunchError>>;
    fn attach<'a>(
        &'a self,
        rec: &'a Record,
        port: Arc<dyn Port>,
    ) -> BoxFuture<'a, Result<(), String>>;
    fn answer<'a>(
        &'a self,
        rec: &'a Record,
        prompt: &'a Prompt,
        option: usize,
    ) -> BoxFuture<'a, Result<Chosen, String>>;
    fn choice_args(&self, choice: &ModelChoice) -> Option<Vec<String>>;
    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])>;
    fn input_seen(&self, screen: &Screen) -> Option<InputSeen>;
    fn install(&self, dir: &Path) -> std::io::Result<()>;
    fn version_warning(&self, version_output: &str) -> Option<VersionMismatch>;
    fn carry(&self, session: &Session, to: &Carry<'_>) -> Result<(), ResumeError>;
    fn end_detached<'a>(
        &'a self,
        rec: &'a Record,
        site: &'a Site,
    ) -> BoxFuture<'a, Result<(), String>>;
    fn record<'a>(&'a self, rec: &'a Record) -> BoxFuture<'a, Option<String>>;
    fn shell_root(&self, rec: &Record) -> Option<u32>;
}
