use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use super::App;
use crate::distro::Host;
use crate::place::{self, Machine};
use crate::ssh::{Failure, Remote};

const RECONNECT_EVERY: Duration = Duration::from_secs(30);
const LOST: &str = "the SSH connection ended";

#[derive(Clone, Debug, PartialEq)]
pub enum Reach {
    Reached(Remote),
    Unreached(Failure),
}

#[derive(Default)]
pub(super) struct SshHosts(Mutex<HashMap<String, Watched>>);

struct Watched {
    reach: Option<Reach>,
    now: Arc<Notify>,
}

impl App {
    pub(super) fn watch_ssh_hosts(self: &Arc<Self>) {
        match self.ssh_hosts_in_use() {
            Ok(hosts) => hosts.into_iter().for_each(|host| self.watch_ssh(host)),
            Err(e) => eprintln!("loom: SSH 호스트를 읽지 못했다: {e}"),
        }
    }

    #[expect(dead_code)]
    pub(super) fn ssh_reach(&self, host: &str) -> Option<Reach> {
        let hosts = self.ssh_hosts.0.lock().unwrap();
        hosts.get(host).and_then(|watched| watched.reach.clone())
    }

    #[expect(dead_code)]
    pub(super) fn reconnect_ssh(self: &Arc<Self>, host: &str) {
        self.watch_ssh(host.to_owned());
        if let Some(watched) = self.ssh_hosts.0.lock().unwrap().get(host) {
            watched.now.notify_one();
        }
    }

    fn watch_ssh(self: &Arc<Self>, host: String) {
        let now = match self.ssh_hosts.0.lock().unwrap().entry(host.clone()) {
            Entry::Occupied(_) => return,
            Entry::Vacant(free) => free
                .insert(Watched {
                    reach: None,
                    now: Arc::new(Notify::new()),
                })
                .now
                .clone(),
        };
        tokio::spawn(self.clone().keep_reaching(host, now));
    }

    async fn keep_reaching(self: Arc<Self>, host: String, now: Arc<Notify>) {
        while self.still_used(&host) {
            let started = tokio::time::Instant::now();
            let distros = self.distros.clone();
            let key = Host::Ssh(host.clone());
            let reached = tokio::task::spawn_blocking(move || {
                let held = distros.hold(&key)?;
                Ok((held, distros.remote(&key)?))
            })
            .await
            .unwrap_or_else(|e| Err(Failure::Failed(e.to_string())));
            let next = match reached {
                Ok((held, remote)) => {
                    self.set_reach(&host, Reach::Reached(remote));
                    let _ = tokio::task::spawn_blocking(move || held.closed()).await;
                    self.set_reach(&host, Reach::Unreached(Failure::Unreachable(LOST.into())));
                    started + RECONNECT_EVERY
                }
                Err(failure) => {
                    self.set_reach(&host, Reach::Unreached(failure));
                    tokio::time::Instant::now() + RECONNECT_EVERY
                }
            };
            tokio::select! {
                () = tokio::time::sleep_until(next) => {}
                () = now.notified() => {}
            }
        }
    }

    fn still_used(&self, host: &str) -> bool {
        let mut hosts = self.ssh_hosts.0.lock().unwrap();
        let used = self.ssh_hosts_in_use().map_or_else(
            |e| {
                eprintln!("loom: SSH 호스트를 읽지 못했다: {e}");
                true
            },
            |used| used.iter().any(|name| name == host),
        );
        if !used {
            hosts.remove(host);
        }
        used
    }

    fn set_reach(&self, host: &str, reach: Reach) {
        let mut hosts = self.ssh_hosts.0.lock().unwrap();
        let Some(watched) = hosts.get_mut(host) else {
            return;
        };
        if watched.reach.as_ref() == Some(&reach) {
            return;
        }
        match &reach {
            Reach::Reached(_) => eprintln!("loom: SSH {host}에 닿았다"),
            Reach::Unreached(failure) => eprintln!("loom: SSH {host}에 닿지 않는다: {failure}"),
        }
        watched.reach = Some(reach);
    }

    fn ssh_hosts_in_use(&self) -> rusqlite::Result<Vec<String>> {
        Ok(self
            .repos
            .list()?
            .into_iter()
            .filter_map(|(_, path)| match place::of(&path) {
                Machine::Ssh(at) => Some(at.host),
                Machine::Pc | Machine::Wsl(_) => None,
            })
            .collect())
    }
}
