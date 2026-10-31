use std::ffi::OsString;
use std::path::{Path, PathBuf};

use portable_pty::CommandBuilder;

use crate::distro::{self, Distro};
use crate::process_tree::Identity;
use crate::protocol::InsideError;
use crate::tunnel::ENDPOINT_ENV;

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

fn of(dir: &Path) -> Option<WslPath> {
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

pub fn interop_path(linux: &Path) -> std::io::Result<String> {
    let out = std::process::Command::new("wslpath")
        .arg("-w")
        .arg(linux)
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::other(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .trim_end_matches('\n')
        .to_owned())
}

fn daemon_path(distro: &str, linux: &str) -> String {
    interop_path(Path::new(linux))
        .ok()
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| windows_path(distro, linux))
}

fn verbatim(windows: &str) -> String {
    match windows.strip_prefix(r"\\") {
        Some(unc) => format!(r"\\?\UNC\{unc}"),
        None => format!(r"\\?\{windows}"),
    }
}

fn daemon_distro() -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    std::env::var_os(WINDOWS_DAEMON_ENV)?;
    std::env::var("WSL_DISTRO_NAME").ok()
}

pub fn to_windows_daemon() -> bool {
    daemon_distro().is_some()
}

fn linux(path: &Path) -> Option<&str> {
    path.to_str()
        .filter(|p| p.starts_with('/') && !p.starts_with("//"))
}

pub fn for_daemon(path: PathBuf) -> PathBuf {
    match (daemon_distro(), linux(&path)) {
        (Some(distro), Some(linux)) => verbatim(&daemon_path(&distro, linux)).into(),
        _ => path,
    }
}

pub fn cwd_for_daemon(dir: PathBuf) -> PathBuf {
    match (daemon_distro(), linux(&dir)) {
        (Some(distro), Some(linux)) => daemon_path(&distro, linux).into(),
        _ => from_caller(dir),
    }
}

pub fn from_daemon(path: String) -> PathBuf {
    inside_distro(path, daemon_distro().as_deref())
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

pub async fn home(distro: &Distro) -> Result<String, InsideError> {
    let text = values(distro, sh(r#"printf '\036%s' "$HOME""#, [])).await?;
    match text.split_once(SEP) {
        Some((_, home)) if home.starts_with('/') => Ok(home.to_owned()),
        _ => Err(InsideError::Start {
            distro: distro.name().into(),
            cause: "$HOME is not an absolute path".into(),
        }),
    }
}

pub fn from_caller(path: std::path::PathBuf) -> std::path::PathBuf {
    linux_at(path, caller())
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

pub async fn end_hosts(distro: &Distro, home: &str) -> Result<(), String> {
    let script = r#"home=$1
for d in /proc/[0-9]*; do
  pid=${d#/proc/}
  [ "$(tr '\0' '\n' <"$d/cmdline" 2>/dev/null | sed -n 2p)" = pty-host ] || continue
  tr '\0' '\n' <"$d/environ" 2>/dev/null | grep -qxF "HOME=$home" && echo "$pid"
done
exit 0"#;
    end_found(distro, sh(script, [home])).await
}

pub async fn end_shells(distro: &Distro, token: &str, shells: &[Identity]) -> Result<(), String> {
    let marks: Vec<String> = shells
        .iter()
        .map(|shell| format!("{}.{}", shell.pid, shell.birth))
        .collect();
    let args = std::iter::once(token).chain(marks.iter().map(String::as_str));
    end_found(distro, sh(&end_shells_script(), args)).await
}

fn end_shells_script() -> String {
    format!(
        r#"{}
holds() {{
  if [ -r /proc/self/environ ]; then
    tr '\0' '\n' <"/proc/$1/environ" 2>/dev/null | grep -qxF "LOOM_TOKEN=$token"
  else
    case " $(ps -E -ww -o command= -p "$1" 2>/dev/null) " in *" LOOM_TOKEN=$token "*) ;; *) return 1 ;; esac
  fi
}}
token=$1; shift
for m; do
  pid=${{m%.*}}
  [ "$(started "$pid")" = "${{m#*.}}" ] || continue
  holds "$pid" && echo "$pid"
done
exit 0"#,
        started_script()
    )
}

fn started_script() -> String {
    format!(
        r#"started() {{
  if [ -r /proc/self/stat ]; then
    s=$(cat "/proc/$1/stat" 2>/dev/null) || return 1; set -- ${{s##*) }}; shift {STAT_START}; echo "$1"
  else
    LC_ALL=C date -j -f '%a %b %d %T %Y' "$(LC_ALL=C ps -o lstart= -p "$1" 2>/dev/null)" +%s 2>/dev/null
  fi
}}"#
    )
}

const STAT_START: usize = 19;

#[cfg(unix)]
pub fn parent_shell() -> Option<Identity> {
    let pid = std::os::unix::process::parent_id();
    let birth = stat_field(&pid.to_string(), STAT_START).or_else(|| started(pid))?;
    Some(Identity { pid, birth })
}

#[cfg(not(unix))]
pub fn parent_shell() -> Option<Identity> {
    None
}

#[cfg(unix)]
fn started(pid: u32) -> Option<i64> {
    let script = format!("{}\nstarted \"$1\"", started_script());
    let out = std::process::Command::new("sh")
        .args(["-c", &script, "sh", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[cfg(unix)]
fn stat_field<T: std::str::FromStr>(pid: &str, at: usize) -> Option<T> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(at)?
        .parse()
        .ok()
}

async fn end_found(distro: &Distro, find: distro::Command) -> Result<(), String> {
    let found = bounded(distro, find).await?;
    let roots: Vec<u32> = found
        .stdout
        .split_whitespace()
        .filter_map(|pid| pid.parse().ok())
        .collect();
    if roots.is_empty() {
        return Ok(());
    }
    let distro = distro.clone();
    tokio::task::spawn_blocking(move || distro.end(roots))
        .await
        .map_err(|e| e.to_string())?
}

pub async fn end_token(distro: &Distro, token: &str, names: &[&str]) -> Result<(), String> {
    let args = std::iter::once(token).chain(names.iter().copied());
    bounded(distro, sh(END_TOKEN, args)).await.map(drop)
}

pub fn end_trees(distro: &Distro, roots: &[u32]) {
    let roots = roots.to_vec();
    let distro = distro.clone();
    std::thread::spawn(move || {
        if let Err(e) = distro.end(roots) {
            eprintln!("loom: {}의 프로세스를 끝내지 못했다: {e}", distro.name());
        }
    });
}

fn sh<'a>(script: &'a str, args: impl IntoIterator<Item = &'a str>) -> distro::Command {
    let argv = ["sh", "-c", script, "sh"]
        .into_iter()
        .chain(args)
        .map(str::to_owned)
        .collect();
    distro::Command {
        argv,
        cwd: "/".into(),
        env: Vec::new(),
    }
}

async fn run(
    distro: &Distro,
    command: distro::Command,
    limit: std::time::Duration,
) -> Result<distro::Ran, String> {
    let distro = distro.clone();
    tokio::task::spawn_blocking(move || distro.run_within(command, limit))
        .await
        .map_err(|e| e.to_string())?
}

const END_TOKEN: &str = r#"token=$1
shift
names=$*
envs() {
  if [ -r /proc/self/environ ]; then
    for f in $(grep -laF "=$token" /proc/[0-9]*/environ 2>/dev/null); do
      pid=${f#/proc/}
      printf '%s %s\n' "${pid%/environ}" "$(tr '\0\n' '  ' <"$f" 2>/dev/null)"
    done
  else
    ps -E -ww -x -o pid= -o command= 2>/dev/null | grep -F "=$token"
  fi
}
signal() {
  sent=1
  found=$(envs)
  while IFS= read -r line; do
    line=${line#"${line%%[! ]*}"}
    held=1
    for name in $names; do
      case " $line " in *" $name=$token "*) held=0 ;; esac
    done
    [ "$held" = 0 ] || continue
    case " $line " in *" HOME=$HOME "*) ;; *) continue ;; esac
    kill -"$1" "${line%% *}" 2>/dev/null && sent=0
  done <<EOF
$found
EOF
  return $sent
}
signal TERM || exit 0
sleep 2
signal KILL
exit 0"#;

const END_TOKEN_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

async fn bounded(distro: &Distro, command: distro::Command) -> Result<distro::Ran, String> {
    match run(distro, command, END_TOKEN_WAIT).await? {
        ran if ran.code == Some(0) => Ok(ran),
        ran => Err(ran_failure(&ran)),
    }
}

fn ran_failure(ran: &distro::Ran) -> String {
    let text: Vec<&str> = [&ran.stdout, &ran.stderr]
        .map(|text| text.trim())
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect();
    let code = ran.code.map_or("no exit code".to_owned(), |code| {
        format!("exit code {code}")
    });
    format!("{} ({code})", text.join(" "))
        .trim_start()
        .to_owned()
}

#[derive(Debug, PartialEq)]
pub struct Inside {
    seen: Seen,
    pub distro: Distro,
    cwd: String,
    bin: String,
    path: String,
    home: String,
    found: Vec<(String, String)>,
    paths: Vec<String>,
}

#[derive(Debug, PartialEq)]
enum Seen {
    Unc(String),
    Ssh { url: String, loom: String },
}

pub enum Run {
    Here(std::process::Command),
    Host(Distro, distro::Command),
}

impl Run {
    pub fn output(self, limit: std::time::Duration) -> Result<distro::Ran, String> {
        match self {
            Run::Here(cmd) => crate::timed::finished(cmd, limit).map(|out| distro::Ran {
                code: out.status.code(),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            }),
            Run::Host(distro, command) => distro.run_within(command, limit),
        }
    }
}

const SEP: char = '\u{1e}';
const PROBE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
const INTEROP_OFF: &str = "interop-off";
pub const REWAKE_EXIT: i32 = 2;
const WINDOWS_DAEMON_ENV: &str = "LOOM_WINDOWS_DAEMON";
pub const WINDOWS_VERBS: &[&str] = &["serve", "start", "open", "stop", "carry", "pty-host"];
pub const SPAWN_AT_DAEMON_EXIT: i32 = 4;

pub async fn inside(
    at: &WslPath,
    distro: Distro,
    find: &[&str],
    bin: &Path,
    paths: &[&Path],
) -> Result<Inside, InsideError> {
    let script = r#"[ -n "$(ls /proc/sys/fs/binfmt_misc/WSLInterop* 2>/dev/null)" ] || { printf '\036interop-off\036'; exit 0; }
printf '\036%s' "$PATH" "$HOME"
while [ "$1" != -- ]; do printf '\036%s' "$(command -v "$1")"; shift; done; shift
for p; do printf '\036%s' "$(wslpath -u "$p")"; done
printf '\036'"#;
    let argv = ["bash", "-lic", script, "bash"]
        .into_iter()
        .chain(find.iter().copied())
        .chain(["--"])
        .map(str::to_owned)
        .chain(
            std::iter::once(bin)
                .chain(paths.iter().copied())
                .map(|p| p.display().to_string()),
        )
        .collect();
    let command = distro::Command {
        argv,
        cwd: "/".into(),
        env: Vec::new(),
    };
    let text = values(&distro, command).await?;
    let name = distro.name().to_owned();
    let seen = Seen::Unc(at.host.clone());
    read_inside(seen, at.path.clone(), distro, find, &text)
        .ok_or(InsideError::Interop { distro: name })
}

pub async fn inside_ssh(
    url: String,
    cwd: &str,
    distro: Distro,
    remote: &crate::ssh::Remote,
    find: &[&str],
    paths: Vec<String>,
) -> Result<Inside, InsideError> {
    let script = [
        crate::ssh::LOGIN_PATH,
        r#"shim=$1 bin=$2
shift 2
[ "$(cat "$bin/loom" 2>/dev/null)" = "$shim" ] || { mkdir -p "$bin" && printf '%s\n' "$shim" >"$bin/loom.$$" && chmod 755 "$bin/loom.$$" && mv -f "$bin/loom.$$" "$bin/loom"; } || exit 1
printf '\036%s' "$PATH" "$HOME"
for c; do printf '\036%s' "$(command -v "$c")"; done
printf '\036%s\036' "$bin""#,
    ]
    .concat();
    let bin = format!("{}/{}/bin", remote.home, crate::ssh::REMOTE_DIR);
    let shim = format!("#!/bin/sh\nexec {} \"$@\"", word(&remote.current()));
    let command = sh(
        &script,
        [shim.as_str(), bin.as_str()]
            .into_iter()
            .chain(find.iter().copied()),
    );
    let name = distro.name().to_owned();
    let text = values(&distro, command).await?;
    let seen = Seen::Ssh {
        url,
        loom: remote.program.clone(),
    };
    let mut inside = read_inside(seen, cwd.to_owned(), distro, find, &text)
        .ok_or(InsideError::Interop { distro: name })?;
    inside.paths = paths;
    Ok(inside)
}

fn word(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

async fn values(distro: &Distro, command: distro::Command) -> Result<String, InsideError> {
    let start = |cause| InsideError::Start {
        distro: distro.name().into(),
        cause,
    };
    let ran = run(distro, command, PROBE_WAIT).await.map_err(start)?;
    match ran.stdout.contains(SEP) {
        true => Ok(ran.stdout),
        false => Err(start(ran_failure(&ran))),
    }
}

fn read_inside(
    seen: Seen,
    cwd: String,
    distro: Distro,
    find: &[&str],
    text: &str,
) -> Option<Inside> {
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
        seen,
        distro,
        cwd,
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

    pub fn wrap(&self, cmd: &CommandBuilder) -> Result<CommandBuilder, String> {
        if let Seen::Ssh { .. } = self.seen {
            return Err(self.no_terminal());
        }
        let env: Vec<_> = cmd.iter_extra_env_as_str().collect();
        let mut wsl = CommandBuilder::new("wsl.exe");
        wsl.args(self.prefix());
        wsl.args(cmd.get_argv());
        for (key, value) in passed(&env) {
            wsl.env(key, value);
        }
        wsl.env("WSLENV", names(&env));
        if let Some(dir) = cmd.get_cwd() {
            wsl.cwd(dir);
        }
        Ok(wsl)
    }

    fn no_terminal(&self) -> String {
        format!(
            "loom does not open terminals or servers on the SSH host {}",
            self.distro.name()
        )
    }

    pub fn host_loom(&self, linux: Option<&Path>) -> Option<String> {
        match &self.seen {
            Seen::Unc(_) => {
                let name = linux?.file_name()?.to_string_lossy();
                Some(self.bin_file(&name))
            }
            Seen::Ssh { loom, .. } => Some(loom.clone()),
        }
    }

    pub fn start(
        &self,
        program: String,
        cmd: &CommandBuilder,
        setup: Option<&CommandBuilder>,
    ) -> distro::Start {
        let argv = |cmd: &CommandBuilder| {
            cmd.get_argv()
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect()
        };
        let env: Vec<_> = cmd.iter_extra_env_as_str().collect();
        distro::Start {
            program,
            command: self.command(argv(cmd), &env),
            setup: setup.map(argv),
        }
    }

    pub fn command<K: AsRef<str>, V: AsRef<std::ffi::OsStr>>(
        &self,
        argv: Vec<String>,
        env: &[(K, V)],
    ) -> distro::Command {
        let mut vars: Vec<(String, String)> = passed(env)
            .map(|(k, v)| {
                (
                    k.as_ref().to_owned(),
                    v.as_ref().to_string_lossy().into_owned(),
                )
            })
            .collect();
        if let Seen::Unc(_) = self.seen {
            vars.push(("WSLENV".into(), names(env)));
        }
        vars.push(("PATH".into(), self.node_path()));
        distro::Command {
            argv,
            cwd: self.cwd.clone(),
            env: vars,
        }
    }

    pub fn process<K: AsRef<str>, V: AsRef<std::ffi::OsStr>>(
        &self,
        argv: &[impl AsRef<std::ffi::OsStr>],
        env: &[(K, V)],
    ) -> Result<std::process::Command, String> {
        match self.seen {
            Seen::Unc(_) => Ok(self.wsl_command(argv, env)),
            Seen::Ssh { .. } => Err(self.no_terminal()),
        }
    }

    pub fn run<K: AsRef<str>, V: AsRef<std::ffi::OsStr>>(
        &self,
        argv: &[impl AsRef<std::ffi::OsStr>],
        env: &[(K, V)],
    ) -> Run {
        match self.seen {
            Seen::Unc(_) => Run::Here(self.wsl_command(argv, env)),
            Seen::Ssh { .. } => {
                let argv = argv
                    .iter()
                    .map(|arg| arg.as_ref().to_string_lossy().into_owned())
                    .collect();
                Run::Host(self.distro.clone(), self.command(argv, env))
            }
        }
    }

    fn wsl_command<K: AsRef<str>, V: AsRef<std::ffi::OsStr>>(
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

    pub fn bin_file(&self, name: &str) -> String {
        format!("{}/{name}", self.bin)
    }

    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    pub fn home(&self) -> &str {
        &self.home
    }

    pub fn run_dir(&self) -> String {
        match self.seen {
            Seen::Unc(_) => format!("{}/.loom/run", self.home),
            Seen::Ssh { .. } => format!("{}/{}/run", self.home, crate::ssh::REMOTE_DIR),
        }
    }

    pub fn local(&self, linux: &str) -> PathBuf {
        let linux = linux.trim_end_matches('/');
        PathBuf::from(match &self.seen {
            Seen::Unc(host) => format!(
                r"\\{host}\{}{}",
                self.distro.name(),
                linux.replace('/', "\\")
            ),
            Seen::Ssh { url, .. } => format!("{url}{linux}"),
        })
    }

    fn prefix(&self) -> [String; 7] {
        [
            "-d".into(),
            self.distro.name().into(),
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

pub fn install_linux(dir: &Path, source: &Path) -> std::io::Result<Option<PathBuf>> {
    let name = match distro::linux_name(source) {
        Ok(name) => name,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    std::fs::create_dir_all(dir)?;
    let path = dir.join(name);
    let len = std::fs::metadata(source)?.len();
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == len) {
        return Ok(Some(path));
    }
    let partial = path.with_extension("partial");
    std::fs::copy(source, &partial)?;
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
                .starts_with(&format!("{}-", distro::LINUX_LOOM))
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn quoted(path: &Path) -> String {
    path.to_string_lossy().replace('\'', r"'\''")
}

pub fn write_shim(dir: &Path, exe: &Path, linux: Option<&Path>) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let shim = dir.join("loom");
    let through_endpoint = match linux {
        Some(linux) => format!(
            r#"case $1 in {windows}) ;;
spawn)
  [ -n "${ENDPOINT_ENV}" ] && {{ env {WINDOWS_DAEMON_ENV}=1 "$(wslpath -u '{linux}')" "$@"; code=$?; [ "$code" = {SPAWN_AT_DAEMON_EXIT} ] || exit "$code"; }} ;;
*)
  [ -n "${ENDPOINT_ENV}" ] && exec env {WINDOWS_DAEMON_ENV}=1 "$(wslpath -u '{linux}')" "$@" ;;
esac
"#,
            windows = WINDOWS_VERBS.join(" | "),
            linux = quoted(linux)
        ),
        None => String::new(),
    };
    let text = format!(
        r#"#!/bin/sh
{through_endpoint}[ "$1" = guard ] && exit 0
exec "$(wslpath -u '{exe}')" "$@"
"#,
        exe = quoted(exe),
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
