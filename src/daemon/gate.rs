//! 관문 경로(`/api/agent/{ready,verdict,fail}`): git으로 브랜치 사실을 읽어 그래프의 관문 상태에 넘긴다. 모두 바로 돌아온다.
//! 무거운 일(쌓기, 검사 스크립트, 병합, push, 배포 확인)은 데몬의 병합 큐(`queue`)가 한다(2026-10-23 사람 결정 Q697 A, 총괄 #3761).
//! 모든 호출과 거절은 흔적을 남긴다. 근거: 출력 표준 R8 v5 ③·④·⑤(2026-10-23 사람 결정 Q559 A).

use std::path::Path;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use super::{AgentError, App, for_agent};
use crate::git;
use crate::graph::{GateChoice, NodeId, Refusal, Round, Via};
use crate::protocol::{self, CHECK_SCRIPT, GateState, GateVerb, Judgment, SHIP_SCRIPT};
use crate::worktree;

/// CLI가 요청마다 보내는 입구 헤더: 부른 `loom` 명령 이름과 판.
pub const TOOL_HEADER: &str = "loom-tool";
pub const VERSION_HEADER: &str = "loom-version";

#[derive(Serialize, Deserialize)]
pub struct ReadyRequest {
    pub branch: String,
    /// 부른 프로세스의 작업 폴더(부른 쪽이 읽는 글 그대로). 노드가 등록한 워크트리가 아닌 곳에서 일할 때 그 폴더가 담당 증거다.
    pub cwd: String,
}

#[derive(Serialize, Deserialize)]
pub struct Readied {
    pub gate: String,
    pub target: String,
}

#[derive(Serialize, Deserialize)]
pub struct VerdictRequest {
    pub branch: String,
    pub judgment: Judgment,
    /// 결함 파일 경로(부른 쪽이 읽는 글 그대로). block이면 꼭 있다.
    pub findings: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct FailRequest {
    pub branch: String,
    pub cause: String,
}

/// 요청이 들어온 입구. 헤더가 없으면(화면, 옛 CLI, 손으로 부른 API) 경로다.
pub fn via(headers: &HeaderMap, route: &str) -> Via {
    let header = |name| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    Via {
        tool: header(TOOL_HEADER).unwrap_or_else(|| route.to_owned()),
        version: header(VERSION_HEADER),
    }
}

impl App {
    /// 손으로 쓴 글(`note`·`answer`·`done`)이 종류 낱말로 시작하면 거절한다. 판정·병합 낱말은 관문 스크립트가 있는 저장소에서만 본다:
    /// 없는 저장소(ktasearch)는 `loom verdict`와 병합 큐가 돌지 않아 옛 글을 쓴다(이전 기간).
    pub(super) fn kind_word(&self, node: NodeId, body: &str) -> Result<(), Refusal> {
        let Some(found) = protocol::kind_word(body) else {
            return Ok(());
        };
        if found.gated_only && !self.gated_repo(node)? {
            return Ok(());
        }
        Err(Refusal::KindWord {
            word: found.word.to_owned(),
            verb: found.verb.to_owned(),
        })
    }

    /// `loom done`의 담당 브랜치 경고. 코디 밑이 아닌 노드의 워크트리 브랜치가 대상(원래 체크아웃의 브랜치)에 병합되지 않았고
    /// 그 커밋으로 `loom ready`도 보내지 않았으면 경고 글이다. 거절하지 않는다: 완료는 중간 결과도 나른다.
    /// 근거: 요청 #3067(ktasearch: 워커가 브랜치를 알리지 않고 끝내 1.5시간 아무도 보지 않았다).
    pub(super) async fn unreadied(&self, node: NodeId) -> Result<Option<String>, Refusal> {
        if self.graph.under_coordinator(node)? {
            return Ok(None);
        }
        let place = self.graph.place_of(node)?;
        let repo = self.repo_of(node)?;
        // 원래 체크아웃에서 도는 노드는 담당 브랜치가 없다. git을 부르지 않는다.
        if Path::new(&place.worktree) == repo {
            return Ok(None);
        }
        branch_warning(&repo, &place.worktree, |branch, head| {
            self.graph.ready_sent(node, branch, head)
        })
        .await
    }

    /// 노드의 저장소가 관문 계약(원래 체크아웃의 `scripts/gate/check.sh`, `ship.sh`)을 가졌는가.
    pub(super) fn gated_repo(&self, node: NodeId) -> Result<bool, Refusal> {
        let repo = self.repo_of(node)?;
        Ok(repo.join(CHECK_SCRIPT).is_file() && repo.join(SHIP_SCRIPT).is_file())
    }

    /// 관문 호출의 결과를 돌려준다. 거절은 흔적으로 남긴다. DB 오류는 요청의 잘못이 아니므로 남기지 않는다.
    fn traced<T>(
        &self,
        node: NodeId,
        verb: GateVerb,
        branch: &str,
        via: &Via,
        result: Result<T, Refusal>,
    ) -> Result<Json<T>, AgentError> {
        match result {
            Ok(value) => {
                self.turn_ends.called(node);
                Ok(Json(value))
            }
            Err(reason) => {
                if !matches!(reason, Refusal::Db(_)) {
                    self.graph
                        .record_gate_refusal(node, verb, branch, &reason, Some(via))
                        .map_err(|e| for_agent(Refusal::from(e)))?;
                }
                Err(for_agent(reason))
            }
        }
    }

    fn repo_of(&self, node: NodeId) -> Result<std::path::PathBuf, Refusal> {
        let place = self.graph.place_of(node)?;
        self.repos
            .path(place.repo)
            .map_err(|e| Refusal::Git(e.to_string()))
    }

    /// 대상 브랜치를 담지 않아도 받는다. 병합 큐가 지금 대상 위에 쌓고, 충돌하면 반려한다(2026-10-24 코디 #3889: notAncestor 거절 8건, 워커가 main을 손으로 병합했다).
    async fn ready(
        &self,
        node: NodeId,
        branch: &str,
        cwd: &str,
        via: &Via,
    ) -> Result<Readied, Refusal> {
        let place = self.graph.place_of(node)?;
        let repo = self.repo_of(node)?;
        let mine = owned_worktree(&repo, &place.worktree, branch, cwd).await?;
        if !worktree::changes(&mine)
            .await
            .map_err(git_problem)?
            .is_empty()
        {
            return Err(Refusal::Uncommitted(branch.to_owned()));
        }
        let base = base_branch(&repo).await?;
        let (gate, target) = match self.graph.gate_for(node)? {
            GateChoice::Coordinator { gate, worktree } => {
                let theirs = worktree::find(&repo, &worktree)
                    .await
                    .map_err(git_problem)?;
                match theirs.branch {
                    Some(target) if target != base => (gate, target),
                    // 코디가 원래 체크아웃(main)에서 일한다: 워커 브랜치는 코디가 받는다. 워커는 `loom done`으로 알린다.
                    _ => return Err(Refusal::SameAsTarget(base)),
                }
            }
            GateChoice::Owner => (node, base),
        };
        if target == branch {
            return Err(Refusal::SameAsTarget(branch.to_owned()));
        }
        if contains(&repo, branch, &target).await? {
            return Err(Refusal::NothingToMerge {
                branch: branch.to_owned(),
                target,
            });
        }
        // 관문 에이전트는 WSL에서 이 경로를 읽는다. Windows 데몬이 보는 UNC 글자 대신 배포판 안의 경로를 쓴다.
        let worktree =
            crate::wsl::location(&mine.path).map_or_else(|| mine.path.clone(), |w| w.path);
        let round = Round {
            branch,
            target: &target,
            worktree: &worktree,
            head: &mine.head,
        };
        let gate = match self.gated_repo(node)? {
            true => self.graph.ready(node, &round, gate, Some(via))?,
            false => self.graph.announce_ready(node, &round, gate, Some(via))?,
        };
        Ok(Readied { gate, target })
    }
}

/// `tip`의 첫 부모 줄에서 `head`를 둘째 이후 부모로 가진 병합 커밋. 병합 큐는 늘 `--no-ff`로 쌓는다.
pub(super) async fn merge_commit(
    repo: &Path,
    head: &str,
    tip: &str,
) -> Result<Option<String>, Refusal> {
    let listed = git::run(repo, &["log", "--first-parent", "--format=%H %P", tip])
        .await
        .map_err(Refusal::Git)?;
    Ok(listed.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let sha = parts.next()?;
        parts.skip(1).any(|p| p == head).then(|| sha.to_owned())
    }))
}

/// 원래 체크아웃이 가리키는 브랜치. 스스로 관문인 담당의 대상이다(보통 main).
pub(super) async fn base_branch(repo: &Path) -> Result<String, Refusal> {
    worktree::worktrees(repo)
        .await
        .into_iter()
        .find(|w| w.original)
        .and_then(|w| w.branch)
        .ok_or_else(|| Refusal::Git("the main checkout is not on a branch".into()))
}

/// `older`가 `newer`에 포함되는가. git은 포함되지 않으면 글 없이 1로 끝난다.
pub(super) async fn contains(repo: &Path, older: &str, newer: &str) -> Result<bool, Refusal> {
    match git::run(repo, &["merge-base", "--is-ancestor", older, newer]).await {
        Ok(_) => Ok(true),
        Err(e) if e.is_empty() => Ok(false),
        Err(e) => Err(Refusal::Git(e)),
    }
}

/// `branch`를 담당하는 워크트리. 노드가 등록한 워크트리가 그 브랜치면 그것이고,
/// 아니면 부른 프로세스의 작업 폴더가 git으로 그 브랜치 워크트리일 때(원래 체크아웃의 노드가 다른 워크트리에서 고칠 때) 그 워크트리다.
async fn owned_worktree(
    repo: &Path,
    registered: &str,
    branch: &str,
    cwd: &str,
) -> Result<worktree::Worktree, Refusal> {
    let mine = worktree::find(repo, registered)
        .await
        .map_err(git_problem)?;
    if mine.branch.as_deref() == Some(branch) {
        return Ok(mine);
    }
    // 폴더 접두사는 안쪽 워크트리·중첩 저장소를 가르지 못한다. git이 푼 최상위 폴더만 믿는다.
    let cwd = Path::new(cwd);
    let top = git::run(cwd, &["rev-parse", "--show-toplevel"])
        .await
        .map_err(Refusal::Git)?;
    let top = git::from_git_path(cwd, top.trim());
    worktree::worktrees(repo)
        .await
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(branch) && w.path == top)
        .ok_or(Refusal::NotOwner {
            branch: branch.to_owned(),
            yours: mine.branch,
        })
}

/// `worktree`의 브랜치가 원래 체크아웃의 브랜치가 아니고, 거기에 병합되지 않았고, 그 커밋으로 `ready`를 보내지 않았으면(`ready_sent`) 경고 글.
async fn branch_warning(
    repo: &Path,
    worktree: &str,
    ready_sent: impl FnOnce(&str, &str) -> rusqlite::Result<bool>,
) -> Result<Option<String>, Refusal> {
    let mine = worktree::find(repo, worktree).await.map_err(git_problem)?;
    let base = base_branch(repo).await?;
    let Some(branch) = mine.branch.filter(|b| *b != base) else {
        return Ok(None);
    };
    if contains(repo, &mine.head, &base).await? || ready_sent(&branch, &mine.head)? {
        return Ok(None);
    }
    Ok(Some(format!(
        "branch {branch} ({}) is not merged into {base}, and no `loom ready {branch}` was sent for this commit",
        short(&mine.head)
    )))
}

/// 경고 글의 짧은 커밋.
fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

fn git_problem(e: worktree::RepoProblem) -> Refusal {
    Refusal::Git(e.to_string())
}

pub(super) async fn agent_ready(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<ReadyRequest>,
) -> Result<Json<Readied>, AgentError> {
    let node = app.caller(&headers)?;
    let via = via(&headers, "/api/agent/ready");
    let result = app.ready(node, &req.branch, &req.cwd, &via).await;
    app.traced(node, GateVerb::Ready, &req.branch, &via, result)
}

pub(super) async fn agent_verdict(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<VerdictRequest>,
) -> Result<Json<GateState>, AgentError> {
    let node = app.caller(&headers)?;
    let via = via(&headers, "/api/agent/verdict");
    let result = app.graph.verdict(
        node,
        &req.branch,
        req.judgment,
        req.findings.as_deref(),
        Some(&via),
    );
    if result == Ok(GateState::Passed) {
        app.merge_queue.notify_one();
    }
    app.traced(node, GateVerb::Verdict, &req.branch, &via, result)
}

pub(super) async fn agent_fail(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(req): Json<FailRequest>,
) -> Result<Json<GateState>, AgentError> {
    let node = app.caller(&headers)?;
    let via = via(&headers, "/api/agent/fail");
    let result = app.graph.fail(node, &req.branch, &req.cause, Some(&via));
    app.traced(node, GateVerb::Fail, &req.branch, &via, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::test_run as run;

    /// 요청 #3067: 담당 브랜치가 병합되지 않았고 그 커밋으로 `loom ready`도 없으면 `loom done`에 경고가 붙는다(거절하지 않는다).
    #[tokio::test]
    async fn done_warns_for_an_unmerged_branch_without_ready() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["commit", "-q", "--allow-empty", "-m", "a"]);
        let repo = std::path::PathBuf::from(run(&repo, &["rev-parse", "--show-toplevel"]));
        let main = repo.display().to_string();
        let feature = dir.path().join("feature");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                feature.to_str().unwrap(),
            ],
        );
        let feature = run(&feature, &["rev-parse", "--show-toplevel"]);
        let never = |_: &str, _: &str| Ok(false);
        // 원래 체크아웃의 브랜치는 보지 않는다. 대상과 같은 커밋(병합할 것이 없음)도 보지 않는다.
        assert_eq!(branch_warning(&repo, &main, never).await, Ok(None));
        assert_eq!(branch_warning(&repo, &feature, never).await, Ok(None));
        run(
            Path::new(&feature),
            &["commit", "-q", "--allow-empty", "-m", "b"],
        );
        let head = run(Path::new(&feature), &["rev-parse", "HEAD"]);
        let warning = branch_warning(&repo, &feature, never)
            .await
            .unwrap()
            .unwrap();
        assert!(
            warning.starts_with(&format!(
                "branch feature ({}) is not merged into main",
                &head[..8]
            )),
            "{warning}"
        );
        // 이 커밋으로 ready를 보냈으면 경고하지 않는다.
        let sent = |branch: &str, at: &str| Ok(branch == "feature" && at == head);
        assert_eq!(branch_warning(&repo, &feature, sent).await, Ok(None));
        // 병합한 뒤에는 경고하지 않는다.
        run(&repo, &["merge", "-q", "--no-ff", "-m", "m", "feature"]);
        assert_eq!(branch_warning(&repo, &feature, never).await, Ok(None));
    }

    #[tokio::test]
    async fn ownership_follows_the_registered_worktree_or_the_callers_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["commit", "-q", "--allow-empty", "-m", "a"]);
        let repo = std::path::PathBuf::from(run(&repo, &["rev-parse", "--show-toplevel"]));
        let main = repo.display().to_string();
        let feature = dir.path().join("feature");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                feature.to_str().unwrap(),
            ],
        );
        let feature = std::path::PathBuf::from(run(&feature, &["rev-parse", "--show-toplevel"]));
        let other = dir.path().join("other");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "other",
                other.to_str().unwrap(),
            ],
        );
        let other = std::path::PathBuf::from(run(&other, &["rev-parse", "--show-toplevel"]));
        std::fs::create_dir(feature.join("src")).unwrap();
        let feature_dir = feature.join("src").display().to_string();

        // 등록한 워크트리가 그 브랜치다.
        let own = owned_worktree(&repo, &feature.display().to_string(), "feature", &main).await;
        assert_eq!(own.unwrap().branch.as_deref(), Some("feature"));
        // 원래 체크아웃의 노드가 그 브랜치 워크트리 안에서 부른다.
        let moved = owned_worktree(&repo, &main, "feature", &feature_dir)
            .await
            .unwrap();
        assert_eq!(moved.path, feature.display().to_string());
        // 그 브랜치 워크트리 안에 다른 브랜치의 워크트리가 중첩돼도 거절한다.
        let nested = feature.join("nested");
        run(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "inner",
                nested.to_str().unwrap(),
            ],
        );
        let nested = nested.display().to_string();
        let refused = owned_worktree(&repo, &main, "feature", &nested).await;
        assert!(matches!(refused, Err(Refusal::NotOwner { .. })));
        // 작업 폴더가 다른 브랜치의 워크트리면 거절한다.
        let refused = owned_worktree(&repo, &main, "feature", &other.display().to_string()).await;
        assert_eq!(
            refused.unwrap_err(),
            Refusal::NotOwner {
                branch: "feature".into(),
                yours: Some("main".into())
            }
        );
    }
}
