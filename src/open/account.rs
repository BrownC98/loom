use std::path::{Path, PathBuf};

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

#[cfg(not(windows))]
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(windows)]
fn account_base() -> Option<PathBuf> {
    use std::ffi::{OsString, c_void};
    use std::os::windows::ffi::OsStringExt;
    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);
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
    let ok =
        unsafe { SHGetKnownFolderPath(&ROAMING_APP_DATA, 0, std::ptr::null_mut(), &mut path) } == 0;
    let found = ok.then(|| {
        let len = (0..).take_while(|&i| unsafe { *path.add(i) } != 0).count();
        PathBuf::from(OsString::from_wide(unsafe {
            std::slice::from_raw_parts(path, len)
        }))
    });
    unsafe { CoTaskMemFree(path.cast()) };
    found
}

#[cfg(unix)]
fn account_base() -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    let mut buf = vec![0 as libc::c_char; 16 * 1024];
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found = std::ptr::null_mut();
    let rc = loop {
        let rc = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                &mut entry,
                buf.as_mut_ptr(),
                buf.len(),
                &mut found,
            )
        };
        if rc != libc::ERANGE || buf.len() >= 1 << 20 {
            break rc;
        }
        buf.resize(buf.len() * 2, 0);
    };
    if rc != 0 || found.is_null() || entry.pw_dir.is_null() {
        return None;
    }
    let dir = unsafe { CStr::from_ptr(entry.pw_dir) };
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes())))
}

#[cfg(not(any(unix, windows)))]
fn account_base() -> Option<PathBuf> {
    None
}
