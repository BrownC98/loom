#![windows_subsystem = "windows"]

use std::io::BufRead;
use std::process::{Command, Stdio};

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_window_state::StateFlags;

const WINDOW: &str = "main";
const READY_PREFIX: &str = "loom: ";
const UPDATE_FLAG: &str = "--update";
const UPDATE_QUERY: &str = "update";

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if args.iter().any(|a| a == UPDATE_FLAG)
                && let Some(window) = app.get_webview_window(WINDOW)
                && let Ok(url) = window.url()
            {
                let _ = window.navigate(for_update(url));
            }
            bring_back(app)
        }))
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::all() - StateFlags::DECORATIONS)
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
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
            let handle = app.handle().clone();
            app.notification().on_action(move |_| bring_back(&handle))?;
            let mut url: tauri::Url = start_daemon()?.parse()?;
            if std::env::args().any(|a| a == UPDATE_FLAG) {
                url = for_update(url);
            }
            frame(WebviewWindowBuilder::new(
                app,
                WINDOW,
                WebviewUrl::External(url),
            ))
            .title(format!("loom {}", app.package_info().version))
            .inner_size(1280.0, 800.0)
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

fn for_update(mut url: tauri::Url) -> tauri::Url {
    url.set_query(Some(UPDATE_QUERY));
    url
}

fn bring_back(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[cfg(not(target_os = "macos"))]
fn frame<'a, R: tauri::Runtime, M: Manager<R>>(
    window: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    window.decorations(false)
}

#[cfg(target_os = "macos")]
const TRAFFIC_LIGHTS: tauri::LogicalPosition<f64> = tauri::LogicalPosition::new(14.0, 18.0);

#[cfg(target_os = "macos")]
fn frame<'a, R: tauri::Runtime, M: Manager<R>>(
    window: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    window
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(TRAFFIC_LIGHTS)
}

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
    login_path(&mut command);
    let mut start = command.spawn()?;
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

#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_: &mut Command) {}

#[cfg(unix)]
const PATH_MARK: &str = "loom-path:";

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
