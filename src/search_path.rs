use std::ffi::OsString;
use std::path::PathBuf;

pub fn find(name: &str) -> Option<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_default();
    which::which_in(name, Some(current()), cwd).ok()
}

#[cfg(unix)]
pub fn current() -> OsString {
    std::env::var_os("PATH").unwrap_or_default()
}

#[cfg(windows)]
pub fn current() -> OsString {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let fresh = [machine(), user()].into_iter().flatten();
    with_fresh(
        inherited,
        fresh.flat_map(|v| std::env::split_paths(&v).collect::<Vec<_>>()),
    )
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

#[cfg(windows)]
fn with_fresh(inherited: OsString, fresh: impl Iterator<Item = std::path::PathBuf>) -> OsString {
    let mut dirs: Vec<_> = std::env::split_paths(&inherited).collect();
    let before = dirs.len();
    for dir in fresh {
        if !dir.as_os_str().is_empty() && !dirs.iter().any(|d| same(d, &dir)) {
            dirs.push(dir);
        }
    }
    if dirs.len() == before {
        return inherited;
    }
    std::env::join_paths(dirs).unwrap_or(inherited)
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_machine_path_is_read_with_its_variables_expanded() {
        let machine = machine().expect("machine PATH");
        let system = std::env::var("SystemRoot").unwrap().to_lowercase();
        let dirs: Vec<_> = std::env::split_paths(&machine).collect();
        assert!(
            dirs.iter()
                .any(|d| same(d, &PathBuf::from(format!(r"{system}\system32")))),
            "{machine:?}"
        );
        assert!(!machine.to_string_lossy().contains('%'), "{machine:?}");
    }

    #[test]
    fn fresh_folders_come_after_the_inherited_ones_without_doubles() {
        let joined = with_fresh(
            r"C:\a;C:\B\".into(),
            [r"c:\b", r"C:\new", r"C:\A\", ""]
                .map(PathBuf::from)
                .into_iter(),
        );
        assert_eq!(joined, OsString::from(r"C:\a;C:\B\;C:\new"));
    }

    #[test]
    fn no_fresh_folder_keeps_the_inherited_path_as_it_is() {
        let inherited = OsString::from(r"C:\a;;C:\b");
        assert_eq!(
            with_fresh(inherited.clone(), [r"C:\a"].map(PathBuf::from).into_iter()),
            inherited
        );
    }
}
