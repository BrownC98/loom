use std::ffi::OsString;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use portable_pty::CommandBuilder;

use super::{ModelRule, VersionMismatch};
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
    pub turn_take: TurnTake,
    pub model_rule: ModelRule,
    pub pinned: Option<&'static str>,
    pub usage_part: &'static str,
    pub waiting: &'static str,
    pub helper_shells: &'static [&'static str],
}

#[derive(Clone, Copy)]
pub enum TurnTake {
    StaysInTurn,
    CanOpenTurn,
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
    pub run_dir: &'a Path,
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
    fn wakes<'a>(&'a self, on: &'a mut (dyn FnMut() + Send)) -> BoxFuture<'a, Refusal>;
    fn relaunch_at_turn_end(&self);
    fn ack(&self, ids: &[MessageId]) -> Result<(), Refusal>;
    fn release(&self, ids: &[MessageId]) -> Result<(), Refusal>;
    fn report(&self, report: Report);
    fn ended(&self) -> BoxFuture<'_, ()>;
}

pub trait Adapter: Sync {
    fn facts(&self) -> Facts;
    fn leading_args(&self, resume: Option<&Session>, bypass: &[String]) -> Vec<String>;
    fn prepare(&self, _: &LaunchAt<'_>) -> std::io::Result<()> {
        Ok(())
    }
    fn command(&self, _: &mut CommandBuilder, _: Command<'_>) {}
    fn launch<'a>(
        &'a self,
        _: &'a LaunchAt<'_>,
        _: Option<&'a Session>,
        _: &'a [String],
        _: &'a mut CommandBuilder,
        _: Arc<dyn Port>,
    ) -> BoxFuture<'a, Result<Started, LaunchError>> {
        Box::pin(async { Ok(Started::default()) })
    }
    fn attach<'a>(&'a self, _: &'a Record, _: Arc<dyn Port>) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn answer<'a>(
        &'a self,
        _: &'a Record,
        _: &'a Prompt,
        _: usize,
    ) -> BoxFuture<'a, Result<Chosen, String>> {
        Box::pin(async { Ok(Chosen::OnScreen) })
    }
    fn choice_args(&self, choice: &ModelChoice) -> Option<Vec<String>>;
    fn screen(&self) -> Option<(&'static [Grammar], &'static [Dialog])>;
    fn input_seen(&self, _: &Screen) -> Option<InputSeen> {
        None
    }
    fn install(&self, _: &Path) -> std::io::Result<()> {
        Ok(())
    }
    fn version_warning(&self, _: &str) -> Option<VersionMismatch> {
        None
    }
    fn carry(&self, _: &Session, _: &Carry<'_>) -> Result<(), ResumeError> {
        Ok(())
    }
    fn end_detached<'a>(&'a self, _: &'a Record, _: &'a Site) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn record<'a>(&'a self, rec: &'a Record) -> BoxFuture<'a, Option<String>>;
    fn shell_root(&self, _: &Record) -> Option<u32> {
        None
    }
}
