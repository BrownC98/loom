//! 기반: Windows가 WSL 배포판 파일을 보이는 UNC 경로(`\\wsl.localhost\<배포판>\…`)와 배포판 안의 Linux 경로를 서로 바꾼다.
//! Windows 데몬이 WSL 폴더의 노드를 그 배포판 안에서 띄우는 명령(`wsl.exe`)도 만든다. 근거: docs/design.md 「데몬 하나」.

use std::ffi::OsString;
use std::path::Path;

use portable_pty::CommandBuilder;

/// UNC 경로가 가리키는 배포판 안의 위치.
#[derive(Debug, PartialEq)]
pub struct WslPath {
    /// `wsl.localhost` 또는 `wsl$`. 사람이 적은 글자 그대로다.
    host: String,
    pub distro: String,
    /// 배포판 안의 Linux 절대 경로.
    pub path: String,
}

impl WslPath {
    /// 같은 배포판의 Linux 절대 경로 `linux`를 Windows git이 적는 UNC 글자(`//<host>/<배포판>/…`)로 바꾼다.
    pub fn unc(&self, linux: &str) -> String {
        format!(
            "//{}/{}{}",
            self.host,
            self.distro,
            linux.trim_end_matches('/')
        )
    }
}

/// `\\wsl.localhost\…`, `\\wsl$\…`, 그 `/` 표기와 `\\?\UNC\` 표기를 읽는다. 다른 경로는 없다.
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

/// Windows 데몬에서 `dir`이 WSL 폴더면 그 배포판 안의 위치다. 다른 OS의 데몬에는 WSL 폴더가 없다.
pub fn of(dir: &Path) -> Option<WslPath> {
    if !cfg!(windows) {
        return None;
    }
    dir.to_str().and_then(location)
}

/// 노드를 띄우기 직전에 배포판 안에서 읽은 값. `wsl.exe` 한 번으로 읽는다(약 0.2초).
#[derive(Debug, PartialEq)]
pub struct Inside {
    pub distro: String,
    /// 노드가 도는 배포판 안의 폴더.
    cwd: String,
    /// 배포판 안에서 본 loom 껍데기 폴더(`write_shim`). `PATH` 맨 앞에 넣는다.
    bin: String,
    /// 로그인·대화형 셸(`bash -lic`)의 `PATH`. `~/.local/bin`, nvm처럼 `.profile`·`.bashrc`가 더한 곳이 든다.
    path: String,
    /// `find`에 준 명령마다 `command -v` 결과. 없으면 빈 글이다.
    found: Vec<(String, String)>,
    /// `paths`에 준 Windows 경로마다 배포판 안의 경로(`wslpath -u`).
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

/// 값 사이의 구분 글자(RS). `.bashrc`가 쓰는 다른 출력과 섞이지 않는다.
const SEP: char = '\u{1e}';
/// interop이 꺼졌을 때 첫 값.
const INTEROP_OFF: &str = "interop-off";

/// `at`의 배포판에서 `PATH`, `find`의 명령 위치, 껍데기 폴더 `bin`과 `paths`의 배포판 안 경로를 한 번에 읽는다.
pub async fn inside(
    at: &WslPath,
    find: &[&str],
    bin: &Path,
    paths: &[&Path],
) -> Result<Inside, InsideError> {
    let distro = at.distro.as_str();
    let script = r#"[ -n "$(ls /proc/sys/fs/binfmt_misc/WSLInterop* 2>/dev/null)" ] || { printf '\036interop-off\036'; exit 0; }
printf '\036%s' "$PATH"
while [ "$1" != -- ]; do printf '\036%s' "$(command -v "$1")"; shift; done; shift
for p; do printf '\036%s' "$(wslpath -u "$p")"; done
printf '\036'"#;
    let mut cmd = tokio::process::Command::new("wsl.exe");
    cmd.args(["-d", distro, "-e", "bash", "-lic", script, "bash"])
        .args(find)
        .arg("--")
        .arg(bin)
        .args(paths)
        // `wsl.exe`의 오류 글은 이 값이 없으면 UTF-16이다.
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
    let found = find
        .iter()
        .map(|c| (c.to_string(), values.next().unwrap_or_default().to_owned()))
        .collect();
    let bin = values.next()?.to_owned();
    Some(Inside {
        distro: at.distro.clone(),
        cwd: at.path.clone(),
        bin,
        path: path.into(),
        found,
        paths: values
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .collect(),
    })
}

impl Inside {
    /// `names` 가운데 처음 찾은 명령의 배포판 안 경로.
    pub fn find<'a>(&self, names: impl IntoIterator<Item = &'a String>) -> Option<&str> {
        names.into_iter().find_map(|n| {
            self.found
                .iter()
                .find(|(c, p)| c == n && !p.is_empty())
                .map(|(_, p)| p.as_str())
        })
    }

    /// `inside`에 준 `paths`의 `i`번째를 배포판 안 경로로.
    pub fn path(&self, i: usize) -> Option<&str> {
        self.paths.get(i).map(String::as_str)
    }

    /// `cmd`(배포판 안의 프로그램과 인자)를 그 배포판의 노드 폴더에서 띄우는 `wsl.exe` 명령으로 바꾼다.
    /// `PATH`는 loom 껍데기 폴더를 앞에 넣은 배포판의 `PATH`다. Windows `PATH`는 넘기지 않는다.
    /// `cmd`에 넣은 다른 환경 변수는 `WSLENV`로 넘긴다. 배포판 안의 `loom` 껍데기가 부르는 `loom.exe`도 그 값을 다시 받는다.
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

    /// 노드 서버(Codex app-server, OpenCode 2 서버)를 노드 폴더에서 띄우는 `wsl.exe` 명령. `wrap`과 같은 규칙이다.
    /// 서버는 배포판의 루프백에서 듣는다. 데몬은 Windows 루프백의 같은 포트로 닿는다(WSL localhost 전달).
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

    /// 배포판 안의 노드 `PATH`: loom 껍데기 폴더를 앞에 넣은 배포판의 `PATH`.
    pub fn node_path(&self) -> String {
        format!("{}:{}", self.bin, self.path)
    }

    /// 노드가 도는 배포판 안의 폴더.
    pub fn cwd(&self) -> &str {
        &self.cwd
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

/// `WSLENV`로 넘길 변수. Windows `PATH`와 `WSLENV` 자신은 넘기지 않는다.
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

/// 배포판 안의 `loom` 명령: 이 데몬의 `loom.exe`를 interop으로 부른다. Linux loom을 따로 깔거나 판을 맞추지 않는다.
/// 데몬이 시작할 때 `<데이터 폴더>/wsl/loom`에 쓴다. 경로는 배포판마다 다를 수 있어 부를 때 `wslpath`가 바꾼다.
pub fn write_shim(dir: &Path, exe: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let exe = exe.to_string_lossy().replace('\'', r"'\''");
    std::fs::write(
        dir.join("loom"),
        format!("#!/bin/sh\nexec \"$(wslpath -u '{exe}')\" \"$@\"\n"),
    )
}

/// 배포판 안에서 띄울 셸 명령. `line`이 비면 사용자의 로그인 셸이다.
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
        let text = "welcome from .bashrc\n\u{1e}/home/me/.local/bin:/usr/bin\u{1e}/home/me/.local/bin/claude\u{1e}\u{1e}/mnt/c/loom/wsl\u{1e}/mnt/c/loom/plugin\u{1e}";
        let inside = read_inside(&at(), &["claude", "claude-code"], text).unwrap();
        assert_eq!(inside.path, "/home/me/.local/bin:/usr/bin");
        let names = ["claude-code".to_string(), "claude".to_string()];
        assert_eq!(inside.find(&names), Some("/home/me/.local/bin/claude"));
        assert_eq!(inside.find(&["claude-code".to_string()]), None);
        assert_eq!(inside.bin, "/mnt/c/loom/wsl");
        assert_eq!(inside.path(0), Some("/mnt/c/loom/plugin"));
        assert_eq!(inside.cwd, "/home/me/src/loom");
        assert_eq!(read_inside(&at(), &[], "\u{1e}interop-off\u{1e}"), None);
    }

    #[test]
    fn wrap_runs_in_the_distro_with_loom_first_and_passes_set_variables() {
        let text = "\u{1e}/usr/bin\u{1e}/home/me/.local/bin/claude\u{1e}/mnt/c/loom/wsl\u{1e}";
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
        let text = "\u{1e}/usr/bin\u{1e}/mnt/c/loom/wsl\u{1e}";
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
