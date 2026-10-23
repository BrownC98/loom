use std::path::Path;
use std::process::Command;

pub fn write_executable(path: &Path, script: &str) {
    let source = path.with_extension("src");
    std::fs::write(&source, script).unwrap();
    let status = Command::new("install")
        .args(["-m", "755"])
        .arg(&source)
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::remove_file(&source).unwrap();
}
