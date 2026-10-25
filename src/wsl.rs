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

fn decode(bytes: &[u8]) -> String {
    if bytes.contains(&0) {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn read_list(bytes: &[u8]) -> Vec<String> {
    decode(bytes)
        .lines()
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
        let text = values(distro, || {
            let mut cmd = tokio::process::Command::new("wsl.exe");
            cmd.args(["-d", distro, "-e", "sh", "-c", r#"printf '\036%s' "$HOME""#]);
            cmd
        })
        .await?;
        let home = match text.split_once(SEP) {
            Some((_, home)) if home.starts_with('/') => home.to_owned(),
            _ => {
                return Err(InsideError::Start {
                    distro: distro.into(),
                    cause: "$HOME is not an absolute path".into(),
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
    let script = format!(
        r#"home=$1; shift; servers=" $* " roots=
for d in /proc/[0-9]*; do
  pid=${{d#/proc/}}
  case $servers in *" $pid "*) ;; *) [ "$(tr '\0' '\n' <"$d/cmdline" 2>/dev/null | sed -n 2p)" = pty-host ] || continue ;; esac
  tr '\0' '\n' <"$d/environ" 2>/dev/null | grep -qxF "HOME=$home" || continue
  roots="$roots $pid"
done
{END_TREES}"#
    );
    let out = tokio::process::Command::new("wsl.exe")
        .args(["-d", distro, "-e", "sh", "-c", &script, "sh", home])
        .args(servers.iter().map(u32::to_string))
        .env("WSL_UTF8", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| e.to_string())?;
    match out.status.success() {
        true => Ok(()),
        false => Err(failure(&out)),
    }
}

const END_TREES: &str = r#"[ -n "$roots" ] || exit 0
all=$(ps -eo pid=,ppid= | awk -v roots="$roots" 'BEGIN { n = split(roots, r, " "); for (i = 1; i <= n; i++) keep[r[i]] = 1 }
  { parent[$1] = $2 }
  END { do { grew = 0; for (p in parent) if (!(p in keep) && (parent[p] in keep)) { keep[p] = 1; grew = 1 } } while (grew); for (p in keep) print p }')
kill -TERM $all 2>/dev/null
sleep 2
kill -KILL $all 2>/dev/null
exit 0"#;

const MARKS: &str = r#"started() { s=$(cat "/proc/$1/stat" 2>/dev/null) || return 1; set -- ${s##*) }; shift 19; echo "$1"; }
marks=/tmp/loom-guard-$(id -u)"#;

pub async fn end_shells(distro: &str, token: &str) -> Result<(), String> {
    let mut cmd = tokio::process::Command::new("wsl.exe");
    cmd.args([
        "-d",
        distro,
        "-e",
        "sh",
        "-c",
        &end_shells_script(),
        "sh",
        token,
    ])
    .env("WSL_UTF8", "1");
    bounded(cmd, END_TOKEN_WAIT).await
}

fn end_shells_script() -> String {
    format!(
        r#"{MARKS}
token=$1 dir=$marks/$1 roots=
[ -d "$dir" ] && [ ! -L "$dir" ] && [ -O "$dir" ] || exit 0
for f in "$dir"/*.*; do
  [ -e "$f" ] || continue
  m=${{f##*/}} pid=${{m%.*}}
  rm -f "$f"
  [ "$(started "$pid")" = "${{m#*.}}" ] || continue
  tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null | grep -qxF "LOOM_TOKEN=$token" && roots="$roots $pid"
done
{END_TREES}"#
    )
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
        Ok(Ok(out)) => Err(failure(&out)),
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
const START_TRIES: u32 = 3;
const START_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(2);
const INTEROP_OFF: &str = "interop-off";
const INTEROP_LOG: &str = ".loom/wsl-interop.log";
const INTEROP_ERROR: &str = "UtilAcceptVsock|WSL.*ERROR";
pub const REWAKE_EXIT: i32 = 2;

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
    let text = values(distro, || {
        let mut cmd = tokio::process::Command::new("wsl.exe");
        cmd.args(["-d", distro, "-e", "bash", "-lic", script, "bash"])
            .args(find)
            .arg("--")
            .arg(bin)
            .args(paths);
        cmd
    })
    .await?;
    read_inside(at, find, &text).ok_or_else(|| InsideError::Interop {
        distro: distro.into(),
    })
}

async fn values(
    distro: &str,
    wsl: impl Fn() -> tokio::process::Command,
) -> Result<String, InsideError> {
    let mut cause = String::new();
    for attempt in 0..START_TRIES {
        if attempt > 0 {
            tokio::time::sleep(START_RETRY_AFTER).await;
        }
        let mut cmd = wsl();
        cmd.env("WSL_UTF8", "1").stdin(std::process::Stdio::null());
        match cmd.output().await {
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout);
                if text.contains(SEP) {
                    return Ok(text.into_owned());
                }
                cause = failure(&out);
            }
            Err(e) => cause = e.to_string(),
        }
    }
    Err(InsideError::Start {
        distro: distro.into(),
        cause,
    })
}

fn failure(out: &std::process::Output) -> String {
    let text: Vec<String> = [&out.stdout, &out.stderr]
        .map(|bytes| decode(bytes).trim().to_owned())
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect();
    format!("{} ({})", text.join(" "), out.status)
        .trim_start()
        .to_owned()
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
    let shim = dir.join("loom");
    let text = format!(
        r#"#!/bin/sh
if [ "$1" = guard ] && [ -n "$LOOM_TOKEN" ]; then
{MARKS}
  dir=$marks/$LOOM_TOKEN
  mkdir -m 700 "$marks" "$dir" 2>/dev/null
  for d in "$marks" "$dir"; do [ -O "$d" ] && [ ! -L "$d" ] || exit 0; done
  for f in "$dir"/*.*; do
    [ -e "$f" ] || continue
    m=${{f##*/}}
    [ "$(started "${{m%.*}}")" = "${{m#*.}}" ] || rm -f "$f"
  done
  : >"$dir/$PPID.$(started $PPID)"
  exit 0
fi
exe=$(wslpath -u '{exe}')
case $1 in hook | cursor-hook) ;; *) exec "$exe" "$@" ;; esac
tmp=$(mktemp -d) || exec "$exe" "$@"
mkfifo "$tmp/o" "$tmp/e" || {{ rm -rf "$tmp"; exec "$exe" "$@"; }}
trap 'rm -rf "$tmp"' EXIT
trap 'kill $pid 2>/dev/null; wait; exit 143' TERM
trap 'kill $pid 2>/dev/null; wait; exit 130' INT
trap 'kill $pid 2>/dev/null; wait; exit 129' HUP
cat >"$tmp/in"
try=1
while :; do
  tee "$tmp/out" <"$tmp/o" &
  tee "$tmp/err" <"$tmp/e" >&2 &
  "$exe" "$@" <"$tmp/in" >"$tmp/o" 2>"$tmp/e" &
  pid=$!
  wait $pid
  code=$?
  wait
  [ $code -ne 0 ] && [ $code -ne {REWAKE_EXIT} ] && [ ! -s "$tmp/out" ] &&
    head -n 1 "$tmp/err" | grep -qE '{INTEROP_ERROR}' || break
  log=$HOME/{INTEROP_LOG}
  mkdir -p "${{log%/*}}" &&
    printf '%s %s try %s/{START_TRIES} exit %s: %s\n' "$(date '+%Y-%m-%dT%H:%M:%S%z')" "$1" $try $code "$(head -n 1 "$tmp/err")" >>"$log"
  [ $try -lt {START_TRIES} ] || break
  try=$((try + 1))
  sleep {retry}
done
exit $code
"#,
        retry = START_RETRY_AFTER.as_secs()
    );
    if std::fs::read_to_string(&shim).is_ok_and(|old| old == text) {
        return Ok(());
    }
    std::fs::write(shim, text)
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
        let child: u32 = loop {
            if let Some(pid) = std::fs::read_to_string(&child_pid)
                .ok()
                .and_then(|text| text.trim().parse().ok())
            {
                break pid;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };

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
    struct Shim {
        dir: tempfile::TempDir,
        token: String,
    }

    #[cfg(unix)]
    impl Shim {
        fn new(name: &str) -> Self {
            Self::with_exe(name, "echo \"$*\" >> \"${0%/*}/ran\"")
        }

        fn with_exe(name: &str, body: &str) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let fake = dir.path().join("loom.exe");
            std::fs::write(&fake, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::write(dir.path().join("wslpath"), "#!/bin/sh\necho \"$2\"\n").unwrap();
            write_shim(dir.path(), &fake).unwrap();
            for file in ["loom.exe", "wslpath", "loom"] {
                let mode = std::fs::Permissions::from_mode(0o755);
                std::fs::set_permissions(dir.path().join(file), mode).unwrap();
            }
            let token = format!("{name}{}", std::process::id());
            Shim { dir, token }
        }

        fn spawn(&self, script: &str, token: &str) -> std::process::Child {
            let path = format!(
                "{}:{}",
                self.dir.path().display(),
                std::env::var("PATH").unwrap()
            );
            std::process::Command::new("sh")
                .args(["-c", script])
                .env("PATH", path)
                .env("LOOM_TOKEN", token)
                .spawn()
                .unwrap()
        }

        fn run(&self, script: &str) {
            assert!(self.spawn(script, &self.token).wait().unwrap().success());
        }

        fn marks(&self) -> PathBuf {
            use std::os::unix::fs::MetadataExt;
            let uid = std::fs::metadata(self.dir.path()).unwrap().uid();
            PathBuf::from(format!("/tmp/loom-guard-{uid}/{}", self.token))
        }

        fn ran(&self) -> String {
            std::fs::read_to_string(self.dir.path().join("ran")).unwrap_or_default()
        }
    }

    #[cfg(unix)]
    fn started(pid: u32) -> String {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let rest = stat.rsplit_once(") ").unwrap().1;
        rest.split(' ').nth(19).unwrap().to_owned()
    }

    #[cfg(unix)]
    #[test]
    fn the_shim_marks_a_guarded_shell_without_the_windows_loom() {
        use std::os::unix::fs::PermissionsExt;
        let shim = Shim::new("mark");
        let mut shell = shim.spawn("loom guard; exec sleep 30", &shim.token);
        let marked = || std::fs::read_dir(shim.marks()).map_or(0, |d| d.count());
        while marked() == 0 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let mark = shim
            .marks()
            .join(format!("{}.{}", shell.id(), started(shell.id())));
        assert!(mark.exists(), "{mark:?}");
        let mode = std::fs::metadata(shim.marks())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
        assert_eq!(shim.ran(), "");
        shim.run("loom status");
        assert_eq!(shim.ran(), "status\n");
        shell.kill().unwrap();
        shell.wait().unwrap();
        std::fs::remove_dir_all(shim.marks()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_shim_runs_a_hook_again_only_after_a_wsl_interop_failure() {
        use std::io::Write;
        let shim = Shim::with_exe(
            "retry",
            r#"dir=${0%/*}
n=$(cat "$dir/count" 2>/dev/null || echo 0)
echo $((n + 1)) >"$dir/count"
input=$(cat)
if [ "$n" = 0 ]; then
  case $(cat "$dir/first") in
  interop) echo 'WSL (12) ERROR: UtilAcceptVsock:273: accept4 failed 110' >&2; exit 1 ;;
  mail) echo 'loom message #1: WSL ERROR: UtilAcceptVsock:273 in the log' >&2; exit 2 ;;
  late) echo 'loom: could not read the reply' >&2; echo 'WSL ERROR: UtilAcceptVsock' >&2; exit 1 ;;
  esac
fi
echo "out $* $input"
echo err >&2
exit 2"#,
        );
        let run = |first: &str, args: &[&str]| {
            std::fs::write(shim.dir.path().join("first"), first).unwrap();
            let mut child = std::process::Command::new(shim.dir.path().join("loom"))
                .args(args)
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        shim.dir.path().display(),
                        std::env::var("PATH").unwrap()
                    ),
                )
                .env("HOME", shim.dir.path())
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(b"{}").unwrap();
            let out = child.wait_with_output().unwrap();
            let count = std::fs::read_to_string(shim.dir.path().join("count")).unwrap();
            std::fs::remove_file(shim.dir.path().join("count")).unwrap();
            (
                out.status.code(),
                String::from_utf8(out.stdout).unwrap(),
                String::from_utf8(out.stderr).unwrap(),
                count.trim().to_owned(),
            )
        };
        let line = |s: &str| s.to_owned();
        let log = || std::fs::read_to_string(shim.dir.path().join(INTEROP_LOG)).unwrap_or_default();
        assert_eq!(
            run("interop", &["hook"]),
            (
                Some(2),
                line("out hook {}\n"),
                line("WSL (12) ERROR: UtilAcceptVsock:273: accept4 failed 110\nerr\n"),
                line("2")
            )
        );
        assert_eq!(log().lines().count(), 1, "{}", log());
        assert!(
            log().contains(" hook try 1/3 exit 1: WSL (12) ERROR: UtilAcceptVsock"),
            "{}",
            log()
        );
        assert_eq!(
            run("mail", &["hook"]),
            (
                Some(2),
                line(""),
                line("loom message #1: WSL ERROR: UtilAcceptVsock:273 in the log\n"),
                line("1")
            ),
            "a delivered message is never run again"
        );
        assert_eq!(
            run("late", &["cursor-hook"]),
            (
                Some(1),
                line(""),
                line("loom: could not read the reply\nWSL ERROR: UtilAcceptVsock\n"),
                line("1")
            ),
            "loom.exe started"
        );
        assert_eq!(
            run("interop", &["status"]),
            (
                Some(1),
                line(""),
                line("WSL (12) ERROR: UtilAcceptVsock:273: accept4 failed 110\n"),
                line("1")
            )
        );
        assert_eq!(log().lines().count(), 1, "{}", log());
    }

    #[cfg(unix)]
    #[test]
    fn the_shim_streams_pi_hook_lines_without_waiting_for_the_end_of_input() {
        use std::io::{BufRead, Write};
        let shim = Shim::with_exe("stream", "while read -r line; do echo \"got $line\"; done");
        let mut child = std::process::Command::new(shim.dir.path().join("loom"))
            .arg("pi-hook")
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    shim.dir.path().display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        for request in ["{\"id\":1}", "{\"id\":2}"] {
            writeln!(stdin, "{request}").unwrap();
            line.clear();
            stdout.read_line(&mut line).unwrap();
            assert_eq!(line, format!("got {request}\n"));
        }
        drop(stdin);
        assert!(child.wait().unwrap().success());
    }

    #[cfg(unix)]
    #[test]
    fn a_stopped_hook_shim_keeps_the_stderr_of_its_child() {
        use std::io::Read;
        let shim = Shim::with_exe(
            "term",
            "cat >/dev/null\necho 'loom message #1' >&2\nexec sleep 30",
        );
        let mut child = std::process::Command::new(shim.dir.path().join("loom"))
            .arg("hook")
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    shim.dir.path().display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let mut seen = [0u8; 16];
        stderr.read_exact(&mut seen).unwrap();
        assert_eq!(&seen, b"loom message #1\n");
        std::process::Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(143));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn end_shells_ends_marked_live_shells_with_the_token_after_many_guards() {
        let shim = Shim::new("end");
        let child_pid = shim.dir.path().join("child");
        let mut guarded = shim.spawn(
            &format!(
                "loom guard; LOOM_TOKEN=other sleep 30 & echo $! > {}; wait",
                child_pid.display()
            ),
            &shim.token,
        );
        while !std::fs::read_to_string(&child_pid).is_ok_and(|t| t.ends_with('\n')) {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let child: u32 = std::fs::read_to_string(&child_pid)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        for _ in 0..70 {
            shim.run("loom guard");
        }
        let unmarked = shim.spawn("exec sleep 30", &shim.token);
        let wrong_start = shim.spawn("exec sleep 30", &shim.token);
        let reused = shim.spawn("exec sleep 30", "other");
        let mark = |name: String| std::fs::write(shim.marks().join(name), "").unwrap();
        mark(format!("{}.1", wrong_start.id()));
        mark(format!("{}.{}", reused.id(), started(reused.id())));
        assert_eq!(std::fs::read_dir(shim.marks()).unwrap().count(), 4);

        let mut late = shim.spawn("exec sleep 30", &shim.token);
        let late_mark = format!("{}.{}", late.id(), started(late.id()));
        let script = end_shells_script();
        let during = script.replacen(
            "  rm -f \"$f\"\n",
            &format!("  rm -f \"$f\"\n  : >\"$dir/{late_mark}\"\n"),
            1,
        );
        assert_ne!(during, script);
        let end = |script: String| {
            let mut cmd = tokio::process::Command::new("sh");
            cmd.args(["-c", &script, "sh", &shim.token]);
            bounded(cmd, END_TOKEN_WAIT)
        };
        end(during).await.unwrap();

        use std::os::unix::process::ExitStatusExt;
        assert_eq!(guarded.wait().unwrap().signal(), Some(15));
        assert!(!std::path::Path::new(&format!("/proc/{child}")).exists());
        let left: Vec<_> = std::fs::read_dir(shim.marks())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, [late_mark.as_str()]);
        assert!(late.try_wait().unwrap().is_none());
        end(script).await.unwrap();
        assert_eq!(late.wait().unwrap().signal(), Some(15));
        std::fs::remove_dir_all(shim.marks()).unwrap();
        for mut left in [unmarked, wrong_start, reused] {
            assert!(left.try_wait().unwrap().is_none());
            left.kill().unwrap();
            left.wait().unwrap();
        }
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

    #[cfg(unix)]
    fn failing_wsl(dir: &Path, fails: u32) -> impl Fn() -> tokio::process::Command {
        let tries = dir.join("tries");
        move || {
            let mut cmd = tokio::process::Command::new("sh");
            cmd.args([
                "-c",
                r#"echo x >> "$0"; [ "$(wc -l < "$0")" -gt "$1" ] && { printf '\036/home/me'; exit 0; }
printf '오류 코드: Wsl/Service/0x8007274c\n'; exit 1"#,
            ])
            .arg(&tries)
            .arg(fails.to_string());
            cmd
        }
    }

    #[cfg(unix)]
    fn tries(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("tries"))
            .unwrap()
            .lines()
            .count()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_start_that_fails_once_is_tried_again() {
        let dir = tempfile::tempdir().unwrap();
        let text = values("Ubuntu", failing_wsl(dir.path(), 1)).await.unwrap();
        assert_eq!(text, "\u{1e}/home/me");
        assert_eq!(tries(dir.path()), 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_start_that_keeps_failing_shows_the_wsl_error_text() {
        let dir = tempfile::tempdir().unwrap();
        let failed = values("Ubuntu", failing_wsl(dir.path(), u32::MAX)).await;
        let Err(InsideError::Start { distro, cause }) = failed else {
            panic!("{failed:?}");
        };
        assert_eq!(distro, "Ubuntu");
        assert!(cause.contains("Wsl/Service/0x8007274c"), "{cause}");
        assert!(cause.ends_with("(exit status: 1)"), "{cause}");
        assert_eq!(tries(dir.path()), START_TRIES as usize);
    }

    #[test]
    fn an_unchanged_shim_is_not_written_again() {
        let dir = tempfile::tempdir().unwrap();
        let exe = Path::new(r"C:\loom\loom.exe");
        write_shim(dir.path(), exe).unwrap();
        let shim = dir.path().join("loom");
        let set_read_only = |on| {
            let mut permissions = std::fs::metadata(&shim).unwrap().permissions();
            permissions.set_readonly(on);
            std::fs::set_permissions(&shim, permissions).unwrap();
        };
        set_read_only(true);
        let same = write_shim(dir.path(), exe);
        let other = write_shim(dir.path(), Path::new(r"C:\other\loom.exe"));
        set_read_only(false);
        same.unwrap();
        assert!(other.is_err());
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
