use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use portable_pty::CommandBuilder;

#[derive(Clone, Debug, PartialEq)]
pub struct WslPath {
    host: String,
    pub distro: String,
    pub path: String,
}

impl WslPath {
    pub fn unc(&self, linux: &str) -> String {
        format!(
            "//{}/{}{}",
            self.host,
            self.distro,
            linux.trim_end_matches('/')
        )
    }

    pub fn shell_path(&self, linux: &str) -> String {
        self.unc(linux).replace('/', "\\")
    }
}

pub fn location(path: &str) -> Option<WslPath> {
    let path = path.replace('/', "\\");
    let rest = path
        .strip_prefix(r"\\?\UNC\")
        .or_else(|| path.strip_prefix(r"\\"))?;
    let mut parts = rest.split('\\').filter(|p| !p.is_empty());
    let host = parts.next()?;
    if !host.eq_ignore_ascii_case("wsl.localhost") && !host.eq_ignore_ascii_case("wsl$") {
        return None;
    }
    let host = host.to_owned();
    let distro = parts.next()?.to_owned();
    let path = parts.fold(String::new(), |acc, p| acc + "/" + p);
    Some(WslPath {
        host,
        distro,
        path: if path.is_empty() { "/".into() } else { path },
    })
}

pub fn of(dir: &Path) -> Option<WslPath> {
    if !cfg!(windows) {
        return None;
    }
    dir.to_str().and_then(location)
}

pub fn windows_path(distro: &str, linux: &str) -> String {
    format!(
        r"\\wsl.localhost\{distro}{}",
        linux.trim_end_matches('/').replace('/', r"\")
    )
}

pub fn tilde(path: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    match path.strip_prefix(home) {
        Some("") => "~".into(),
        Some(rest) if rest.starts_with('/') && !home.is_empty() => format!("~{rest}"),
        _ => path.into(),
    }
}

pub async fn distros() -> Vec<String> {
    list(&["-l", "-q"]).await
}

pub async fn running() -> Vec<String> {
    list(&["-l", "--running", "-q"]).await
}

async fn list(args: &[&str]) -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let out = tokio::process::Command::new("wsl.exe")
        .args(args)
        .env("WSL_UTF8", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .await;
    match out {
        Ok(out) if out.status.success() => read_list(&out.stdout),
        _ => Vec::new(),
    }
}

fn read_list(bytes: &[u8]) -> Vec<String> {
    let text = if bytes.contains(&0) {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    text.lines()
        .map(|l| l.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}'))
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

#[derive(Default)]
pub struct Homes(Mutex<HashMap<String, String>>);

impl Homes {
    pub async fn get(&self, distro: &str) -> Result<String, InsideError> {
        if let Some(home) = self.0.lock().unwrap().get(distro) {
            return Ok(home.clone());
        }
        let out = tokio::process::Command::new("wsl.exe")
            .args(["-d", distro, "-e", "sh", "-c", r#"printf '\036%s' "$HOME""#])
            .env("WSL_UTF8", "1")
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(|e| InsideError::Start {
                distro: distro.into(),
                cause: e.to_string(),
            })?;
        let text = String::from_utf8_lossy(&out.stdout);
        let home = match text.split_once(SEP) {
            Some((_, home)) if home.starts_with('/') => home.to_owned(),
            _ => {
                return Err(InsideError::Start {
                    distro: distro.into(),
                    cause: String::from_utf8_lossy(&out.stderr).trim().into(),
                });
            }
        };
        self.0.lock().unwrap().insert(distro.into(), home.clone());
        Ok(home)
    }
}

pub fn from_caller(path: std::path::PathBuf) -> std::path::PathBuf {
    linux_at(path, caller())
}

pub fn from_node(path: PathBuf, folder: &Path) -> PathBuf {
    linux_at(path, of(folder))
}

fn linux_at(path: PathBuf, at: Option<WslPath>) -> PathBuf {
    let linux = path
        .to_str()
        .filter(|p| p.starts_with('/') && !p.starts_with("//"));
    match (linux, at) {
        (Some(p), Some(at)) => at.unc(p).into(),
        _ => path,
    }
}

pub fn for_caller(path: &Path) -> String {
    let text = path.display().to_string();
    match (caller(), location(&text)) {
        (Some(at), Some(inside)) if at.distro == inside.distro => inside.path,
        _ => text,
    }
}

pub fn command(dir: &str, program: &str) -> std::process::Command {
    let at = location(dir).filter(|_| cfg!(windows)).or_else(|| {
        let at = caller().filter(|_| dir.starts_with('/') && !dir.starts_with("//"))?;
        Some(WslPath {
            path: dir.to_owned(),
            ..at
        })
    });
    launch(at.as_ref(), dir, program)
}

fn launch(at: Option<&WslPath>, dir: &str, program: &str) -> std::process::Command {
    match at {
        Some(at) => {
            let mut cmd = std::process::Command::new("wsl.exe");
            cmd.args([
                "-d",
                &at.distro,
                "--cd",
                &at.path,
                "-e",
                "bash",
                "-lc",
                r#"exec "$0" "$@""#,
                program,
            ]);
            cmd
        }
        None => {
            let mut cmd = std::process::Command::new(program);
            cmd.current_dir(dir);
            cmd
        }
    }
}

fn caller() -> Option<WslPath> {
    std::env::current_dir().ok().and_then(|dir| of(&dir))
}

pub async fn end_hosts(distro: &str, home: &str, servers: &[u32]) -> Result<(), String> {
    let script = r#"home=$1; shift; servers=" $* " roots=
for d in /proc/[0-9]*; do
  pid=${d#/proc/}
  case $servers in *" $pid "*) ;; *) [ "$(tr '\0' '\n' <"$d/cmdline" 2>/dev/null | sed -n 2p)" = pty-host ] || continue ;; esac
  tr '\0' '\n' <"$d/environ" 2>/dev/null | grep -qxF "HOME=$home" || continue
  roots="$roots $pid"
done
[ -n "$roots" ] || exit 0
all=$(ps -eo pid=,ppid= | awk -v roots="$roots" 'BEGIN { n = split(roots, r, " "); for (i = 1; i <= n; i++) keep[r[i]] = 1 }
  { parent[$1] = $2 }
  END { do { grew = 0; for (p in parent) if (!(p in keep) && (parent[p] in keep)) { keep[p] = 1; grew = 1 } } while (grew); for (p in keep) print p }')
kill -TERM $all 2>/dev/null
sleep 2
kill -KILL $all 2>/dev/null
exit 0"#;
    let out = tokio::process::Command::new("wsl.exe")
        .args(["-d", distro, "-e", "sh", "-c", script, "sh", home])
        .args(servers.iter().map(u32::to_string))
        .env("WSL_UTF8", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| e.to_string())?;
    match out.status.success() {
        true => Ok(()),
        false => Err(String::from_utf8_lossy(&out.stderr).trim().to_owned()),
    }
}

pub async fn end_token(distro: &str, token: &str) -> Result<(), String> {
    let mut cmd = tokio::process::Command::new("wsl.exe");
    cmd.args(["-d", distro, "-e", "sh", "-c", END_TOKEN, "sh", token])
        .env("WSL_UTF8", "1");
    bounded(cmd, END_TOKEN_WAIT).await
}

const END_TOKEN: &str = r#"token=$1 nl='
'
signal() {
  sent=1
  for f in $(grep -laF "LOOM_TOKEN=$token" /proc/[0-9]*/environ 2>/dev/null); do
    env=$nl$(tr '\0' '\n' <"$f" 2>/dev/null)$nl
    case $env in *"${nl}LOOM_TOKEN=$token$nl"*) ;; *) continue ;; esac
    case $env in *"${nl}HOME=$HOME$nl"*) ;; *) continue ;; esac
    pid=${f#/proc/}
    kill -"$1" "${pid%/environ}" 2>/dev/null && sent=0
  done
  return $sent
}
signal TERM || exit 0
sleep 2
signal KILL
exit 0"#;

const END_TOKEN_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

async fn bounded(
    mut cmd: tokio::process::Command,
    limit: std::time::Duration,
) -> Result<(), String> {
    cmd.stdin(std::process::Stdio::null()).kill_on_drop(true);
    match tokio::time::timeout(limit, cmd.output()).await {
        Err(_) => Err(format!("{}초 안에 끝나지 않았다", limit.as_secs())),
        Ok(Err(e)) => Err(e.to_string()),
        Ok(Ok(out)) if out.status.success() => Ok(()),
        Ok(Ok(out)) => Err(String::from_utf8_lossy(&out.stderr).trim().to_owned()),
    }
}

#[derive(Debug, PartialEq)]
pub struct Inside {
    host: String,
    pub distro: String,
    cwd: String,
    bin: String,
    path: String,
    home: String,
    found: Vec<(String, String)>,
    paths: Vec<String>,
}

/// 배포판을 쓸 수 없다. 에이전트에게는 `Display` 글이, 화면에는 코드와 값이 간다.
#[derive(Debug, PartialEq, serde::Serialize, ts_rs::TS)]
#[serde(tag = "code", rename_all = "camelCase")]
#[ts(export)]
pub enum InsideError {
    /// 배포판이 시작하지 못했다. `cause`는 `wsl.exe` 오류 글이다.
    Start { distro: String, cause: String },
    /// 배포판 안에서 Windows 프로그램을 띄울 수 없다(`/etc/wsl.conf` `[interop] enabled=false`). 노드의 `loom` 명령이 닿지 않는다.
    Interop { distro: String },
}

impl std::fmt::Display for InsideError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InsideError::Start { distro, cause } => write!(
                f,
                "Could not start {distro}: {cause}. Run `wsl -d {distro}` in a terminal to see the cause."
            ),
            InsideError::Interop { distro } => write!(
                f,
                "{distro} cannot run Windows programs (WSL interop is off), so the node could not use loom commands. Set [interop] enabled=true in /etc/wsl.conf, run `wsl --shutdown`, and try again."
            ),
        }
    }
}

const SEP: char = '\u{1e}';
const INTEROP_OFF: &str = "interop-off";

pub async fn inside(
    at: &WslPath,
    find: &[&str],
    bin: &Path,
    paths: &[&Path],
) -> Result<Inside, InsideError> {
    let distro = at.distro.as_str();
    let script = r#"[ -n "$(ls /proc/sys/fs/binfmt_misc/WSLInterop* 2>/dev/null)" ] || { printf '\036interop-off\036'; exit 0; }
printf '\036%s' "$PATH" "$HOME"
while [ "$1" != -- ]; do printf '\036%s' "$(command -v "$1")"; shift; done; shift
for p; do printf '\036%s' "$(wslpath -u "$p")"; done
printf '\036'"#;
    let mut cmd = tokio::process::Command::new("wsl.exe");
    cmd.args(["-d", distro, "-e", "bash", "-lic", script, "bash"])
        .args(find)
        .arg("--")
        .arg(bin)
        .args(paths)
        .env("WSL_UTF8", "1")
        .stdin(std::process::Stdio::null());
    let start = |cause: String| InsideError::Start {
        distro: distro.into(),
        cause,
    };
    let out = cmd.output().await.map_err(|e| start(e.to_string()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !text.contains(SEP) {
        return Err(start(String::from_utf8_lossy(&out.stderr).trim().into()));
    }
    read_inside(at, find, &text).ok_or_else(|| InsideError::Interop {
        distro: distro.into(),
    })
}

fn read_inside(at: &WslPath, find: &[&str], text: &str) -> Option<Inside> {
    let mut values = text.split(SEP).skip(1);
    let path = values.next()?;
    if path == INTEROP_OFF {
        return None;
    }
    let home = values.next()?.to_owned();
    let found = find
        .iter()
        .map(|c| (c.to_string(), values.next().unwrap_or_default().to_owned()))
        .collect();
    let bin = values.next()?.to_owned();
    Some(Inside {
        host: at.host.clone(),
        distro: at.distro.clone(),
        cwd: at.path.clone(),
        bin,
        path: path.into(),
        home,
        found,
        paths: values
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .collect(),
    })
}

impl Inside {
    pub fn find<'a>(&self, names: impl IntoIterator<Item = &'a String>) -> Option<&str> {
        names.into_iter().find_map(|n| {
            self.found
                .iter()
                .find(|(c, p)| c == n && !p.is_empty())
                .map(|(_, p)| p.as_str())
        })
    }

    pub fn path(&self, i: usize) -> Option<&str> {
        self.paths.get(i).map(String::as_str)
    }

    pub fn wrap(&self, cmd: &CommandBuilder) -> CommandBuilder {
        let mut wsl = CommandBuilder::new("wsl.exe");
        wsl.args(self.prefix());
        wsl.args(cmd.get_argv());
        let env: Vec<_> = cmd.iter_extra_env_as_str().collect();
        for (key, value) in passed(&env) {
            wsl.env(key, value);
        }
        wsl.env("WSLENV", names(&env));
        if let Some(dir) = cmd.get_cwd() {
            wsl.cwd(dir);
        }
        wsl
    }

    pub fn server<K: AsRef<str>, V: AsRef<std::ffi::OsStr>>(
        &self,
        argv: &[impl AsRef<std::ffi::OsStr>],
        env: &[(K, V)],
    ) -> std::process::Command {
        let mut wsl = std::process::Command::new("wsl.exe");
        wsl.args(self.prefix()).args(argv);
        wsl.envs(passed(env).map(|(k, v)| (k.as_ref(), v)));
        wsl.env("WSLENV", names(env));
        wsl
    }

    pub fn node_path(&self) -> String {
        format!("{}:{}", self.bin, self.path)
    }

    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    pub fn home(&self) -> &str {
        &self.home
    }

    pub fn local(&self, linux: &str) -> PathBuf {
        PathBuf::from(format!(
            r"\\{}\{}{}",
            self.host,
            self.distro,
            linux.trim_end_matches('/').replace('/', "\\")
        ))
    }

    fn prefix(&self) -> [String; 7] {
        [
            "-d".into(),
            self.distro.clone(),
            "--cd".into(),
            self.cwd.clone(),
            "--exec".into(),
            "env".into(),
            format!("PATH={}", self.node_path()),
        ]
    }
}

fn passed<K: AsRef<str>, V>(env: &[(K, V)]) -> impl Iterator<Item = &(K, V)> {
    env.iter().filter(|(k, _)| {
        let k = k.as_ref();
        !k.eq_ignore_ascii_case("PATH") && !k.eq_ignore_ascii_case("WSLENV")
    })
}

fn names<K: AsRef<str>, V>(env: &[(K, V)]) -> String {
    passed(env)
        .map(|(k, _)| k.as_ref())
        .collect::<Vec<_>>()
        .join(":")
}

pub fn write_shim(dir: &Path, exe: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let exe = exe.to_string_lossy().replace('\'', r"'\''");
    std::fs::write(
        dir.join("loom"),
        format!("#!/bin/sh\nexec \"$(wslpath -u '{exe}')\" \"$@\"\n"),
    )
}

pub fn shell(line: &str) -> Vec<OsString> {
    let line = match line.trim() {
        "" => r#"exec "${SHELL:-/bin/sh}" -l"#,
        _ => line,
    };
    ["sh", "-c", line].map(OsString::from).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn end_token_ends_only_processes_with_the_token_and_home() {
        let token = format!("test{}", std::process::id());
        let home = std::env::var("HOME").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let child_pid = dir.path().join("child");
        let spawn = |script: String, token: &str, home: &str| {
            std::process::Command::new("sh")
                .args(["-c", &script])
                .env("LOOM_TOKEN", token)
                .env("HOME", home)
                .spawn()
                .unwrap()
        };
        let mut stubborn = spawn(
            "trap '' TERM; while :; do sleep 0.1; done".into(),
            &token,
            &home,
        );
        let mut parent = spawn(
            format!(
                "LOOM_TOKEN=other sleep 30 & echo $! > {}; wait",
                child_pid.display()
            ),
            &token,
            &home,
        );
        let mut test_daemon = spawn("exec sleep 30".into(), &token, "/tmp/other-home");
        let mut other_node = spawn("exec sleep 30".into(), "other", &home);
        while !child_pid.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let child: u32 = std::fs::read_to_string(&child_pid)
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", END_TOKEN, "sh", &token]);
        bounded(cmd, END_TOKEN_WAIT).await.unwrap();

        use std::os::unix::process::ExitStatusExt;
        assert_eq!(stubborn.wait().unwrap().signal(), Some(9));
        assert_eq!(parent.wait().unwrap().signal(), Some(15));
        assert!(test_daemon.try_wait().unwrap().is_none());
        assert!(other_node.try_wait().unwrap().is_none());
        assert!(std::path::Path::new(&format!("/proc/{child}")).exists());
        for mut left in [test_daemon, other_node] {
            left.kill().unwrap();
            left.wait().unwrap();
        }
        std::process::Command::new("kill")
            .arg(child.to_string())
            .status()
            .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bounded_ends_a_stalled_command() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args([
            "-c",
            &format!("echo $$ > {}; exec sleep 30", pid_file.display()),
        ]);
        let started = std::time::Instant::now();
        let ended = bounded(cmd, std::time::Duration::from_millis(500)).await;
        assert!(ended.is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let stat = std::path::PathBuf::from(format!("/proc/{}/stat", pid.trim()));
        for _ in 0..50 {
            match std::fs::read_to_string(&stat) {
                Err(_) => return,
                Ok(line) if line.rsplit(") ").next().is_some_and(|s| s.starts_with('Z')) => return,
                Ok(_) => {}
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("the stalled command still runs");
    }

    #[test]
    fn a_node_path_is_read_in_the_node_folder() {
        let read = from_node(
            PathBuf::from("/home/k/a.html"),
            Path::new(r"\\wsl.localhost\Ubuntu\home\k"),
        );
        let expected = if cfg!(windows) {
            "//wsl.localhost/Ubuntu/home/k/a.html"
        } else {
            "/home/k/a.html"
        };
        assert_eq!(read, PathBuf::from(expected));
        assert_eq!(
            from_node(PathBuf::from("/home/k/a.html"), Path::new(r"C:\work")),
            PathBuf::from("/home/k/a.html")
        );
    }

    #[test]
    fn a_wsl_folder_command_runs_inside_the_distro() {
        let at = location(r"\\wsl.localhost\Ubuntu\home\a\repo");
        let mut cmd = launch(at.as_ref(), r"\\wsl.localhost\Ubuntu\home\a\repo", "bash");
        cmd.args(["scripts/gate/ship.sh", "b", "c0ffee"]);
        assert_eq!(cmd.get_program(), "wsl.exe");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(
            args,
            [
                "-d",
                "Ubuntu",
                "--cd",
                "/home/a/repo",
                "-e",
                "bash",
                "-lc",
                r#"exec "$0" "$@""#,
                "bash",
                "scripts/gate/ship.sh",
                "b",
                "c0ffee"
            ]
        );
        let plain = launch(None, "/home/a/repo", "git");
        assert_eq!(plain.get_program(), "git");
        assert_eq!(plain.get_current_dir(), Some(Path::new("/home/a/repo")));
    }

    #[test]
    fn a_gate_command_from_a_unc_folder_follows_the_os() {
        let cmd = command(r"\\wsl.localhost\Ubuntu\home\a\repo", "bash");
        if cfg!(windows) {
            assert_eq!(cmd.get_program(), "wsl.exe");
            assert!(cmd.get_args().any(|a| a == "/home/a/repo"));
        } else {
            assert_eq!(cmd.get_program(), "bash");
        }
    }

    fn wsl(host: &str, distro: &str, path: &str) -> Option<WslPath> {
        Some(WslPath {
            host: host.into(),
            distro: distro.into(),
            path: path.into(),
        })
    }

    #[test]
    fn location_reads_wsl_unc_paths() {
        let home = |host| wsl(host, "Ubuntu-24.04", "/home/ktadev/src/loom");
        assert_eq!(
            location(r"\\wsl.localhost\Ubuntu-24.04\home\ktadev\src\loom"),
            home("wsl.localhost")
        );
        assert_eq!(
            location("//wsl.localhost/Ubuntu-24.04/home/ktadev/src/loom/"),
            home("wsl.localhost")
        );
        assert_eq!(
            location(r"\\wsl$\Ubuntu-24.04\home\ktadev\src\loom"),
            home("wsl$")
        );
        assert_eq!(
            location("//WSL$/Ubuntu-24.04/home/ktadev/src/loom"),
            home("WSL$")
        );
        assert_eq!(
            location(r"\\?\UNC\wsl.localhost\Ubuntu-24.04\home\ktadev\src\loom"),
            home("wsl.localhost")
        );
        assert_eq!(
            location(r"\\wsl.localhost\Ubuntu"),
            wsl("wsl.localhost", "Ubuntu", "/")
        );
        assert_eq!(location("//wsl$/Ubuntu/"), wsl("wsl$", "Ubuntu", "/"));
    }

    #[test]
    fn location_ignores_other_paths() {
        assert_eq!(location(r"\\server\share\loom"), None);
        assert_eq!(location(r"\\wsl.localhost"), None);
        assert_eq!(location(r"C:\Users\me\loom"), None);
        assert_eq!(location("C:/Users/me/loom"), None);
        assert_eq!(location(r"\\?\C:\Users\me\loom"), None);
        assert_eq!(location("/home/ktadev/src/loom"), None);
    }

    #[test]
    fn shell_path_writes_backslashes() {
        let repo = location(r"\\wsl.localhost\Ubuntu\home\ktadev").unwrap();
        assert_eq!(
            repo.shell_path("/home/ktadev/a b/협업.html"),
            r"\\wsl.localhost\Ubuntu\home\ktadev\a b\협업.html"
        );
    }

    #[test]
    fn unc_writes_the_text_windows_git_prints() {
        let repo = location(r"\\wsl.localhost\Ubuntu\home\ktadev\src\loom").unwrap();
        assert_eq!(
            repo.unc("/home/ktadev/loom/workspaces/loom/찾기사이드바"),
            "//wsl.localhost/Ubuntu/home/ktadev/loom/workspaces/loom/찾기사이드바"
        );
        assert_eq!(
            location(&repo.unc("/home/ktadev/src/loom")),
            location(r"\\wsl.localhost\Ubuntu\home\ktadev\src\loom")
        );
    }

    fn at() -> WslPath {
        location(r"\\wsl.localhost\Ubuntu-24.04\home\me\src\loom").unwrap()
    }

    #[test]
    fn inside_reads_values_after_shell_noise() {
        let text = "welcome from .bashrc\n\u{1e}/home/me/.local/bin:/usr/bin\u{1e}/home/me\u{1e}/home/me/.local/bin/claude\u{1e}\u{1e}/mnt/c/loom/wsl\u{1e}/mnt/c/loom/plugin\u{1e}";
        let inside = read_inside(&at(), &["claude", "claude-code"], text).unwrap();
        assert_eq!(inside.path, "/home/me/.local/bin:/usr/bin");
        let names = ["claude-code".to_string(), "claude".to_string()];
        assert_eq!(inside.find(&names), Some("/home/me/.local/bin/claude"));
        assert_eq!(inside.find(&["claude-code".to_string()]), None);
        assert_eq!(inside.bin, "/mnt/c/loom/wsl");
        assert_eq!(inside.path(0), Some("/mnt/c/loom/plugin"));
        assert_eq!(inside.cwd, "/home/me/src/loom");
        assert_eq!(inside.home(), "/home/me");
        assert_eq!(
            inside.local("/home/me/.claude/"),
            PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\me\.claude")
        );
        assert_eq!(read_inside(&at(), &[], "\u{1e}interop-off\u{1e}"), None);
    }

    #[test]
    fn wrap_runs_in_the_distro_with_loom_first_and_passes_set_variables() {
        let text = "\u{1e}/usr/bin\u{1e}/home/me\u{1e}/home/me/.local/bin/claude\u{1e}/mnt/c/loom/wsl\u{1e}";
        let inside = read_inside(&at(), &["claude"], text).unwrap();
        let mut cmd = CommandBuilder::new("/home/me/.local/bin/claude");
        cmd.args(["--plugin-dir", "/mnt/c/loom/plugin", "a \"b\"\nc"]);
        cmd.env("LOOM_TOKEN", "t1");
        cmd.env("PATH", r"C:\loom;C:\Windows");
        let wsl = inside.wrap(&cmd);
        let argv: Vec<_> = wsl.get_argv().iter().map(|a| a.to_string_lossy()).collect();
        assert_eq!(
            argv,
            [
                "wsl.exe",
                "-d",
                "Ubuntu-24.04",
                "--cd",
                "/home/me/src/loom",
                "--exec",
                "env",
                "PATH=/mnt/c/loom/wsl:/usr/bin",
                "/home/me/.local/bin/claude",
                "--plugin-dir",
                "/mnt/c/loom/plugin",
                "a \"b\"\nc",
            ]
        );
        assert_eq!(wsl.get_env("LOOM_TOKEN").unwrap(), "t1");
        let wslenv = wsl
            .get_env("WSLENV")
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(wslenv.split(':').any(|k| k == "LOOM_TOKEN"), "{wslenv}");
        assert!(!wslenv.split(':').any(|k| k == "PATH"), "{wslenv}");
    }

    #[test]
    fn read_list_reads_utf16_and_utf8() {
        let utf16: Vec<u8> = "\u{feff}Ubuntu-24.04\r\nDebian\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(read_list(&utf16), ["Ubuntu-24.04", "Debian"]);
        assert_eq!(read_list(b"Ubuntu\n\n"), ["Ubuntu"]);
        assert!(read_list(b"").is_empty());
    }

    #[test]
    fn tilde_shortens_only_paths_under_home() {
        assert_eq!(tilde("/home/me/src/loom", "/home/me"), "~/src/loom");
        assert_eq!(tilde("/home/me", "/home/me/"), "~");
        assert_eq!(tilde("/home/mel/src", "/home/me"), "/home/mel/src");
        assert_eq!(tilde("/srv/loom", "/home/me"), "/srv/loom");
        assert_eq!(tilde("/srv/loom", "/"), "/srv/loom");
    }

    #[test]
    fn task_dir_of_a_wsl_node_is_the_linux_path_and_the_daemon_writes_the_unc_path() {
        let text = "\u{1e}/usr/bin\u{1e}/home/me\u{1e}/mnt/c/loom/wsl\u{1e}";
        let inside = read_inside(&at(), &[], text).unwrap();
        let dir = crate::task::Tasks::wsl(inside.home(), |p| inside.local(p)).dir("loom-feat-x");
        assert_eq!(dir.shown, "/home/me/.cache/loom/loom-feat-x");
        if cfg!(windows) {
            assert_eq!(
                dir.local,
                PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home\me\.cache\loom\loom-feat-x")
            );
        }
    }

    #[test]
    fn windows_path_is_the_unc_path() {
        assert_eq!(
            windows_path("Ubuntu", "/home/me/"),
            r"\\wsl.localhost\Ubuntu\home\me"
        );
        assert_eq!(
            location(&windows_path("Ubuntu", "/home/me")).unwrap().path,
            "/home/me"
        );
    }

    #[test]
    fn shell_is_the_login_shell_or_the_line() {
        assert_eq!(
            wsl_shell(""),
            ["sh", "-c", r#"exec "${SHELL:-/bin/sh}" -l"#]
        );
        assert_eq!(wsl_shell("npm ci"), ["sh", "-c", "npm ci"]);
    }

    fn wsl_shell(line: &str) -> Vec<String> {
        shell(line)
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn server_runs_in_the_distro_and_passes_env_but_not_path() {
        let text = "\u{1e}/usr/bin\u{1e}/home/me\u{1e}/mnt/c/loom/wsl\u{1e}";
        let inside = read_inside(&at(), &[], text).unwrap();
        let env = [("LOOM_TOKEN", "t1"), ("PATH", r"C:\Windows")];
        let cmd = inside.server(&["codex", "app-server"], &env);
        let argv: Vec<_> = std::iter::once(cmd.get_program())
            .chain(cmd.get_args())
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            argv,
            [
                "wsl.exe",
                "-d",
                "Ubuntu-24.04",
                "--cd",
                "/home/me/src/loom",
                "--exec",
                "env",
                "PATH=/mnt/c/loom/wsl:/usr/bin",
                "codex",
                "app-server",
            ]
        );
        let envs: Vec<_> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(envs.contains(&("LOOM_TOKEN".into(), Some("t1".into()))));
        assert!(envs.contains(&("WSLENV".into(), Some("LOOM_TOKEN".into()))));
        assert!(!envs.iter().any(|(k, _)| k == "PATH"));
    }
}
