//! 데스크톱 앱(Windows, macOS): 옆에 놓인 `loom start`로 기계의 데몬 하나에 붙고(없으면 띄우고), 그 주소를 창에 연다.
//! 화면은 브라우저용과 같고 데몬도 같다. 앱을 닫아도 데몬과 노드는 산다. 앱을 업데이트하면 새 판의 `loom start`가 옛 판 데몬을 바꾼다.
//! 업데이트는 화면이 `tauri-plugin-updater` JS API로 한다(`web/src/Update.tsx`).
#![windows_subsystem = "windows"]

use std::io::BufRead;
use std::process::{Command, Stdio};

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_window_state::StateFlags;

const WINDOW: &str = "main";
/// `loom start`가 출력하는 줄의 앞부분. 근거: `src/launch.rs`의 `start`.
const READY_PREFIX: &str = "loom: ";

fn main() {
    tauri::Builder::default()
        // 두 번째 실행은 창을 띄우지 않고 이미 있는 창을 앞으로 가져온다.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            bring_back(app)
        }))
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
            let url = start_daemon()?;
            frame(WebviewWindowBuilder::new(
                app,
                WINDOW,
                WebviewUrl::External(url.parse()?),
            ))
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

/// 옆에 놓인 `loom start`를 부르고, 출력한 주소를 읽는다.
fn start_daemon() -> Result<String, Box<dyn std::error::Error>> {
    let exe =
        std::env::current_exe()?.with_file_name(format!("loom{}", std::env::consts::EXE_SUFFIX));
    let mut command = Command::new(exe);
    command
        .arg("start")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_console(&mut command);
    // `start`가 띄운 데몬이 이 `PATH`를 물려받는다.
    login_path(&mut command);
    let mut start = command.spawn()?;
    // 끝까지 읽지 않고 한 줄만 읽는다. Windows는 `start`가 띄운 데몬도 이 파이프를 물려받아, 끝(EOF)이 데몬과 함께 온다.
    let mut line = String::new();
    let stdout = start.stdout.take().ok_or("loom start stdout이 없다")?;
    std::io::BufReader::new(stdout).read_line(&mut line)?;
    start.wait()?;
    let url = line
        .trim()
        .strip_prefix(READY_PREFIX)
        .ok_or("데몬이 시작하지 못했다. 데이터 폴더의 serve.log를 본다")?;
    Ok(url.to_owned())
}

/// 콘솔 프로그램인 `loom start`가 새 콘솔 창을 열지 않게 한다. 데몬은 `start`가 숨은 콘솔로 띄운다(`src/launch.rs`).
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
