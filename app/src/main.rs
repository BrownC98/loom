//! Windows 앱: 옆에 놓인 `loom.exe serve`를 띄우고, 데몬이 출력한 주소를 창에 연다.
//! 화면은 브라우저용과 같다. 앱이 끝나면 Job Object가 데몬과 노드를 함께 끝낸다.
//! 업데이트는 화면이 `tauri-plugin-updater` JS API로 한다(`web/src/Update.tsx`).
#![windows_subsystem = "windows"]

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const WINDOW: &str = "main";
/// 마지막에 연 저장소 경로. 앱 설정 폴더에 둔다.
const LAST_REPO: &str = "last-repo.txt";
/// 데몬의 시작 줄 앞부분. 근거: `src/daemon.rs`의 `serve`.
const READY_PREFIX: &str = "loom: ";

fn main() {
    tauri::Builder::default()
        // 두 번째 실행은 창을 띄우지 않고 이미 있는 창을 앞으로 가져온다.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window(WINDOW) {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let Some(repo) = choose_repo(&config_dir) else {
                app.handle().exit(0);
                return Ok(());
            };
            std::fs::create_dir_all(&config_dir)?;
            std::fs::write(
                config_dir.join(LAST_REPO),
                repo.as_os_str().as_encoded_bytes(),
            )?;
            let (daemon, url) = start_daemon(&repo)?;
            app.manage(daemon);
            WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::External(url.parse()?))
                .title(format!("loom {}", app.package_info().version))
                .inner_size(1280.0, 800.0)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("loom 앱을 시작하지 못했다");
}

/// 저장소: 명령줄 인자 → 마지막에 연 폴더 → 폴더 고르기 창. 고르기를 취소하면 `None`이다.
fn choose_repo(config_dir: &std::path::Path) -> Option<PathBuf> {
    let last = std::fs::read_to_string(config_dir.join(LAST_REPO)).ok();
    std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .or(last.map(PathBuf::from))
        .filter(|path| path.is_dir())
        .or_else(|| {
            rfd::FileDialog::new()
                .set_title("loom: 저장소 폴더")
                .pick_folder()
        })
}

/// 저장소 폴더에서 데몬을 띄우고, 시작 줄의 주소를 읽는다.
fn start_daemon(repo: &std::path::Path) -> Result<(Child, String), Box<dyn std::error::Error>> {
    let exe = std::env::current_exe()?.with_file_name("loom.exe");
    let mut command = Command::new(exe);
    command
        .args(["serve", "--listen", "127.0.0.1:0"])
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_console(&mut command);
    let mut daemon = command.spawn()?;
    end_with_app(&daemon)?;
    let mut line = String::new();
    // 파이프는 `Child`에 남겨 둔다. 데몬이 뒤에 쓰는 출력이 닫힌 파이프에 막히지 않는다.
    let stdout = daemon.stdout.as_mut().ok_or("데몬 stdout이 없다")?;
    std::io::BufReader::new(stdout).read_line(&mut line)?;
    let url = line
        .trim()
        .strip_prefix(READY_PREFIX)
        .ok_or_else(|| format!("데몬이 시작하지 못했다: {line:?}"))?;
    Ok((daemon, url.to_owned()))
}

/// 콘솔 프로그램인 데몬과, 데몬이 띄우는 콘솔 프로그램이 새 콘솔 창을 열지 않게 한다.
#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_: &mut Command) {}

/// 데몬을 kill-on-close Job Object에 넣는다. 앱이 어떻게 끝나든(창 닫기, 강제 종료, 업데이트)
/// OS가 앱의 핸들을 닫을 때 데몬을 끝낸다. 노드는 데몬이 자기 Job으로 끝낸다(`process_tree`).
/// 앱 자신은 Job에 넣지 않는다. 업데이트 설치 파일은 앱이 끝난 뒤에도 돌아야 한다.
/// 핸들은 닫지 않는다. 닫으면 데몬이 끝난다.
#[cfg(windows)]
fn end_with_app(daemon: &Child) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    let job = win32job::Job::create_with_limit_info(
        win32job::ExtendedLimitInfo::new().limit_kill_on_job_close(),
    )?;
    job.assign_process(daemon.as_raw_handle() as isize)?;
    job.into_handle();
    Ok(())
}

#[cfg(not(windows))]
fn end_with_app(_: &Child) -> std::io::Result<()> {
    Ok(())
}
