use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::adapter::Record;
use crate::distro::Distro;
use crate::process_tree::{self, Snapshot};
use crate::protocol::AdapterRecord;
use crate::wsl::{self, Inside};

#[derive(Serialize, Deserialize)]
struct Stored<S> {
    pid: Option<u32>,
    state: S,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Served {
    pub pid: Option<u32>,
    pub distro: Option<Distro>,
}

impl Served {
    pub async fn start(
        argv: &[String],
        cwd: &Path,
        inside: Option<&Inside>,
        env: &[(&'static str, OsString)],
        port: u16,
        limit: Duration,
    ) -> Result<Served, String> {
        let (served, listening) = match inside {
            Some(inside) => {
                let command = inside.command(argv.to_vec(), env);
                let distro = inside.distro.clone();
                let serving = tokio::task::spawn_blocking(move || distro.serve(command))
                    .await
                    .map_err(|e| e.to_string())??;
                let serving = Arc::new(serving);
                let watching = serving.clone();
                let exited = async move {
                    let _ = tokio::task::spawn_blocking(move || watching.exited()).await;
                };
                let listening = listen(exited, port, limit).await;
                serving.hang_up();
                let served = Served {
                    pid: Some(serving.pid),
                    distro: Some(inside.distro.clone()),
                };
                (served, listening)
            }
            None => {
                let mut cmd = process_tree::command(&argv[0]);
                cmd.args(&argv[1..])
                    .current_dir(cwd)
                    .envs(env.iter().map(|(k, v)| (k, v)));
                crate::timed::own_group(&mut cmd);
                let mut child = tokio::process::Command::from(cmd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| e.to_string())?;
                let pid = child.id();
                let listening = listen(
                    async {
                        let _ = child.wait().await;
                    },
                    port,
                    limit,
                )
                .await;
                tokio::spawn(async move { child.wait().await });
                (Served { pid, distro: None }, listening)
            }
        };
        match listening {
            Ok(()) => Ok(served),
            Err(e) => {
                served.stop();
                Err(e)
            }
        }
    }

    pub fn stop(&self) {
        match (&self.distro, self.pid) {
            (Some(distro), Some(pid)) => wsl::end_trees(distro, &[pid]),
            (None, Some(pid)) => drop(Snapshot::now().kill_trees([pid])),
            (_, None) => {}
        }
    }

    pub async fn end(&self) -> Result<(), String> {
        let Some(pid) = self.pid else {
            return Ok(());
        };
        let distro = self.distro.clone();
        tokio::task::spawn_blocking(move || match distro {
            Some(distro) => distro.end(vec![pid]),
            None => Snapshot::now().end_trees(vec![pid]),
        })
        .await
        .map_err(|e| e.to_string())?
    }

    pub fn here(&self) -> Option<u32> {
        self.pid.filter(|_| self.distro.is_none())
    }

    pub fn record<S: Serialize>(&self, state: S) -> AdapterRecord {
        let stored = Stored {
            pid: self.pid,
            state,
        };
        AdapterRecord(serde_json::to_value(stored).expect("문자열 표는 늘 직렬화된다"))
    }

    pub fn stored<S: DeserializeOwned>(rec: &Record) -> Option<(Served, S)> {
        let stored = Stored::<S>::deserialize(&rec.state.as_ref()?.0)
            .inspect_err(|e| eprintln!("loom: 어댑터 기록을 읽지 못했다: {e}"))
            .ok()?;
        let served = Served {
            pid: stored.pid,
            distro: rec.distro.clone(),
        };
        Some((served, stored.state))
    }
}

async fn listen(
    exited: impl Future<Output = ()>,
    port: u16,
    limit: Duration,
) -> Result<(), String> {
    tokio::time::timeout(limit, process_tree::until_listening(exited, port))
        .await
        .unwrap_or_else(|_| Err("did not listen".into()))
}
