use std::path::Path;
use std::process::Command;

/// 시험용 가짜 실행 파일을 만든다. 쓰기 fd를 다른 프로세스가 열어 두고 끝낸다.
/// 이 프로세스에서 쓰면 다른 시험 스레드의 fork가 그 fd를 물려받아, 곧바로 exec할 때
/// "Text file busy"(ETXTBSY)가 난다. `install`이 따로 쓰고 끝나면 이 프로세스에는 쓰기 fd가 없다.
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
