use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use portable_pty::CommandBuilder;

use crate::distro::{self, Distro};
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

fn daemon_distro() -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    std::env::var_os(WINDOWS_DAEMON_ENV)?;
    std::env::var("WSL_DISTRO_NAME").ok()
}

fn linux(path: &Path) -> Option<&str> {
    path.to_str()
        .filter(|p| p.starts_with('/') && !p.starts_with("//"))
}

pub fn for_daemon(path: PathBuf) -> PathBuf {
    match (daemon_distro(), linux(&path)) {
        (Some(distro), Some(linux)) => verbatim(&interop_path(&distro, linux)).into(),
        _ => path,
    }
}

pub fn cwd_for_daemon(dir: PathBuf) -> PathBuf {
    match (daemon_distro(), linux(&dir)) {
        (Some(distro), Some(linux)) => interop_path(&distro, linux).into(),
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

#[derive(Default)]
pub struct Homes(Mutex<HashMap<String, String>>);

impl Homes {
    pub async fn get(&self, distro: &Distro) -> Result<String, InsideError> {
        if let Some(home) = self.0.lock().unwrap().get(distro.name()) {
            return Ok(home.clone());
        }
        let text = values(distro, sh(r#"printf '\036%s' "$HOME""#, [])).await?;
        let home = match text.split_once(SEP) {
            Some((_, home)) if home.starts_with('/') => home.to_owned(),
            _ => {
                return Err(InsideError::Start {
                    distro: distro.name().into(),
                    cause: "$HOME is not an absolute path".into(),
                });
            }
        };
        self.0
            .lock()
            .unwrap()
            .insert(distro.name().into(), home.clone());
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

pub async fn end_shells(distro: &Distro, token: &str) -> Result<(), String> {
    bounded(distro, sh(&end_shells_script(), [token])).await
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

pub async fn end_token(distro: &Distro, token: &str) -> Result<(), String> {
    bounded(distro, sh(END_TOKEN, [token])).await
}

pub fn end_trees(distro: &Distro, roots: &[u32]) {
    let roots: Vec<String> = roots.iter().map(u32::to_string).collect();
    let script = format!("roots=\"$*\"\n{END_TREES}");
    let command = sh(&script, roots.iter().map(String::as_str));
    let distro = distro.clone();
    std::thread::spawn(move || {
        if let Err(e) = distro.run(command) {
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

async fn run(distro: &Distro, command: distro::Command) -> Result<distro::Ran, String> {
    let distro = distro.clone();
    tokio::task::spawn_blocking(move || distro.run(command))
        .await
        .map_err(|e| e.to_string())?
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

async fn bounded(distro: &Distro, command: distro::Command) -> Result<(), String> {
    match tokio::time::timeout(END_TOKEN_WAIT, run(distro, command)).await {
        Err(_) => Err(format!("{}초 안에 끝나지 않았다", END_TOKEN_WAIT.as_secs())),
        Ok(Err(e)) => Err(e),
        Ok(Ok(ran)) if ran.code == Some(0) => Ok(()),
        Ok(Ok(ran)) => Err(ran_failure(&ran)),
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
    host: String,
    pub distro: Distro,
    cwd: String,
    bin: String,
    path: String,
    home: String,
    found: Vec<(String, String)>,
    paths: Vec<String>,
}

const SEP: char = '\u{1e}';
const INTEROP_OFF: &str = "interop-off";
pub const REWAKE_EXIT: i32 = 2;
const WINDOWS_DAEMON_ENV: &str = "LOOM_WINDOWS_DAEMON";
pub const LINUX_LOOM: &str = "loom-linux";
pub const WINDOWS_VERBS: &[&str] = &[
    "serve", "start", "open", "stop", "carry", "show", "spawn", "pty-host", "guard",
];
const LINUX_HASH_HEX: usize = 12;

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
    read_inside(at, distro, find, &text).ok_or(InsideError::Interop { distro: name })
}

async fn values(distro: &Distro, command: distro::Command) -> Result<String, InsideError> {
    let start = |cause| InsideError::Start {
        distro: distro.name().into(),
        cause,
    };
    let ran = run(distro, command).await.map_err(start)?;
    match ran.stdout.contains(SEP) {
        true => Ok(ran.stdout),
        false => Err(start(ran_failure(&ran))),
    }
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

fn read_inside(at: &WslPath, distro: Distro, find: &[&str], text: &str) -> Option<Inside> {
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
        distro,
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
        vars.push(("WSLENV".into(), names(env)));
        vars.push(("PATH".into(), self.node_path()));
        distro::Command {
            argv,
            cwd: self.cwd.clone(),
            env: vars,
        }
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

    pub fn bin_file(&self, name: &str) -> String {
        format!("{}/{name}", self.bin)
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
            self.distro.name(),
            linux.trim_end_matches('/').replace('/', "\\")
        ))
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

fn quoted(path: &Path) -> String {
    path.to_string_lossy().replace('\'', r"'\''")
}

pub fn write_shim(dir: &Path, exe: &Path, linux: Option<&Path>) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let shim = dir.join("loom");
    let through_endpoint = match linux {
        Some(linux) => format!(
            r#"case $1 in {windows}) ;; *)
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
{through_endpoint}exec "$(wslpath -u '{exe}')" "$@"
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

        let (distros, local) = distro::local(dir.path().join("distros"), no_start);
        end_token(&distros.distro("Ubuntu"), &token).await.unwrap();
        local.end_all();

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
            Self::with_linux(name, body, None)
        }

        fn with_linux(name: &str, body: &str, linux: Option<&str>) -> Self {
            use crate::fake_exe::{seal, write_executable};
            let dir = tempfile::tempdir().unwrap();
            let fake = dir.path().join("loom.exe");
            write_executable(&fake, &format!("#!/bin/sh\n{body}\n"));
            write_executable(&dir.path().join("wslpath"), "#!/bin/sh\necho \"$2\"\n");
            let linux = linux.map(|body| {
                let linux = dir.path().join("loom-linux-test");
                write_executable(&linux, &format!("#!/bin/sh\n{body}\n"));
                linux
            });
            write_shim(dir.path(), &fake, linux.as_deref()).unwrap();
            seal(&dir.path().join("loom"));
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
        let (distros, local) = distro::local(shim.dir.path().join("distros"), no_start);
        let ubuntu = distros.distro("Ubuntu");
        let end = |script: String| {
            let ubuntu = ubuntu.clone();
            let token = shim.token.clone();
            async move { bounded(&ubuntu, sh(&script, [token.as_str()])).await }
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
        local.end_all();
        assert_eq!(late.wait().unwrap().signal(), Some(15));
        std::fs::remove_dir_all(shim.marks()).unwrap();
        for mut left in [unmarked, wrong_start, reused] {
            assert!(left.try_wait().unwrap().is_none());
            left.kill().unwrap();
            left.wait().unwrap();
        }
    }

    #[cfg(unix)]
    fn no_start(_: distro::Start) -> Result<std::net::SocketAddr, String> {
        Err("no pty-host here".into())
    }

    fn ubuntu() -> Distro {
        let none: distro::Launch = Box::new(|_| Err("no distro host here".into()));
        std::sync::Arc::new(distro::Distros::new(PathBuf::new(), none)).distro("Ubuntu-24.04")
    }

    #[cfg(unix)]
    #[test]
    fn end_trees_ends_a_server_and_its_children_in_the_distro() {
        use std::os::unix::process::ExitStatusExt;
        let dir = tempfile::tempdir().unwrap();
        let (distros, local) = distro::local(dir.path().to_path_buf(), no_start);
        let mut server = std::process::Command::new("sh")
            .args(["-c", "sleep 30 & wait"])
            .spawn()
            .unwrap();
        end_trees(&distros.distro("Ubuntu"), &[server.id()]);
        assert_eq!(server.wait().unwrap().signal(), Some(libc::SIGTERM));
        local.end_all();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_value_read_that_fails_shows_the_error_text_of_the_distro() {
        let dir = tempfile::tempdir().unwrap();
        let (distros, local) = distro::local(dir.path().to_path_buf(), no_start);
        let failed = values(
            &distros.distro("Ubuntu"),
            sh("printf '오류 코드: Wsl/Service/0x8007274c\\n'; exit 1", []),
        )
        .await;
        let Err(InsideError::Start { distro, cause }) = failed else {
            panic!("{failed:?}");
        };
        assert_eq!(distro, "Ubuntu");
        assert!(cause.contains("Wsl/Service/0x8007274c"), "{cause}");
        assert!(cause.ends_with("(exit code 1)"), "{cause}");
        let ok = values(&distros.distro("Ubuntu"), sh(r#"printf '\036%s' "$0""#, []))
            .await
            .unwrap();
        assert_eq!(ok, "\u{1e}sh");
        local.end_all();
    }

    #[cfg(unix)]
    #[test]
    fn the_shim_sends_all_but_the_windows_verbs_to_the_linux_loom_inside_a_node() {
        use std::io::Write;
        let shim = Shim::with_linux(
            "endpoint",
            r#"echo "exe $* $(cat)""#,
            Some(r#"echo "linux $* $LOOM_SOCKET $LOOM_WINDOWS_DAEMON $(cat)""#),
        );
        let home = shim.dir.path();
        let run = |args: &[&str], endpoint: Option<&str>| {
            let mut cmd = std::process::Command::new(home.join("loom"));
            cmd.args(args)
                .env(
                    "PATH",
                    format!("{}:{}", home.display(), std::env::var("PATH").unwrap()),
                )
                .env_remove("LOOM_TOKEN")
                .env_remove(ENDPOINT_ENV)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped());
            if let Some(endpoint) = endpoint {
                cmd.env(ENDPOINT_ENV, endpoint);
            }
            let mut child = cmd.spawn().unwrap();
            let _ = child.stdin.take().unwrap().write_all(b"{}");
            let out = child.wait_with_output().unwrap();
            (out.status.code(), String::from_utf8(out.stdout).unwrap())
        };
        let sock = "/home/me/.loom/run/abc.sock";
        for verb in [
            "hook",
            "cursor-hook",
            "pi-hook",
            "done",
            "note",
            "wait",
            "status",
        ] {
            assert_eq!(
                run(&[verb], Some(sock)),
                (Some(0), format!("linux {verb} {sock} 1 {{}}\n"))
            );
            assert_eq!(run(&[verb], None), (Some(0), format!("exe {verb} {{}}\n")));
        }
        for verb in WINDOWS_VERBS {
            assert_eq!(
                run(&[verb], Some(sock)),
                (Some(0), format!("exe {verb} {{}}\n"))
            );
        }
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
        let inside = read_inside(&at(), ubuntu(), &["claude", "claude-code"], text).unwrap();
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
        assert_eq!(
            read_inside(&at(), ubuntu(), &[], "\u{1e}interop-off\u{1e}"),
            None
        );
    }

    #[test]
    fn wrap_runs_in_the_distro_with_loom_first_and_passes_set_variables() {
        let text = "\u{1e}/usr/bin\u{1e}/home/me\u{1e}/home/me/.local/bin/claude\u{1e}/mnt/c/loom/wsl\u{1e}";
        let inside = read_inside(&at(), ubuntu(), &["claude"], text).unwrap();
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
        let inside = read_inside(&at(), ubuntu(), &[], text).unwrap();
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
        let inside = read_inside(&at(), ubuntu(), &[], text).unwrap();
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
