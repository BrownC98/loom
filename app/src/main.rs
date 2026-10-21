//! 데스크톱 앱(Windows, macOS): 옆에 놓인 `loom serve`를 띄우고, 데몬이 출력한 주소를 창에 연다.
//! 화면은 브라우저용과 같다. 앱이 끝나면 데몬의 stdin 파이프가 닫혀 데몬만 끝난다. 노드는 살고, 다음에 띄운 데몬이 다시 붙는다.
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
        .plugin(tauri_plugin_single_instance::init(|app, _, _| bring_back(app)))
        // 제목 줄은 화면이 그린다(아래 `frame`). 옛 판이 저장한 「OS 제목 줄 있음」을 되살리지 않는다.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::all() - StateFlags::DECORATIONS)
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        // macOS 업데이트는 앱 묶음만 바꾼다. 화면이 설치 뒤 앱을 다시 띄운다(`web/src/Update.tsx`).
        .plugin(tauri_plugin_process::init())
        // macOS는 창을 닫아도 앱이 끝나지 않는다(맥 관례). 창만 숨기고, Dock 아이콘을 누르면 다시 보인다. ⌘Q가 앱을 끝낸다.
        .on_window_event(|window, event| {
            #[cfg(target_os = "macos")]
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (window, event);
        })
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
            frame(WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::External(url.parse()?)))
                .title(format!("loom {}", app.package_info().version))
                .inner_size(1280.0, 800.0)
                // 앱이 끌어다 놓은 파일을 가로채지 않게 한다. 화면이 HTML 놓기로 받아 데몬에 올린다(`web/src/Terminal.tsx`).
                .disable_drag_drop_handler()
                .build()?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("loom 앱을 시작하지 못했다")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                bring_back(app);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

/// 숨기거나 최소화한 창을 앞으로 가져온다.
fn bring_back(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 화면이 테마를 따르는 제목 줄을 그린다(`web/src/TitleBar.tsx`). 제목은 작업 표시줄·Dock 메뉴에 남는다.
/// Windows: OS 제목 줄을 끈다. 창 버튼은 화면이 그린다.
#[cfg(not(target_os = "macos"))]
fn frame<'a, R: tauri::Runtime, M: Manager<R>>(
    window: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    window.decorations(false)
}

/// macOS 창 버튼(신호등)의 왼쪽 위 자리. 36px 줄 가운데에 놓는다. 화면이 그 자리를 비운다(`.titlebar.mac`).
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHTS: tauri::LogicalPosition<f64> = tauri::LogicalPosition::new(14.0, 18.0);

/// macOS: OS가 화면 위에 창 버튼만 그린다(사람 결정 2026-10-21 "맥에선 맥의 법칙을 따르라", docs/mockups/mac-titlebar.html 안 B).
#[cfg(target_os = "macos")]
fn frame<'a, R: tauri::Runtime, M: Manager<R>>(
    window: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    window
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(TRAFFIC_LIGHTS)
}

/// `folder`에서 루프백 `port`를 듣는 데몬을 띄우고, 시작 줄의 주소를 읽는다.
fn start_daemon(
    folder: &std::path::Path,
    port: u16,
) -> Result<(Child, String), Box<dyn std::error::Error>> {
    let exe = std::env::current_exe()?
        .with_file_name(format!("loom{}", std::env::consts::EXE_SUFFIX));
    let mut command = Command::new(exe);
    command
        .args([
            "serve",
            "--listen",
            &format!("127.0.0.1:{port}"),
            "--until-stdin-closes",
        ])
        .current_dir(folder)
        // 쓰는 끝은 `Child`가 쥔다. 앱이 어떻게 끝나든(창 닫기, 강제 종료, 업데이트) OS가 닫아 데몬이 끝난다.
        // 그래서 데몬 둘이 한 DB를 쓰지 않는다. 앱이 띄운 업데이트 설치 파일은 앱이 끝난 뒤에도 돈다.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_console(&mut command);
    login_path(&mut command);
    let mut daemon = command.spawn()?;
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

/// 로그인 셸의 표시. 대화형 셸이 시작할 때 쓰는 글과 `PATH`를 가른다.
#[cfg(unix)]
const PATH_MARK: &str = "loom-path:";

/// macOS가 Finder·Dock에서 띄운 앱은 로그인 셸의 `PATH`를 받지 않는다(`/usr/bin:/bin:/usr/sbin:/sbin`).
/// 그러면 노드가 `~/.local/bin`·Homebrew의 `claude`·`codex`를 찾지 못한다. 사람의 셸에서 `PATH`를 한 번 읽어 데몬에 준다.
/// `-i`도 준다: `claude` 설치 안내는 `PATH`를 `~/.zshrc`(대화형 셸)에 더한다. 읽지 못하면 받은 `PATH`를 그대로 쓴다.
/// Windows 앱은 사용자 `PATH`를 받으므로 할 일이 없다.
#[cfg(unix)]
fn login_path(command: &mut Command) {
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/zsh".into());
    let Ok(out) = Command::new(shell)
        .args(["-ilc", &format!("printf '{PATH_MARK}%s' \"$PATH\"")])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    if let Some((_, path)) = text.rsplit_once(PATH_MARK)
        && !path.is_empty()
    {
        command.env("PATH", path);
    }
}

#[cfg(windows)]
fn login_path(_: &mut Command) {}
