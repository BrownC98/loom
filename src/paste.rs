//! 터미널에 붙여넣은 그림을 파일로 남긴다. 화면이 그 경로를 터미널에 붙여넣으면 에이전트 CLI가 첨부로 바꾼다.
//! 그림은 브라우저 클립보드에 있다. 노드가 도는 기계의 클립보드 도구(`wl-paste`·`xclip`)에 기대지 않는다.
//! 근거: docs/design.md 「저장」.

use std::path::PathBuf;

/// 한 번에 받는 그림의 상한. 4K 화면 캡처 PNG도 들어가게 넉넉히 둔다. axum 기본값(2MB)은 작다.
pub const LIMIT_BYTES: usize = 32 * 1024 * 1024;

/// 형식을 확인한 그림.
pub struct Image<'a> {
    extension: &'static str,
    bytes: &'a [u8],
}

impl<'a> Image<'a> {
    /// Claude Code·Codex·Cursor CLI가 모두 첨부로 읽는 형식만 받는다. 아니면 없다.
    pub fn new(mime: &str, bytes: &'a [u8]) -> Option<Self> {
        let extension = match mime {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => return None,
        };
        Some(Self { extension, bytes })
    }

    /// 새 파일에 쓰고 절대 경로를 돌려준다. 파일은 OS 임시 폴더에 두므로 OS가 지운다.
    pub fn save(&self) -> std::io::Result<PathBuf> {
        let dir = std::env::temp_dir().join("loom-paste");
        std::fs::create_dir_all(&dir)?;
        let name = jiff::Timestamp::now().as_nanosecond();
        let path = dir.join(format!("{name}.{}", self.extension));
        std::fs::write(&path, self.bytes)?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_supported_images_and_refuses_others() {
        let path = Image::new("image/jpeg", b"jpg").unwrap().save().unwrap();
        assert!(path.is_absolute());
        assert_eq!(path.extension().unwrap(), "jpg");
        assert_eq!(std::fs::read(&path).unwrap(), b"jpg");
        std::fs::remove_file(path).unwrap();
        assert!(Image::new("image/tiff", b"x").is_none());
    }
}
