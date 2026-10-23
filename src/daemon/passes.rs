//! 폰 앱의 파일 링크를 폰 브라우저(Chrome)로 연다. 앱 안의 WebView는 줌이 되지 않는다(2026-10-23 사람 "헤드리스 브라우저고 줌인아웃도 안됨 크롬이나 기타 모바일 웹브라우저에서 볼수없나").
//! 폰 브라우저에는 기기 쿠키가 없다. 그래서 짝지은 화면이 파일 하나의 통행증을 받고, 그 주소는 쿠키 없이 그 파일의 폴더만 읽는다.
//! 주소에 토큰이 있어서 HTML의 상대 주소(옆 그림)도 같은 통행증으로 온다.

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

/// 통행증 주소의 앞. 사람 문(`origin::human_site`)이 이 GET만 쿠키 없이 받는다.
pub const PASS_PREFIX: &str = "/api/files/pass/";
/// 통행증의 수명. 주소는 폰 브라우저의 기록에 남는다. 사람이 문서 하나를 읽는 동안만 살린다.
/// 끝난 뒤 그 탭을 다시 읽으면 404다. 링크를 다시 누르면 새 통행증을 받는다.
const PASS_TTL: Duration = Duration::from_secs(30 * 60);

/// 파일 이름에서 그대로 두는 글자. 브라우저의 `encodeURIComponent`와 같다(주소 줄에 `r.html`로 보인다).
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

/// 기억에만 둔다. 데몬이 다시 뜨면 모든 통행증이 끝난다.
#[derive(Default)]
pub struct Passes(Mutex<HashMap<String, Pass>>);

struct Pass {
    folder: PathBuf,
    until: Instant,
}

impl Passes {
    /// `folder` 아래를 읽는 새 토큰.
    fn issue(&self, folder: PathBuf) -> String {
        let token = crate::devices::random_token();
        let now = Instant::now();
        let mut passes = self.0.lock().expect("passes lock");
        passes.retain(|_, pass| pass.until > now);
        passes.insert(
            token.clone(),
            Pass {
                folder,
                until: now + PASS_TTL,
            },
        );
        token
    }

    /// 살아 있는 토큰이고 `file`이 그 폴더 안의 파일이면 실제 경로.
    fn resolve(&self, token: &str, file: &str) -> Option<PathBuf> {
        let folder = {
            let passes = self.0.lock().expect("passes lock");
            let pass = passes.get(token).filter(|p| p.until > Instant::now())?;
            pass.folder.clone()
        };
        inside(&folder, file)
    }
}

/// `folder` 안의 상대 경로 `file`의 실제 경로. 폴더 안의 심볼릭 링크가 폴더 밖을 가리키면 내지 않는다(2026-10-23 검토).
/// 통행증과 문서 옆 파일(`daemon::doc_sibling`)이 쓴다.
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
    pass_url(&app, &path).await
}

/// 질문·완료 문서(`--doc`)의 통행증. 사람은 목업을 받은편지함에서 연다(2026-10-23 사람 "여전히 헤드리스 브라우저임").
async fn issue_doc(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> Result<Json<String>, DocProblem> {
    let path = doc_file(&id)?;
    Ok(pass_url(&app, &path).await?)
}

/// 파일 하나의 통행증 주소. Serve가 켜져 있으면 tailnet 이름으로 준다. 폰 문은 스스로 서명한 인증서라 브라우저가 경고한다.
/// 꺼져 있으면 경로만 준다. 화면이 자기 주소에 붙인다.
async fn pass_url(app: &App, path: &FsPath) -> Result<Json<String>, Problem> {
    let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(internal(format!("파일 경로가 아니다: {}", path.display())));
    };
    let token = app.passes.issue(folder.to_path_buf());
    let name = utf8_percent_encode(&name.to_string_lossy(), NAME_KEEPS).to_string();
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
        let token = passes.issue(folder.clone());
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
    }

    /// 폴더 안의 링크가 폴더 밖을 가리키면 내지 않는다.
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
        let token = passes.issue(PathBuf::from("/docs"));
        passes.0.lock().unwrap().get_mut(&token).unwrap().until = Instant::now();
        assert_eq!(passes.resolve(&token, "a.html"), None);
    }
}
