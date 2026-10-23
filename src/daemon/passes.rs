use std::collections::HashMap;
use std::path::{Component, Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use super::{App, DocProblem, OpenFile, Problem, doc_file, host_path, internal, phone, serve_file};

pub const PASS_PREFIX: &str = "/api/files/pass/";
const PASS_TTL: Duration = Duration::from_secs(30 * 60);

const NAME_KEEPS: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

#[derive(Default)]
pub struct Passes(Mutex<HashMap<String, Pass>>);

struct Pass {
    folder: PathBuf,
    only: Option<String>,
    until: Instant,
}

#[derive(Clone, Copy)]
enum Reach {
    Folder,
    File,
}

impl Passes {
    fn issue(&self, folder: PathBuf, only: Option<String>) -> String {
        let token = crate::devices::random_token();
        let now = Instant::now();
        let mut passes = self.0.lock().expect("passes lock");
        passes.retain(|_, pass| pass.until > now);
        passes.insert(
            token.clone(),
            Pass {
                folder,
                only,
                until: now + PASS_TTL,
            },
        );
        token
    }

    fn resolve(&self, token: &str, file: &str) -> Option<PathBuf> {
        let folder = {
            let passes = self.0.lock().expect("passes lock");
            let pass = passes.get(token).filter(|p| p.until > Instant::now())?;
            if pass.only.as_deref().is_some_and(|only| only != file) {
                return None;
            }
            pass.folder.clone()
        };
        inside(&folder, file)
    }
}

pub(super) fn inside(folder: &FsPath, file: &str) -> Option<PathBuf> {
    let relative = FsPath::new(file);
    if file.is_empty()
        || !relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let folder = std::fs::canonicalize(folder).ok()?;
    let real = std::fs::canonicalize(folder.join(relative)).ok()?;
    real.starts_with(&folder).then_some(real)
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/files/pass", post(issue))
        .route("/api/files/pass/{token}/{*file}", get(read))
        .route("/api/docs/{id}/pass", post(issue_doc))
}

async fn issue(
    State(app): State<Arc<App>>,
    Json(req): Json<OpenFile>,
) -> Result<Json<String>, Problem> {
    let path = PathBuf::from(host_path(&app, req.node, req.path)?);
    pass_url(&app, &path, Reach::Folder).await
}

async fn issue_doc(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> Result<Json<String>, DocProblem> {
    let doc = doc_file(&id)?;
    let reach = match doc.folder {
        Some(_) => Reach::Folder,
        None => Reach::File,
    };
    Ok(pass_url(&app, &doc.path, reach).await?)
}

async fn pass_url(app: &App, path: &FsPath, reach: Reach) -> Result<Json<String>, Problem> {
    let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(internal(format!("파일 경로가 아니다: {}", path.display())));
    };
    let name = name.to_string_lossy();
    let only = match reach {
        Reach::Folder => None,
        Reach::File => Some(name.clone().into_owned()),
    };
    let token = app.passes.issue(folder.to_path_buf(), only);
    let name = utf8_percent_encode(&name, NAME_KEEPS).to_string();
    let path = format!("{PASS_PREFIX}{token}/{name}");
    Ok(Json(match phone::serve_host(app.port).await {
        Some(host) => format!("https://{host}{path}"),
        None => path,
    }))
}

async fn read(
    State(app): State<Arc<App>>,
    Path((token, file)): Path<(String, String)>,
    request: axum::extract::Request,
) -> Result<Response, Problem> {
    match app.passes.resolve(&token, &file) {
        Some(path) => serve_file(&path, request).await,
        None => Ok(StatusCode::NOT_FOUND.into_response()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_reads_only_inside_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("report");
        std::fs::create_dir_all(folder.join("img")).unwrap();
        std::fs::write(folder.join("index.html"), "").unwrap();
        std::fs::write(folder.join("img/a.png"), "").unwrap();
        std::fs::write(dir.path().join("secret"), "").unwrap();
        let real = std::fs::canonicalize(&folder).unwrap();
        let passes = Passes::default();
        let token = passes.issue(folder.clone(), None);
        assert_eq!(
            passes.resolve(&token, "index.html"),
            Some(real.join("index.html"))
        );
        assert_eq!(
            passes.resolve(&token, "img/a.png"),
            Some(real.join("img").join("a.png"))
        );
        for file in [
            "",
            "../secret",
            "img/../../secret",
            "/etc/passwd",
            "./a",
            "none.html",
        ] {
            assert_eq!(passes.resolve(&token, file), None, "{file}");
        }
        assert_eq!(passes.resolve("other", "index.html"), None);

        let one = passes.issue(folder.clone(), Some("index.html".into()));
        assert_eq!(
            passes.resolve(&one, "index.html"),
            Some(real.join("index.html"))
        );
        assert_eq!(passes.resolve(&one, "img/a.png"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_folder_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("doc");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "x").unwrap();
        std::fs::write(folder.join("a.html"), "").unwrap();
        std::os::unix::fs::symlink(&outside, folder.join("out")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), folder.join("s.txt")).unwrap();
        assert_eq!(inside(&folder, "out/secret.txt"), None);
        assert_eq!(inside(&folder, "s.txt"), None);
        assert!(inside(&folder, "a.html").is_some());
    }

    #[test]
    fn pass_ends_after_ttl() {
        let passes = Passes::default();
        let token = passes.issue(PathBuf::from("/docs"), None);
        passes.0.lock().unwrap().get_mut(&token).unwrap().until = Instant::now();
        assert_eq!(passes.resolve(&token, "a.html"), None);
    }
}
