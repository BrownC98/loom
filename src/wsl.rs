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

fn interop_path(distro: &str, linux: &str) -> String {
    std::process::Command::new("wslpath")
        .arg("-w")
        .arg(linux)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|path| path.trim_end_matches('\n').to_owned())
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| windows_path(distro, linux))
}

fn verbatim(windows: &str) -> String {
    match windows.strip_prefix(r"\\") {
        Some(unc) => format!(r"\\?\UNC\{unc}"),
        None => format!(r"\\?\{windows}"),
    }
}

fn tunnel_distro() -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    std::env::var_os(SOCKET_ENV)?;
    std::env::var("WSL_DISTRO_NAME").ok()
}

fn linux(path: &Path) -> Option<&str> {
    path.to_str()
        .filter(|p| p.starts_with('/') && !p.starts_with("//"))
}

pub fn for_daemon(path: PathBuf) -> PathBuf {
    match (tunnel_distro(), linux(&path)) {
        (Some(distro), Some(linux)) => verbatim(&interop_path(&distro, linux)).into(),
        _ => path,
    }
}

pub fn cwd_for_daemon(dir: PathBuf) -> PathBuf {
    match (tunnel_distro(), linux(&dir)) {
        (Some(distro), Some(linux)) => interop_path(&distro, linux).into(),
        _ => from_caller(dir),
    }
}

pub fn from_daemon(path: String) -> PathBuf {
    inside_distro(path, tunnel_distro().as_deref())
}

fn inside_distro(path: String, distro: Option<&str>) -> PathBuf {
    match (distro, location(&path)) {
        (Some(distro), Some(inside)) if inside.distro == distro => inside.path.into(),
        _ => path.into(),
    }
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
    match (linux(&path), at) {
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

pub async fn hook_running(distro: &str, token: &str, hooks: &[&str]) -> Result<bool, String> {
    let mut cmd = tokio::process::Command::new("wsl.exe");
    cmd.args(["-d", distro, "-e"]).env("WSL_UTF8", "1");
    hook_found(cmd, token, hooks).await
}

async fn hook_found(
    mut cmd: tokio::process::Command,
    token: &str,
    hooks: &[&str],
) -> Result<bool, String> {
    cmd.args(["sh", "-c", HOOK_FOUND, "sh", token]).args(hooks);
    Ok(!output_within(cmd, END_TOKEN_WAIT).await?.stdout.is_empty())
}

const HOOK_FOUND: &str = r#"token=$1 nl='
'
shift
for f in $(grep -laF "LOOM_TOKEN=$token" /proc/[0-9]*/environ 2>/dev/null); do
  d=${f%/environ}
  [ "$d" = "/proc/$$" ] && continue
  env=$nl$(tr '\0' '\n' <"$f" 2>/dev/null)$nl
  case $env in *"${nl}LOOM_TOKEN=$token$nl"*) ;; *) continue ;; esac
  line=$(tr '\0' ' ' <"$d/cmdline" 2>/dev/null)
  line=${line% }
  line=${line%\"}
  for hook in "$@"; do
    case $line in *"$hook") echo "${d#/proc/}"; exit 0 ;; esac
  done
done
exit 0"#;

async fn bounded(cmd: tokio::process::Command, limit: std::time::Duration) -> Result<(), String> {
    output_within(cmd, limit).await.map(drop)
}

async fn output_within(
    mut cmd: tokio::process::Command,
    limit: std::time::Duration,
) -> Result<std::process::Output, String> {
    cmd.stdin(std::process::Stdio::null()).kill_on_drop(true);
    match tokio::time::timeout(limit, cmd.output()).await {
        Err(_) => Err(format!("{}초 안에 끝나지 않았다", limit.as_secs())),
        Ok(Err(e)) => Err(e.to_string()),
        Ok(Ok(out)) if out.status.success() => Ok(out),
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
pub const TUNNEL_DOWN_EXIT: i32 = 3;
pub const SOCKET_ENV: &str = "LOOM_SOCKET";
pub const LINUX_LOOM: &str = "loom-linux";
pub const WINDOWS_VERBS: &[&str] = &[
    "serve", "start", "open", "stop", "carry", "show", "spawn", "pty-host", "guard", "tunnel",
];
const STDIN_VERBS: &[&str] = &["hook", "cursor-hook"];
const RETRY_VERBS: &[&str] = &[
    "hook",
    "cursor-hook",
    "spawn",
    "status",
    "roles",
    "cursor-rules",
    "start",
    "open",
    "stop",
    "show",
];
const LINUX_HASH_HEX: usize = 12;

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

pub struct Tunnel {
    pub linux: PathBuf,
    pub sock: String,
}

pub fn socket(port: u16) -> String {
    format!(".loom/run/daemon-{port}.sock")
}

pub fn install_linux(dir: &Path, source: &Path) -> std::io::Result<Option<PathBuf>> {
    use sha2::Digest;
    let bytes = match std::fs::read(source) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let hash: String = sha2::Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{LINUX_LOOM}-{}", &hash[..LINUX_HASH_HEX]));
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64) {
        return Ok(Some(path));
    }
    let partial = path.with_extension("partial");
    std::fs::write(&partial, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&partial, &path)?;
    Ok(Some(path))
}

pub fn prune_linux(keep: &Path) {
    let (Some(dir), Some(keep)) = (keep.parent(), keep.file_name()) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name != keep
            && name
                .to_string_lossy()
                .starts_with(&format!("{LINUX_LOOM}-"))
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub fn relay(distro: &str, tunnel: &Tunnel) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("wsl.exe");
    cmd.args([
        "-d",
        distro,
        "--cd",
        "~",
        "--exec",
        "sh",
        "-c",
        r#"exec "$(wslpath -u "$1")" tunnel "$HOME/$2""#,
        "sh",
    ])
    .arg(&tunnel.linux)
    .arg(&tunnel.sock);
    cmd
}

fn quoted(path: &Path) -> String {
    path.to_string_lossy().replace('\'', r"'\''")
}

pub fn write_shim(dir: &Path, exe: &Path, tunnel: Option<&Tunnel>) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let shim = dir.join("loom");
    let through_tunnel = match tunnel {
        Some(Tunnel { linux, sock }) => format!(
            r#"case $1 in {windows}) ;; *)
sock=$HOME/{sock}
lin=$(wslpath -u '{linux}')
if [ ! -x "$lin" ]; then
  why="no $lin"
elif [ ! -S "$sock" ]; then
  why="no $sock"
else
  exec 3<&0
  {SOCKET_ENV}=$sock "$lin" "$@" <&3 3<&- &
  pid=$!
  wait $pid
  code=$?
  exec 3<&-
  [ $code -eq {TUNNEL_DOWN_EXIT} ] || exit $code
  why="$sock refused the connection"
fi
note "$1" "uses loom.exe: $why"
;; esac
"#,
            windows = WINDOWS_VERBS.join(" | "),
            linux = quoted(linux)
        ),
        None => String::new(),
    };
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
trap 'kill $pid 2>/dev/null; wait; exit 143' TERM
trap 'kill $pid 2>/dev/null; wait; exit 130' INT
trap 'kill $pid 2>/dev/null; wait; exit 129' HUP
note() {{
  log=$HOME/{INTEROP_LOG}
  mkdir -p "${{log%/*}}" && printf '%s %s %s\n' "$(date '+%Y-%m-%dT%H:%M:%S%z')" "$1" "$2" >>"$log"
}}
{through_tunnel}[ "$1" = pi-hook ] && exec "$exe" "$@"
tmp=$(mktemp -d) || exec "$exe" "$@"
mkfifo "$tmp/o" "$tmp/e" || {{ rm -rf "$tmp"; exec "$exe" "$@"; }}
trap 'rm -rf "$tmp"' EXIT
: >"$tmp/in"
case $1 in
{stdin}) [ -t 0 ] || cat >"$tmp/in" ;;
spawn)
  brief=1
  for a; do case $a in --file | --file=*) brief= ;; --) break ;; esac; done
  [ -n "$brief" ] && {{ [ -p /dev/stdin ] || [ -f /dev/stdin ]; }} && cat >"$tmp/in" ;;
esac
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
  note "$1" "try $try/{START_TRIES} exit $code: $(head -n 1 "$tmp/err")"
  case $1 in {again}) ;; *)
    printf 'loom: loom.exe may have run %s before WSL interop failed. Look with `loom status` before you send it again.\n' "$1" >&2
    break ;;
  esac
  if [ $try -ge {START_TRIES} ]; then
    echo "loom: loom.exe did not start through WSL interop in {START_TRIES} tries ($HOME/{INTEROP_LOG}). Run the command again." >&2
    break
  fi
  try=$((try + 1))
  sleep {retry}
done
exit $code
"#,
        exe = quoted(exe),
        stdin = STDIN_VERBS.join(" | "),
        again = RETRY_VERBS.join(" | "),
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
    async fn hook_found_sees_only_a_hook_process_with_the_token() {
        let token = format!("hook{}", std::process::id());
        let hooks = ["loom hook", "loom cursor-hook"];
        let found = |token: String| async move {
            hook_found(tokio::process::Command::new("env"), &token, &hooks)
                .await
                .unwrap()
        };
        let spawn = |script: &str, token: &str| {
            std::process::Command::new("sh")
                .args(["-c", script])
                .env("LOOM_TOKEN", token)
                .spawn()
                .unwrap()
        };
        let agent = spawn("exec sleep 30", &token);
        let other = spawn("exec sh -c 'sleep 30; :' loom hook", "other");
        assert!(!found(token.clone()).await, "no hook runs");
        let hook = spawn("exec sh -c 'sleep 30; :' loom cursor-hook", &token);
        while !std::fs::read(format!("/proc/{}/cmdline", hook.id()))
            .is_ok_and(|line| line.ends_with(b"cursor-hook\0"))
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(found(token).await);
        for mut child in [agent, other, hook] {
            child.kill().unwrap();
            child.wait().unwrap();
        }
    }

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

    #[test]
    fn a_linux_path_reaches_the_windows_daemon_in_the_form_loom_exe_sends() {
        assert_eq!(
            verbatim(&windows_path("Ubuntu-24.04", "/tmp/trace/work/문서.html")),
            r"\\?\UNC\wsl.localhost\Ubuntu-24.04\tmp\trace\work\문서.html"
        );
        assert_eq!(verbatim(r"C:\repo\문서.html"), r"\\?\C:\repo\문서.html");
        if let Ok(distro) = std::env::var("WSL_DISTRO_NAME")
            && Path::new("/mnt/c").is_dir()
        {
            assert_eq!(interop_path(&distro, "/mnt/c/repo/작업"), r"C:\repo\작업");
            assert_eq!(interop_path(&distro, "/mnt/c"), r"C:\");
            assert_eq!(
                interop_path(&distro, "/home/k/작업"),
                windows_path(&distro, "/home/k/작업")
            );
        }
        assert_eq!(
            windows_path("Ubuntu-24.04", "/home/ktadev/loom/workspaces/loom/CLI터널"),
            r"\\wsl.localhost\Ubuntu-24.04\home\ktadev\loom\workspaces\loom\CLI터널"
        );
        let task = r"\\wsl.localhost\Ubuntu-24.04\home\k\.cache\loom\repo-me";
        assert_eq!(
            inside_distro(task.into(), Some("Ubuntu-24.04")),
            PathBuf::from("/home/k/.cache/loom/repo-me")
        );
        for (path, distro) in [
            (task, Some("Debian")),
            (task, None),
            (r"C:\Users\k\loom", Some("Ubuntu-24.04")),
        ] {
            assert_eq!(inside_distro(path.into(), distro), PathBuf::from(path));
        }
        assert_eq!(linux(Path::new("/home/k")), Some("/home/k"));
        assert_eq!(linux(Path::new("//wsl.localhost/U/home")), None);
        assert_eq!(linux(Path::new("rel/a")), None);
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
            Self::with_tunnel(name, body, None)
        }

        fn with_tunnel(name: &str, body: &str, linux: Option<&str>) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let fake = dir.path().join("loom.exe");
            std::fs::write(&fake, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::write(dir.path().join("wslpath"), "#!/bin/sh\necho \"$2\"\n").unwrap();
            let tunnel = linux.map(|body| {
                let linux = dir.path().join("loom-linux-test");
                std::fs::write(&linux, format!("#!/bin/sh\n{body}\n")).unwrap();
                let mode = std::fs::Permissions::from_mode(0o755);
                std::fs::set_permissions(&linux, mode).unwrap();
                Tunnel {
                    linux,
                    sock: socket(1),
                }
            });
            write_shim(dir.path(), &fake, tunnel.as_ref()).unwrap();
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
    fn the_shim_runs_a_safe_verb_again_only_after_a_wsl_interop_failure() {
        use std::io::Write;
        let shim = Shim::with_exe(
            "retry",
            r#"dir=${0%/*}
n=$(cat "$dir/count" 2>/dev/null || echo 0)
echo $((n + 1)) >"$dir/count"
input=$(cat)
[ "$(cat "$dir/first")" = never ] && { echo 'WSL ERROR: UtilAcceptVsock:273' >&2; exit 1; }
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
            let _ = child.stdin.take().unwrap().write_all(b"{}");
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
                Some(2),
                line("out status \n"),
                line("WSL (12) ERROR: UtilAcceptVsock:273: accept4 failed 110\nerr\n"),
                line("2")
            ),
            "a verb that reads no input gets none"
        );
        assert_eq!(
            run("interop", &["spawn", "child"]),
            (
                Some(2),
                line("out spawn child {}\n"),
                line("WSL (12) ERROR: UtilAcceptVsock:273: accept4 failed 110\nerr\n"),
                line("2")
            ),
            "the brief survives the retry"
        );
        assert_eq!(log().lines().count(), 3, "{}", log());
        let (code, out, err, count) = run("never", &["status"]);
        assert_eq!((code, out.as_str(), count.as_str()), (Some(1), "", "3"));
        assert!(
            err.ends_with("loom: loom.exe did not start through WSL interop in 3 tries ($HOME/.loom/wsl-interop.log). Run the command again.\n".replace("$HOME", &shim.dir.path().display().to_string()).as_str()),
            "{err}"
        );
        assert_eq!(log().lines().count(), 6, "{}", log());
        for verb in [
            "done", "note", "ask", "answer", "answered", "verdict", "ready", "fail", "adopt", "rm",
            "relaunch", "wait",
        ] {
            assert!(!RETRY_VERBS.contains(&verb));
            assert_eq!(
                run("interop", &[verb, "x"]),
                (
                    Some(1),
                    line(""),
                    format!(
                        "WSL (12) ERROR: UtilAcceptVsock:273: accept4 failed 110\nloom: loom.exe may have run {verb} before WSL interop failed. Look with `loom status` before you send it again.\n"
                    ),
                    line("1")
                ),
                "a one-shot verb is never sent twice"
            );
        }
        assert_eq!(log().lines().count(), 18, "{}", log());
    }

    #[cfg(unix)]
    #[test]
    fn the_shim_reads_a_spawn_brief_only_from_a_pipe_or_a_file_without_file() {
        use std::io::Write;
        let shim = Shim::with_exe("spawnin", r#"echo "exe $* [$(cat)]""#);
        let home = shim.dir.path();
        let run = |args: &[&str], stdin: std::process::Stdio| {
            let mut child = std::process::Command::new(home.join("loom"))
                .args(args)
                .env(
                    "PATH",
                    format!("{}:{}", home.display(), std::env::var("PATH").unwrap()),
                )
                .env("HOME", home)
                .env_remove("LOOM_TOKEN")
                .stdin(stdin)
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(b"Goal: pipe").unwrap();
            }
            let out = child.wait_with_output().unwrap();
            String::from_utf8(out.stdout).unwrap()
        };
        let (open, _held) = std::os::unix::net::UnixStream::pair().unwrap();
        let socket =
            || std::process::Stdio::from(std::os::fd::OwnedFd::from(open.try_clone().unwrap()));
        assert_eq!(
            run(&["spawn", "w", "--file", "b.md"], socket()),
            "exe spawn w --file b.md []\n"
        );
        assert_eq!(
            run(&["spawn", "w", "--file=b.md"], std::process::Stdio::piped()),
            "exe spawn w --file=b.md []\n"
        );
        assert_eq!(run(&["spawn", "w"], socket()), "exe spawn w []\n");
        assert_eq!(
            run(&["spawn", "w"], std::process::Stdio::piped()),
            "exe spawn w [Goal: pipe]\n"
        );
        let brief = home.join("brief.md");
        std::fs::write(&brief, "Goal: file").unwrap();
        assert_eq!(
            run(&["spawn", "w"], std::fs::File::open(&brief).unwrap().into()),
            "exe spawn w [Goal: file]\n"
        );
        assert_eq!(
            run(
                &["spawn", "s", "--", "x", "--file"],
                std::process::Stdio::piped()
            ),
            "exe spawn s -- x --file [Goal: pipe]\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_shim_sends_all_but_the_windows_verbs_to_the_tunnel_and_falls_back_to_loom_exe() {
        use std::io::Write;
        let shim = Shim::with_tunnel(
            "tunnel",
            r#"echo "exe $* $(cat)""#,
            Some(
                r#"[ -e "$HOME/refuse" ] && exit 3
[ -e "$HOME/sent" ] && { echo "sent $1"; exit 1; }
echo "linux $* $LOOM_SOCKET $(cat)"
exit 2"#,
            ),
        );
        let home = shim.dir.path();
        let run = |args: &[&str]| {
            let mut child = std::process::Command::new(home.join("loom"))
                .args(args)
                .env(
                    "PATH",
                    format!("{}:{}", home.display(), std::env::var("PATH").unwrap()),
                )
                .env("HOME", home)
                .env_remove("LOOM_TOKEN")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let _ = child.stdin.take().unwrap().write_all(b"{}");
            let out = child.wait_with_output().unwrap();
            (out.status.code(), String::from_utf8(out.stdout).unwrap())
        };
        let log = || std::fs::read_to_string(home.join(INTEROP_LOG)).unwrap_or_default();
        let sock = home.join(socket(1));

        assert_eq!(run(&["hook"]), (Some(0), "exe hook {}\n".into()));
        assert!(
            log().ends_with(&format!(" hook uses loom.exe: no {}\n", sock.display())),
            "{}",
            log()
        );

        std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        assert_eq!(
            run(&["cursor-hook"]),
            (
                Some(2),
                format!("linux cursor-hook {} {{}}\n", sock.display())
            )
        );
        for verb in [
            "done", "ask", "note", "answer", "status", "wait", "ready", "verdict", "roles",
        ] {
            assert_eq!(
                run(&[verb]),
                (Some(2), format!("linux {verb} {} {{}}\n", sock.display()))
            );
        }
        for verb in WINDOWS_VERBS {
            let input = if STDIN_VERBS.contains(verb) || *verb == "spawn" {
                "{}"
            } else {
                ""
            };
            assert_eq!(run(&[verb]), (Some(0), format!("exe {verb} {input}\n")));
        }
        assert_eq!(log().lines().count(), 1, "{}", log());

        std::fs::write(home.join("sent"), "").unwrap();
        assert_eq!(run(&["note"]), (Some(1), "sent note\n".into()));
        assert_eq!(
            log().lines().count(),
            1,
            "a sent request never goes to loom.exe"
        );
        std::fs::remove_file(home.join("sent")).unwrap();

        std::fs::write(home.join("refuse"), "").unwrap();
        assert_eq!(run(&["pi-hook"]), (Some(0), "exe pi-hook {}\n".into()));
        assert!(
            log().ends_with(
                " pi-hook uses loom.exe: {} refused the connection\n"
                    .replace("{}", &sock.display().to_string())
                    .as_str()
            ),
            "{}",
            log()
        );
        assert_eq!(log().lines().count(), 2, "{}", log());
        drop(listener);
    }

    #[cfg(unix)]
    #[test]
    fn the_shim_streams_pi_hook_lines_through_the_tunnel() {
        use std::io::{BufRead, Write};
        let shim = Shim::with_tunnel(
            "pistream",
            "echo exe",
            Some(r#"while read -r line; do echo "linux $line"; done"#),
        );
        let home = shim.dir.path();
        let sock = home.join(socket(1));
        std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
        let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let mut child = std::process::Command::new(home.join("loom"))
            .arg("pi-hook")
            .env(
                "PATH",
                format!("{}:{}", home.display(), std::env::var("PATH").unwrap()),
            )
            .env("HOME", home)
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
            assert_eq!(line, format!("linux {request}\n"));
        }
        drop(stdin);
        assert!(child.wait().unwrap().success());
        assert!(!home.join(INTEROP_LOG).exists());
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
        write_shim(dir.path(), exe, None).unwrap();
        let shim = dir.path().join("loom");
        let set_read_only = |on| {
            let mut permissions = std::fs::metadata(&shim).unwrap().permissions();
            permissions.set_readonly(on);
            std::fs::set_permissions(&shim, permissions).unwrap();
        };
        set_read_only(true);
        let same = write_shim(dir.path(), exe, None);
        let other = write_shim(dir.path(), Path::new(r"C:\other\loom.exe"), None);
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
