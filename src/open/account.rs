//! 이 loom이 OS 계정의 데이터 폴더를 쓰는가. 쓰지 않으면 시험 loom이다(`super::launch`).

use std::path::{Path, PathBuf};

/// 데이터 폴더(`db::data_dir`)의 바탕이 OS 계정의 기본 폴더인가. 환경 변수(`HOME`, `APPDATA`)가 아니라 OS가 아는 계정 폴더와 견준다.
/// 시험 데몬은 늘 다른 `HOME`·`APPDATA`로 뜬다(AGENTS.md 「개발 환경」). 사람의 데몬(`scripts/host.sh`, 데스크톱 앱)은 바꾸지 않는다.
/// 포트로 가르지 않는다: 7420을 다른 프로그램이 쓰면 사람의 데몬도 다른 포트로 뜬다(`launch::ANY_PORT`).
pub fn is_account_folder() -> bool {
    match (env_base(), account_base()) {
        (Some(env), Some(account)) => same_dir(&env, &account),
        _ => false,
    }
}

#[cfg(windows)]
fn env_base() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

#[cfg(not(windows))]
fn env_base() -> Option<PathBuf> {
    std::env::home_dir()
}

/// Windows 경로는 대소문자와 `/`·`\`를 가리지 않는다.
#[cfg(windows)]
fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// 심볼릭 링크로 닿은 홈(`/home` → `/var/home`)도 같은 폴더다.
#[cfg(not(windows))]
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// 계정의 Roaming AppData. `SHGetKnownFolderPath`는 `APPDATA` 변수가 아니라 계정 설정(폴더 리디렉션 포함)을 읽는다.
#[cfg(windows)]
fn account_base() -> Option<PathBuf> {
    use std::ffi::{OsString, c_void};
    use std::os::windows::ffi::OsStringExt;
    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);
    /// FOLDERID_RoamingAppData {3EB685DB-65F9-4CF6-A03A-E3EF65729F3D}
    const ROAMING_APP_DATA: Guid = Guid(
        0x3EB6_85DB,
        0x65F9,
        0x4CF6,
        [0xA0, 0x3A, 0xE3, 0xEF, 0x65, 0x72, 0x9F, 0x3D],
    );
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHGetKnownFolderPath(
            id: *const Guid,
            flags: u32,
            token: *mut c_void,
            path: *mut *mut u16,
        ) -> i32;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoTaskMemFree(p: *mut c_void);
    }
    let mut path = std::ptr::null_mut();
    // SAFETY: 성공하든 실패하든 `path`는 이 함수가 할당한 글자열이고 `CoTaskMemFree`로 푼다(Win32 문서).
    let ok =
        unsafe { SHGetKnownFolderPath(&ROAMING_APP_DATA, 0, std::ptr::null_mut(), &mut path) } == 0;
    let found = ok.then(|| {
        // SAFETY: 성공하면 `path`는 NUL로 끝나는 UTF-16 글자열이다.
        let len = (0..).take_while(|&i| unsafe { *path.add(i) } != 0).count();
        PathBuf::from(OsString::from_wide(unsafe {
            std::slice::from_raw_parts(path, len)
        }))
    });
    unsafe { CoTaskMemFree(path.cast()) };
    found
}

/// 계정의 홈 폴더(passwd). `HOME` 변수는 보지 않는다.
#[cfg(unix)]
fn account_base() -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    let mut buf = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: passwd 구조체는 0으로 채워도 되는 C 구조체다.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found = std::ptr::null_mut();
    let rc = loop {
        // SAFETY: `buf`가 `entry`의 글자열을 담고, 둘 다 이 함수 안에서만 쓴다.
        let rc = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                &mut entry,
                buf.as_mut_ptr(),
                buf.len(),
                &mut found,
            )
        };
        // 항목이 버퍼보다 크면(NSS 디렉터리 계정) 키워서 다시 읽는다.
        if rc != libc::ERANGE || buf.len() >= 1 << 20 {
            break rc;
        }
        buf.resize(buf.len() * 2, 0);
    };
    if rc != 0 || found.is_null() || entry.pw_dir.is_null() {
        return None;
    }
    // SAFETY: 성공하면 `pw_dir`은 `buf` 안의 NUL로 끝나는 글자열이다.
    let dir = unsafe { CStr::from_ptr(entry.pw_dir) };
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes())))
}

#[cfg(not(any(unix, windows)))]
fn account_base() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 시험 데몬을 가르는 기준(`is_account_folder`)은 OS가 아는 계정 폴더가 있어야 선다.
    #[test]
    fn the_account_folder_comes_from_the_os() {
        let dir = account_base().expect("account folder");
        assert!(dir.is_absolute(), "{}", dir.display());
        assert!(!same_dir(
            &dir,
            &std::env::temp_dir().join("loom-test-home")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_folders_ignore_case_and_slashes() {
        assert!(same_dir(
            std::path::Path::new(r"C:\Users\Me\AppData\Roaming"),
            std::path::Path::new("c:/users/me/appdata/roaming/")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_home_is_the_same_folder() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("var-home");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("home");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(same_dir(&link, &real));
        assert!(!same_dir(&link, dir.path()));
    }
}
