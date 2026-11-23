pub mod chat;
pub mod db;
mod desktop;
mod updates;

use std::{path::PathBuf, sync::Arc};

use chat::{Core, Emit, Event, Status};
use db::{Cursor, Message, Page};
use desktop::{Desktop, MAIN_WINDOW, MINIMIZED_ARG};
use serde::Serialize;
use tauri::{Emitter, Manager, RunEvent, State, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as _};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_window_state::StateFlags;
use updates::{Ready, Updates};

type CoreState<'a> = State<'a, Arc<Core>>;

/// What window-state remembers. Not visibility: quitting from the tray would make the
/// next launch hidden.
const WINDOW_STATE: StateFlags = StateFlags::all().difference(StateFlags::VISIBLE);

fn err(e: impl std::fmt::Display) -> String {
    format!("{e:#}")
}

/// Running with TIGHTBEAM_DATA_DIR (two copies on one machine for testing): skip
/// single-instance, window memory and start-at-login, which are per-app, not per-copy.
pub(crate) fn test_mode() -> bool {
    std::env::var_os("TIGHTBEAM_DATA_DIR").is_some()
}

#[tauri::command]
fn get_status(core: CoreState) -> Status {
    core.status()
}

/// A page of history: the newest messages, or those before `before`. Oldest first.
#[tauri::command]
fn get_messages(
    core: CoreState,
    before: Option<Cursor>,
    limit: Option<usize>,
) -> Result<Page, String> {
    let limit = limit.unwrap_or(200).clamp(1, 1000);
    core.page(before.as_ref(), limit).map_err(err)
}

#[tauri::command]
fn set_name(core: CoreState, name: String) -> Result<(), String> {
    core.set_name(&name).map_err(err)
}

#[tauri::command]
fn send_message(core: CoreState, text: String) -> Result<Message, String> {
    core.send_message(&text).map_err(err)
}

/// Erases the conversation on this computer only.
#[tauri::command]
fn clear_history(core: CoreState) -> Result<(), String> {
    core.clear_history().map_err(err)
}

#[tauri::command]
fn nudge(core: CoreState) -> Result<(), String> {
    core.nudge().map_err(err)
}

#[tauri::command]
fn pair_with(core: CoreState, id: String) -> Result<(), String> {
    core.pair_with(&id).map_err(err)
}

#[tauri::command]
fn answer_pair(core: CoreState, accept: bool) {
    core.answer_pair(accept)
}

#[tauri::command]
fn unpair(core: CoreState) -> Result<(), String> {
    core.unpair().map_err(err)
}

#[tauri::command]
fn dismiss_notice(core: CoreState) {
    core.dismiss_notice()
}

#[derive(Serialize)]
struct Prefs {
    notifications: bool,
    /// `None` when start-at-login isn't available (test mode).
    start_at_login: Option<bool>,
    /// Automatic update checks. `None` when updates aren't available (dev and test builds).
    updates: Option<bool>,
}

#[tauri::command]
fn get_prefs(app: tauri::AppHandle, desktop: State<Desktop>, updates: State<Updates>) -> Prefs {
    Prefs {
        notifications: desktop.notifications(),
        start_at_login: (!test_mode()).then(|| app.autolaunch().is_enabled().unwrap_or(false)),
        updates: updates::available().then(|| updates.auto()),
    }
}

#[tauri::command]
fn set_auto_updates(core: CoreState, updates: State<Updates>, on: bool) -> Result<(), String> {
    updates.set_auto(on);
    core.set_pref("updates", if on { "1" } else { "0" }).map_err(err)
}

/// Checks now. `None` means up to date.
#[tauri::command]
async fn check_for_updates(app: tauri::AppHandle) -> Result<Option<Ready>, String> {
    updates::check(&app).await.map_err(err)
}

/// The update that's downloaded and waiting for a restart, if any.
#[tauri::command]
fn get_update(updates: State<Updates>) -> Option<Ready> {
    updates.ready()
}

#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    // Unpacking the update is blocking work.
    tauri::async_runtime::spawn_blocking(move || updates::install(&app))
        .await
        .map_err(err)?
        .map_err(err)
}

#[tauri::command]
fn set_notifications(core: CoreState, desktop: State<Desktop>, on: bool) -> Result<(), String> {
    desktop.set_notifications(on);
    core.set_pref("notifications", if on { "1" } else { "0" }).map_err(err)
}

#[tauri::command]
fn set_start_at_login(app: tauri::AppHandle, on: bool) -> Result<(), String> {
    if test_mode() {
        return Err("start at login is off in test mode".into());
    }
    let autolaunch = app.autolaunch();
    if on { autolaunch.enable() } else { autolaunch.disable() }.map_err(err)
}

/// Quits for real (closing the window only hides it). The engine shuts down on exit.
#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

/// Opens a link from a message in the default browser or mail app.
#[tauri::command]
fn open_link(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let allowed = ["https://", "http://", "mailto:"];
    if !allowed.iter().any(|p| url.to_ascii_lowercase().starts_with(p)) {
        return Err("only web and email links can be opened".into());
    }
    app.opener().open_url(url, None::<&str>).map_err(err)
}

pub fn run() {
    let mut builder = tauri::Builder::default();
    if !test_mode() {
        builder = builder
            // Must be first: a second launch just brings back the running window.
            .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| desktop::show_main(app)))
            .plugin(
                tauri_plugin_window_state::Builder::default()
                    .with_state_flags(WINDOW_STATE)
                    .build(),
            )
            .plugin(tauri_plugin_autostart::init(
                MacosLauncher::LaunchAgent,
                Some(vec![MINIMIZED_ARG]),
            ));
    }

    builder
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // To run two copies on one machine: give each its own TIGHTBEAM_DATA_DIR and
            // set TIGHTBEAM_LOOPBACK=1 (see Core::start_with for why).
            let dir = match std::env::var_os("TIGHTBEAM_DATA_DIR") {
                Some(dir) => PathBuf::from(dir),
                None => app.path().app_data_dir()?,
            };
            let loopback = std::env::var_os("TIGHTBEAM_LOOPBACK").is_some();

            app.manage(Desktop::new());
            let handle = app.handle().clone();
            let emit: Emit = Arc::new(move |event| {
                match &event {
                    Event::Message(message) => desktop::on_message(&handle, message),
                    Event::Nudge => desktop::on_nudge(&handle),
                    Event::Status(_) => {}
                }
                let _ = handle.emit("tightbeam", event);
            });
            let core = tauri::async_runtime::block_on(Core::start_with(dir, emit, loopback))?;
            let notifications = core.pref("notifications")?.as_deref() != Some("0");
            app.state::<Desktop>().set_notifications(notifications);
            app.manage(Updates::new(core.pref("updates")?.as_deref() != Some("0")));
            app.manage(core);
            updates::start(app.handle());

            desktop::setup_tray(app.handle())?;
            // The window starts hidden (tauri.conf.json); show it unless we were started
            // at login, in which case we stay in the tray.
            if !std::env::args().any(|a| a == MINIMIZED_ARG) {
                desktop::show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Closing the window hides it; the app keeps running in the tray so messages
            // still arrive. Quit from the tray (or Cmd+Q on macOS).
            WindowEvent::CloseRequested { api, .. } if window.label() == MAIN_WINDOW => {
                api.prevent_close();
                let _ = window.hide();
            }
            WindowEvent::Focused(true) => desktop::on_focused(window.app_handle()),
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_messages,
            set_name,
            send_message,
            clear_history,
            nudge,
            pair_with,
            answer_pair,
            unpair,
            dismiss_notice,
            get_prefs,
            set_notifications,
            set_start_at_login,
            set_auto_updates,
            check_for_updates,
            get_update,
            install_update,
            quit,
            open_link,
        ])
        .build(tauri::generate_context!())
        .expect("error while starting Tightbeam")
        .run(|app, event| match event {
            // macOS: clicking the Dock icon while the window is hidden.
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => desktop::show_main(app),
            RunEvent::Exit => {
                if let Some(core) = app.try_state::<Arc<Core>>() {
                    tauri::async_runtime::block_on(core.shutdown());
                }
            }
            _ => {}
        });
}
