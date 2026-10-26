use std::path::Path;
use std::process::Command;

pub fn write_executable(path: &Path, script: &str) {
    let source = path.with_extension("src");
    std::fs::write(&source, script).unwrap();
    install(&source, path);
    std::fs::remove_file(&source).unwrap();
}

pub fn seal(path: &Path) {
    let sealed = path.with_extension("sealed");
    install(path, &sealed);
    std::fs::rename(&sealed, path).unwrap();
}

pub fn stale_socket(path: &Path) {
    drop(std::os::unix::net::UnixDatagram::bind(path).unwrap());
}

fn install(from: &Path, to: &Path) {
    let status = Command::new("install")
        .args(["-m", "755"])
        .arg(from)
        .arg(to)
        .status()
        .unwrap();
    assert!(status.success());
}
