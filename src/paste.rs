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
