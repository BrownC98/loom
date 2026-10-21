//! 터미널에 붙여넣거나 끌어다 놓은 파일을 데몬 기계에 남긴다. 화면이 그 경로를 터미널에 붙여넣으면 에이전트 CLI가 읽거나 첨부한다.
//! 파일은 브라우저에 있다. 브라우저는 원래 경로를 알려 주지 않고, 데몬(WSL)은 Windows 경로를 읽지 못한다.
//! 근거: docs/design.md 「저장」, 목업 docs/mockups/terminal-drop.html.

use std::path::{Path, PathBuf};

/// 파일 하나의 상한. 4K 화면 캡처 PNG도 들어가게 넉넉히 둔다. axum 기본값(2MB)은 작다.
/// 화면(`web/src/api.ts`의 `PASTE_LIMIT_BYTES`)이 올리기 전에 같은 값으로 거른다.
pub const LIMIT_BYTES: usize = 32 * 1024 * 1024;

/// 이름이 `name`인 새 파일에 쓰고 절대 경로를 돌려준다. 파일은 OS 임시 폴더에 두므로 OS가 지운다.
/// 이름이 같은 파일끼리 겹치지 않게 파일마다 새 폴더에 둔다. 에이전트가 원래 이름을 보게 이름은 바꾸지 않는다.
pub fn save(name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    // 경로 조각(`../`)으로 폴더 밖에 쓰지 못하게 마지막 이름만 쓴다.
    let name = Path::new(name).file_name().unwrap_or("file".as_ref());
    let dir = std::env::temp_dir()
        .join("loom-paste")
        .join(jiff::Timestamp::now().as_nanosecond().to_string());
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_under_original_name_inside_its_own_folder() {
        let path = save("보고서 초안.pdf", b"pdf").unwrap();
        assert!(path.is_absolute());
        assert_eq!(path.file_name().unwrap(), "보고서 초안.pdf");
        assert_eq!(std::fs::read(&path).unwrap(), b"pdf");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();

        let path = save("../../escape.txt", b"x").unwrap();
        assert_eq!(
            path.parent().unwrap().parent().unwrap(),
            std::env::temp_dir().join("loom-paste")
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
