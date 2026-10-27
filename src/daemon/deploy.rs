use std::sync::Arc;

use super::App;
use crate::graph::Deploy;
use crate::protocol::{SHIP_DEPLOY_FAILED, SHIP_SCRIPT};
use crate::worktree::merge::{Lines, SHIP, script};

pub(super) fn recover(app: &Arc<App>) {
    match app.graph.deploys() {
        Ok(deploys) => deploys.into_iter().for_each(|d| start(app, d)),
        Err(e) => eprintln!("loom: deploy recovery: {e}"),
    }
}

pub(super) fn start(app: &Arc<App>, deploy: Deploy) {
    tokio::spawn(confirm(app.clone(), deploy));
}

async fn confirm(app: Arc<App>, deploy: Deploy) {
    let failure = check(&app, &deploy).await.err();
    if let Err(e) = app.graph.deploy_ended(&deploy, failure.as_deref()) {
        eprintln!("loom: deploy {}: {e}", deploy.sha);
    }
}

async fn check(app: &App, deploy: &Deploy) -> Result<(), String> {
    let dir = app.repos.path(deploy.repo).map_err(|e| e.to_string())?;
    let args = [
        format!("deploy-{}.log", deploy.id),
        deploy.branch.clone(),
        deploy.sha.clone(),
    ];
    let (code, out) = script(&dir, SHIP, &args).await.map_err(|e| e.to_string())?;
    let log = Lines::read(&out)
        .log
        .unwrap_or_else(|| out.trim().to_owned());
    match code {
        0 => Ok(()),
        SHIP_DEPLOY_FAILED => Err(format!(
            "{SHIP_SCRIPT} did not see the deploy in time (log {log})"
        )),
        _ => Err(format!(
            "{SHIP_SCRIPT} exited {code} and did not confirm the deploy (log {log})"
        )),
    }
}
