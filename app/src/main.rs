//! Windows 앱: 옆에 놓인 `loom.exe serve`를 띄우고, 데몬이 출력한 주소를 창에 연다.
//! 화면은 브라우저용과 같다. 앱이 끝나면 Job Object가 데몬만 끝낸다. 노드는 살고, 다음에 띄운 데몬이 다시 붙는다.
//! 업데이트는 화면이 `tauri-plugin-updater` JS API로 한다(`web/src/Update.tsx`).
#![windows_subsystem = "windows"]

use std::io::BufRead;
use std::process::{Child, Command, Stdio};

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_window_state::StateFlags;

const WINDOW: &str = "main";
/// 데몬의 시작 줄 앞부분. 근거: `src/daemon.rs`의 `serve`.
const READY_PREFIX: &str = "loom: ";
/// 데몬이 들은 포트를 적는 파일(앱 설정 폴더 안). 노드 환경의 `LOOM_URL`은 데몬 포트를 담는다.
/// 업데이트나 다시 열기 뒤의 데몬이 같은 포트에서 들어야 살아 있는 노드의 loom 명령이 닿는다.
const PORT_FILE: &str = "daemon-port";
/// 지난 포트를 쓸 수 없을 때 OS가 포트를 고르게 하는 값.
const ANY_PORT: u16 = 0;

fn main() {
    tauri::Builder::default()
        // 두 번째 실행은 창을 띄우지 않고 이미 있는 창을 앞으로 가져온다.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window(WINDOW) {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        // 제목 줄은 화면이 그린다(아래 `decorations(false)`). 옛 판이 저장한 「OS 제목 줄 있음」을 되살리지 않는다.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::all() - StateFlags::DECORATIONS)
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // 시작 폴더는 git 저장소가 아니라 데몬이 저장소로 더하지 않는다. 저장소는 DB에 남고, 사람이 화면에서 더한다.
            let config_dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&config_dir)?;
            let port_file = config_dir.join(PORT_FILE);
            let last_port = std::fs::read_to_string(&port_file)
                .ok()
                .and_then(|text| text.trim().parse().ok());
            // 지난 포트를 다른 프로그램이 쓰면 데몬이 듣지 못하고 끝난다. 그때는 새 포트로 띄운다.
            let (daemon, url) = match last_port.map(|port| start_daemon(&config_dir, port)) {
                Some(Ok(started)) => started,
                _ => start_daemon(&config_dir, ANY_PORT)?,
            };
            if let Some((_, port)) = url.rsplit_once(':') {
                std::fs::write(&port_file, port)?;
            }
            app.manage(daemon);
            WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::External(url.parse()?))
                .title(format!("loom {}", app.package_info().version))
                .inner_size(1280.0, 800.0)
                // 화면이 테마를 따르는 제목 줄을 그린다(`web/src/TitleBar.tsx`). 제목은 작업 표시줄에 남는다.
                .decorations(false)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("loom 앱을 시작하지 못했다");
}

/// `folder`에서 루프백 `port`를 듣는 데몬을 띄우고, 시작 줄의 주소를 읽는다.
fn start_daemon(
    folder: &std::path::Path,
    port: u16,
) -> Result<(Child, String), Box<dyn std::error::Error>> {
    let exe = std::env::current_exe()?.with_file_name("loom.exe");
    let mut command = Command::new(exe);
    command
        .args(["serve", "--listen", &format!("127.0.0.1:{port}")])
        .current_dir(folder)
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
/// OS가 앱의 핸들을 닫을 때 데몬을 끝낸다. 그래서 데몬 둘이 한 DB를 쓰지 않는다.
/// 데몬이 띄우는 프로세스(PTY 호스트, 노드 서버)는 Job에 들지 않는다(silent breakaway). 노드는 데몬보다 오래 산다.
/// 앱 자신은 Job에 넣지 않는다. 업데이트 설치 파일은 앱이 끝난 뒤에도 돌아야 한다.
/// 핸들은 닫지 않는다. 닫으면 데몬이 끝난다.
#[cfg(windows)]
fn end_with_app(daemon: &Child) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    let job = win32job::Job::create_with_limit_info(
        win32job::ExtendedLimitInfo::new()
            .limit_kill_on_job_close()
            .limit_silent_breakaway_ok(),
    )?;
    job.assign_process(daemon.as_raw_handle() as isize)?;
    job.into_handle();
    Ok(())
}

#[cfg(not(windows))]
fn end_with_app(_: &Child) -> std::io::Result<()> {
    Ok(())
}
