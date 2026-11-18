use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::SystemTime;

static STARTED: OnceLock<Option<(PathBuf, SystemTime)>> = OnceLock::new();

fn file() -> Option<(PathBuf, SystemTime)> {
    let exe = std::env::current_exe().ok()?;
    let modified = std::fs::metadata(&exe).ok()?.modified().ok()?;
    Some((exe, modified))
}

pub fn remember() {
    STARTED.get_or_init(file);
}

pub fn replaced() -> bool {
    STARTED
        .get()
        .and_then(Option::as_ref)
        .is_some_and(|(exe, started)| {
            std::fs::metadata(exe)
                .and_then(|meta| meta.modified())
                .is_ok_and(|now| now != *started)
        })
}
