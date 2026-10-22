//! 실행 위치(이 PC, WSL 배포판)마다 다른 값. 위치는 저장소 경로에서 나온다(`wsl::of`). 따로 저장하지 않는다.
//! 근거: docs/design.md 「데몬 하나」, 목업 docs/mockups/wsl-place.html.

use std::path::Path as FsPath;

use super::App;
use crate::worktree::{self, RepoId, RepoView, WslPlace};
use crate::wsl;

impl App {
    /// 화면에 보내는 저장소. WSL 저장소면 배포판과 `~/…` 경로를 붙인다.
    /// 저장소의 git도 그 배포판에서 돈다. 홈을 읽는 `wsl.exe`는 git이 켤 배포판을 켤 뿐이고, 한 번 읽으면 다시 부르지 않는다.
    pub(super) async fn repo_view(&self, id: RepoId, path: &FsPath) -> RepoView {
        let wsl = match wsl::of(path) {
            Some(at) => Some(WslPlace {
                path: match self.homes.get(&at.distro).await {
                    Ok(home) => wsl::tilde(&at.path, &home),
                    Err(_) => at.path,
                },
                distro: at.distro,
            }),
            None => None,
        };
        worktree::view(id, path, wsl).await
    }
}
