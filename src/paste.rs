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

pub fn read(path: &Path) -> std::io::Result<Vec<u8>> {
    let size = std::fs::metadata(path)?.len();
    if size > LIMIT_BYTES as u64 {
        return Err(std::io::Error::other(format!(
            "{size} bytes, over {LIMIT_BYTES}"
        )));
    }
    std::fs::read(path)
}
