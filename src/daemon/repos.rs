use super::*;

pub(super) fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/repos", get(list_repos).post(add_repo))
        .route("/api/repos/clone", post(clone_repo))
        .route("/api/repos/create", post(create_repo))
        .route("/api/repos/{id}", delete(remove_repo))
        .route("/api/folders", get(list_folders))
        .route(
            "/api/repos/{id}/worktrees",
            post(create_worktree).delete(delete_worktree),
        )
        .route("/api/repos/{id}/worktrees/changes", get(worktree_changes))
        .route("/api/repos/{id}/worktrees/name", put(rename_worktree))
        .route("/api/repos/{id}/commits", get(commits))
        .route("/api/repos/{id}/spots", delete(clear_spots))
        .route("/api/spots", get(list_spots).put(put_spot))
        .route("/api/spots/arranged", put(put_arranged))
        .route(
            "/api/sizes",
            get(list_sizes).put(put_sizes).delete(clear_sizes),
        )
}

/// [저장소 추가]. `path`는 데몬이 보는 폴더다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct NewRepo {
    pub path: String,
}

/// [저장소 추가] 「URL 복제」. `parent`는 데몬이 보는 폴더다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct CloneRepo {
    pub url: String,
    pub parent: String,
}

/// [저장소 추가] 「새 프로젝트」. `<parent>/<name>`을 만든다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct CreateRepo {
    pub parent: String,
    pub name: String,
}

#[derive(Deserialize)]
pub struct FolderQuery {
    pub path: Option<String>,
}

/// [새 워크트리]. `base`에서 새 브랜치 `branch`를 만든다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct NewWorktree {
    pub branch: String,
    pub base: String,
}

#[derive(Deserialize)]
pub struct WorktreePath {
    pub path: String,
}

/// [이름 바꾸기]. `path` 워크트리의 브랜치 이름을 `name`으로 바꾼다.
#[derive(Deserialize, TS)]
#[ts(export)]
pub struct RenameWorktree {
    pub path: String,
    pub name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteWorktree {
    pub path: String,
    #[serde(default)]
    pub delete_branch: bool,
}

/// 커밋 그래프 칸이 읽는 것: 한 구간의 커밋과, 워크트리마다 체크아웃한 커밋.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct CommitGraph {
    pub commits: Vec<Commit>,
    /// 참조의 지문(`commit_graph::version`). 같으면 이미 읽은 구간도 그대로다.
    pub version: String,
    pub worktrees: Vec<Worktree>,
}

async fn list_repos(State(app): State<Arc<App>>) -> Result<Json<Vec<RepoView>>, Problem> {
    let shared = app
        .repo_list
        .run(|| {
            let app = app.clone();
            async move {
                let mut views = Vec::new();
                for (id, path) in app.repos.list().map_err(|e| e.to_string())? {
                    views.push(app.repo_view(id, &path).await);
                }
                Ok(views)
            }
        })
        .await;
    match &*shared {
        Ok(views) => Ok(Json(views.clone())),
        Err(e) => Err(internal(e)),
    }
}

async fn add_repo(
    State(app): State<Arc<App>>,
    Json(req): Json<NewRepo>,
) -> Result<Json<RepoView>, Problem> {
    let id = app.repos.add(req.path.trim()).await?;
    Ok(Json(app.repo_view(id, &app.repos.path(id)?).await))
}

async fn clone_repo(
    State(app): State<Arc<App>>,
    Json(req): Json<CloneRepo>,
) -> Result<Json<RepoView>, Problem> {
    let id = app.repos.clone(&req.url, req.parent.trim()).await?;
    Ok(Json(app.repo_view(id, &app.repos.path(id)?).await))
}

async fn create_repo(
    State(app): State<Arc<App>>,
    Json(req): Json<CreateRepo>,
) -> Result<Json<RepoView>, Problem> {
    let id = app.repos.create(req.parent.trim(), &req.name).await?;
    Ok(Json(app.repo_view(id, &app.repos.path(id)?).await))
}

async fn remove_repo(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.repos.remove(RepoId(id))?;
    Ok(Json(()))
}

async fn list_folders(Query(req): Query<FolderQuery>) -> Result<Json<FolderList>, Problem> {
    let path = match req.path {
        Some(path) => path,
        None => worktree::loom_home()
            .map_err(internal)?
            .to_string_lossy()
            .into_owned(),
    };
    Ok(Json(worktree::folders(path.trim())?))
}

async fn create_worktree(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
    Json(req): Json<NewWorktree>,
) -> Result<Json<RepoView>, Problem> {
    let id = RepoId(id);
    let repo = app.repos.path(id)?;
    let workspace = app.workspace(&repo).await?;
    let unowned = |path: &str| app.graph.nodes_in(path).is_ok_and(|nodes| nodes.is_empty());
    let made = worktree::create(&repo, &repo, &workspace, &req.branch, &req.base, unowned).await?;
    app.setup(&Place {
        repo: id,
        worktree: made,
    })
    .await?;
    Ok(Json(app.repo_view(id, &repo).await))
}

async fn worktree_changes(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
    Query(req): Query<WorktreePath>,
) -> Result<Json<Vec<String>>, Problem> {
    let repo = app.repos.path(RepoId(id))?;
    match worktree::find(&repo, &req.path).await {
        Ok(listed) => Ok(Json(worktree::changes(&listed).await?)),
        Err(RepoProblem::NotInRepo(_)) => Ok(Json(Vec::new())),
        Err(e) => Err(e.into()),
    }
}

async fn rename_worktree(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
    Json(req): Json<RenameWorktree>,
) -> Result<Json<RepoView>, Problem> {
    let id = RepoId(id);
    let repo = app.repos.path(id)?;
    let listed = worktree::find(&repo, &req.path).await?;
    if let Some(branch) = &listed.branch
        && app.graph.open_round(id, branch).map_err(internal)?
    {
        return Err(RepoProblem::OpenRound(branch.clone()).into());
    }
    worktree::rename(&repo, &listed, &req.name).await?;
    Ok(Json(app.repo_view(id, &repo).await))
}

async fn delete_worktree(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
    Query(req): Query<DeleteWorktree>,
) -> Result<Json<RepoView>, Problem> {
    let id = RepoId(id);
    let repo = app.repos.path(id)?;
    let listed = match worktree::find(&repo, &req.path).await {
        Ok(listed) => Some(listed),
        Err(RepoProblem::NotInRepo(_)) => None,
        Err(e) => return Err(e.into()),
    };
    if listed.as_ref().is_some_and(|w| w.original) {
        return Err(RepoProblem::Original.into());
    }
    for node in app.graph.nodes_in(&req.path).map_err(internal)? {
        app.remove(node).map_err(internal)?;
    }
    app.spots.forget_worktree(id, &req.path).map_err(internal)?;
    app.graph.publish_view_changed();
    if let Some(listed) = listed {
        worktree::remove(&repo, &listed).await?;
        if let (true, Some(branch)) = (req.delete_branch, &listed.branch) {
            worktree::delete_branch(&repo, branch).await?;
        }
    }
    Ok(Json(app.repo_view(id, &repo).await))
}

async fn list_spots(State(app): State<Arc<App>>) -> Result<Json<Layout>, Problem> {
    Ok(Json(app.spots.layout().map_err(internal)?))
}

async fn put_spot(
    State(app): State<Arc<App>>,
    Json(placed): Json<Vec<Placed>>,
) -> Result<Json<()>, Problem> {
    app.spots.put(&placed).map_err(internal)?;
    app.graph.publish_view_changed();
    Ok(Json(()))
}

async fn put_arranged(
    State(app): State<Arc<App>>,
    Json(arranged): Json<Arranged>,
) -> Result<Json<()>, Problem> {
    app.spots.put_arranged(&arranged).map_err(internal)?;
    app.graph.publish_view_changed();
    Ok(Json(()))
}

async fn clear_spots(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
) -> Result<Json<()>, Problem> {
    app.spots.clear(RepoId(id)).map_err(internal)?;
    app.graph.publish_view_changed();
    Ok(Json(()))
}

async fn list_sizes(State(app): State<Arc<App>>) -> Result<Json<Vec<CardSize>>, Problem> {
    Ok(Json(app.spots.sizes().map_err(internal)?))
}

async fn put_sizes(
    State(app): State<Arc<App>>,
    Json(sizes): Json<Vec<CardSize>>,
) -> Result<Json<()>, Problem> {
    app.spots.put_sizes(&sizes).map_err(internal)?;
    app.graph.publish_view_changed();
    Ok(Json(()))
}

async fn clear_sizes(
    State(app): State<Arc<App>>,
    Json(nodes): Json<Vec<NodeId>>,
) -> Result<Json<()>, Problem> {
    app.spots.clear_sizes(&nodes).map_err(internal)?;
    app.graph.publish_view_changed();
    Ok(Json(()))
}

async fn commits(
    State(app): State<Arc<App>>,
    Path(id): Path<u32>,
    Query(range): Query<commit_graph::CommitRange>,
) -> Result<Json<CommitGraph>, Problem> {
    let repo = app.repos.path(RepoId(id))?;
    let read = tokio::try_join!(
        commit_graph::read(&repo, range),
        commit_graph::version(&repo)
    );
    let (commits, version) = match read {
        Ok(read) => read,
        Err(e) => {
            return Err(worktree::check(&repo)
                .await
                .err()
                .map_or_else(|| internal(e), Into::into));
        }
    };
    Ok(Json(CommitGraph {
        commits,
        version,
        worktrees: worktree::worktrees(&repo).await,
    }))
}
