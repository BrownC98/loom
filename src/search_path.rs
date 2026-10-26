use std::ffi::OsString;
use std::path::PathBuf;

pub fn find(name: &str) -> Option<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_default();
    which::which_in(name, Some(current()), cwd).ok()
}

#[cfg(unix)]
pub fn current() -> OsString {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    match login::path() {
        Some(login) => joined(login, std::env::split_paths(&inherited)),
        None => inherited,
    }
}

#[cfg(windows)]
pub fn current() -> OsString {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let fresh = [machine(), user()].into_iter().flatten();
    joined(
        inherited,
        fresh.flat_map(|v| std::env::split_paths(&v).collect::<Vec<_>>()),
    )
}

#[cfg(unix)]
mod login {
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::OsStringExt;
    use std::sync::{Mutex, PoisonError};
    use std::time::{Duration, Instant};

    const READ_LIMIT: Duration = Duration::from_secs(3);
    const REUSE_FOR: Duration = Duration::from_secs(2);
    const MARK: &str = "\u{1e}loom-path:";
    const END: u8 = 0x1e;

    static LAST: Mutex<Option<(Instant, Option<OsString>)>> = Mutex::new(None);

    pub fn path() -> Option<OsString> {
        let shell = std::env::var_os("SHELL")?;
        let mut last = LAST.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((at, path)) = &*last
            && at.elapsed() < REUSE_FOR
        {
            return path.clone();
        }
        let path = read(&shell, READ_LIMIT);
        *last = Some((Instant::now(), path.clone()));
        path
    }

    pub(super) fn read(shell: &OsStr, limit: Duration) -> Option<OsString> {
        let mut cmd = std::process::Command::new(shell);
        cmd.args(["-ilc", &format!("printf '{MARK}%s\\036' \"$PATH\"")]);
        let out = crate::timed::output(cmd, limit)?;
        let start = out
            .windows(MARK.len())
            .rposition(|w| w == MARK.as_bytes())?
            + MARK.len();
        let len = out[start..].iter().position(|&b| b == END)?;
        Some(OsString::from_vec(out[start..start + len].to_vec())).filter(|p| !p.is_empty())
    }
}

#[cfg(windows)]
const MACHINE_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
#[cfg(windows)]
const USER_KEY: &str = "Environment";
#[cfg(all(windows, debug_assertions))]
const TEST_USER_PATH: &str = "TEST_LOOM_USER_PATH";

#[cfg(windows)]
fn machine() -> Option<OsString> {
    registry::path(registry::HKEY_LOCAL_MACHINE, MACHINE_KEY)
}

#[cfg(windows)]
fn user() -> Option<OsString> {
    #[cfg(debug_assertions)]
    if let Some(file) = std::env::var_os(TEST_USER_PATH) {
        return std::fs::read_to_string(file).ok().map(|s| s.trim().into());
    }
    registry::path(registry::HKEY_CURRENT_USER, USER_KEY)
}

fn joined(first: OsString, more: impl Iterator<Item = std::path::PathBuf>) -> OsString {
    let mut dirs: Vec<_> = std::env::split_paths(&first).collect();
    let before = dirs.len();
    for dir in more {
        if !dir.as_os_str().is_empty() && !dirs.iter().any(|d| same(d, &dir)) {
            dirs.push(dir);
        }
    }
    if dirs.len() == before {
        return first;
    }
    std::env::join_paths(dirs).unwrap_or(first)
}

#[cfg(windows)]
fn same(a: &std::path::Path, b: &std::path::Path) -> bool {
    let key = |p: &std::path::Path| {
        p.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    key(a) == key(b)
}

#[cfg(unix)]
fn same(a: &std::path::Path, b: &std::path::Path) -> bool {
    a.components().eq(b.components())
}

#[cfg(windows)]
mod registry {
    use std::ffi::{OsString, c_void};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    pub type Hkey = isize;
    pub const HKEY_CURRENT_USER: Hkey = 0x8000_0001_u32 as i32 as isize;
    pub const HKEY_LOCAL_MACHINE: Hkey = 0x8000_0002_u32 as i32 as isize;
    const RRF_RT_REG_SZ: u32 = 0x0000_0002;
    const VALUE: &str = "Path";

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegGetValueW(
            key: Hkey,
            sub_key: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut c_void,
            len: *mut u32,
        ) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain([0]).collect()
    }

    pub fn path(key: Hkey, sub_key: &str) -> Option<OsString> {
        let sub_key = wide(sub_key);
        let value = wide(VALUE);
        let get = |data: *mut c_void, len: &mut u32| unsafe {
            RegGetValueW(
                key,
                sub_key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                data,
                len,
            )
        };
        let mut len = 0;
        if get(std::ptr::null_mut(), &mut len) != 0 {
            return None;
        }
        let mut buf = vec![0u16; (len as usize).div_ceil(2)];
        let mut len = (buf.len() * 2) as u32;
        if get(buf.as_mut_ptr().cast(), &mut len) != 0 {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(OsString::from_wide(&buf[..end]))
    }
}
