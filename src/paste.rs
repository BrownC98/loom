use std::path::{Path, PathBuf};

pub const LIMIT_BYTES: usize = 32 * 1024 * 1024;

pub fn save(temp: &Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let name = Path::new(name).file_name().unwrap_or("file".as_ref());
    let dir = temp
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
        let path = save(&std::env::temp_dir(), "보고서 초안.pdf", b"pdf").unwrap();
        assert!(path.is_absolute());
        assert_eq!(path.file_name().unwrap(), "보고서 초안.pdf");
        assert_eq!(std::fs::read(&path).unwrap(), b"pdf");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();

        let path = save(&std::env::temp_dir(), "../../escape.txt", b"x").unwrap();
        assert_eq!(
            path.parent().unwrap().parent().unwrap(),
            std::env::temp_dir().join("loom-paste")
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
