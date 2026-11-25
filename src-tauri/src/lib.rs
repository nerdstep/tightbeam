pub mod chat;
mod clock;
pub mod db;
mod desktop;
pub mod entry;
mod files;
mod idle;
mod net;
mod updates;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use chat::{ConnectionInfo, Core, Emit, Event, NewFile, Status};
use db::{Cursor, Hit, Message, Page};
use desktop::{Desktop, MAIN_WINDOW, MINIMIZED_ARG};
use entry::{MAX_FILE_BYTES, Ref};
use serde::Serialize;
use tauri::{
    Emitter, Manager, RunEvent, State, WindowEvent,
    http::{Response, StatusCode, header},
    ipc::InvokeBody,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as _};
use tauri_plugin_dialog::DialogExt;
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

/// Messages from `from` up to `before`: fills the gap back to a search hit.
#[tauri::command]
fn get_messages_between(
    core: CoreState,
    from: Cursor,
    before: Cursor,
) -> Result<Vec<Message>, String> {
    core.range(&from, &before).map_err(err)
}

/// Messages matching `query`, newest first.
#[tauri::command]
fn search(core: CoreState, query: String) -> Result<Vec<Hit>, String> {
    core.search(&query).map_err(err)
}

#[tauri::command]
fn set_name(core: CoreState, name: String) -> Result<(), String> {
    core.set_name(&name).map_err(err)
}

/// `reply_to`: the message this one answers. `vanish`: seconds it lasts once seen.
#[tauri::command]
fn send_message(
    core: CoreState,
    text: String,
    reply_to: Option<Ref>,
    vanish: Option<u64>,
) -> Result<Message, String> {
    core.send_message(&text, reply_to, vanish).map_err(err)
}

/// Sends a file. The body is its bytes; the `x-tightbeam-file` header is a `NewFile` as
/// JSON (ASCII only: the UI escapes the rest).
#[tauri::command]
async fn send_file(
    core: State<'_, Arc<Core>>,
    request: tauri::ipc::Request<'_>,
) -> Result<Message, String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("expected the file's bytes".into());
    };
    let details = request
        .headers()
        .get("x-tightbeam-file")
        .and_then(|v| v.to_str().ok())
        .ok_or("missing the file's details")?;
    let new: NewFile = serde_json::from_str(details).map_err(err)?;
    let (core, bytes) = (core.inner().clone(), bytes.clone());
    // Hashing and writing a big file takes a moment.
    tauri::async_runtime::spawn_blocking(move || core.send_file(&bytes, new))
        .await
        .map_err(err)?
        .map_err(err)
}

/// Files from disk waiting in the message box, by id. They go by path, never through the
/// webview, so a big one isn't read into memory.
#[derive(Default)]
struct Staging(Mutex<StagingState>);

#[derive(Default)]
struct StagingState {
    next: u64,
    files: HashMap<u64, Staged>,
}

struct Staged {
    path: PathBuf,
    mime: &'static str,
    size: u64,
}

/// A file from disk, staged, as the UI sees it.
#[derive(Serialize)]
struct StagedFile {
    id: u64,
    name: String,
    size: u64,
    mime: String,
}

/// What came of staging some paths: the ones staged, and why the others weren't.
#[derive(Serialize)]
struct StagedFiles {
    staged: Vec<StagedFile>,
    refused: Vec<String>,
}

impl Staging {
    fn add(&self, paths: impl IntoIterator<Item = PathBuf>) -> StagedFiles {
        let (mut staged, mut refused) = (Vec::new(), Vec::new());
        let mut st = self.0.lock().unwrap();
        for path in paths {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            let name = name.unwrap_or_else(|| path.to_string_lossy().into_owned());
            match std::fs::metadata(&path) {
                Ok(m) if m.is_dir() => {
                    refused.push(format!("{name} is a folder. Only files can be sent."))
                }
                Ok(m) if m.len() == 0 => refused.push(format!("{name} is empty.")),
                Ok(m) if m.len() > MAX_FILE_BYTES => {
                    refused.push(format!("{name} is too big. Files can be up to 1 GB."))
                }
                Ok(m) => {
                    st.next += 1;
                    let (id, mime, size) = (st.next, files::mime_for(&name), m.len());
                    st.files.insert(id, Staged { path, mime, size });
                    staged.push(StagedFile { id, name, size, mime: mime.into() });
                }
                Err(_) => refused.push(format!("{name} can't be read.")),
            }
        }
        StagedFiles { staged, refused }
    }
}

/// Stages files dropped on the window.
#[tauri::command]
fn stage_paths(staging: State<Staging>, paths: Vec<PathBuf>) -> StagedFiles {
    staging.add(paths)
}

/// Asks which files to send, then stages them.
#[tauri::command]
async fn pick_files(
    app: tauri::AppHandle,
    staging: State<'_, Staging>,
) -> Result<StagedFiles, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_title("Send files").pick_files(move |chosen| {
        let _ = tx.send(chosen);
    });
    let chosen = rx.await.map_err(err)?.unwrap_or_default();
    Ok(staging.add(chosen.into_iter().filter_map(|p| p.into_path().ok())))
}

/// Sends a staged file. It stays staged if that fails, so it can be tried again.
#[tauri::command]
async fn send_staged(
    core: State<'_, Arc<Core>>,
    staging: State<'_, Staging>,
    id: u64,
    details: NewFile,
) -> Result<Message, String> {
    let path = staging.0.lock().unwrap().files.get(&id).map(|s| s.path.clone());
    let path = path.ok_or("That file isn't waiting to be sent any more.")?;
    let core = core.inner().clone();
    // Hashing and copying a big file takes a moment.
    let sent = tauri::async_runtime::spawn_blocking(move || core.send_file_from(&path, details))
        .await
        .map_err(err)?
        .map_err(err)?;
    staging.0.lock().unwrap().files.remove(&id);
    Ok(sent)
}

/// Takes a file out of the message box.
#[tauri::command]
fn unstage(staging: State<Staging>, id: u64) {
    staging.0.lock().unwrap().files.remove(&id);
}

/// Previews of staged pictures are made from at most this much.
const MAX_PREVIEW_SOURCE: u64 = 50 * 1024 * 1024;

/// A staged picture's bytes, for the webview to make its preview. Only pictures it can
/// show, up to 50 MB; nothing else on disk. Raw bytes over IPC, not a fetch from a URI
/// scheme: that would be cross-origin, and the webview would refuse it without CORS.
#[tauri::command]
async fn staged_picture(
    staging: State<'_, Staging>,
    id: u64,
) -> Result<tauri::ipc::Response, String> {
    let path = {
        let st = staging.0.lock().unwrap();
        let s = st.files.get(&id).ok_or("That file isn't waiting to be sent any more.")?;
        let fits = files::may_show(s.mime) && s.size <= MAX_PREVIEW_SOURCE;
        fits.then(|| s.path.clone()).ok_or("That file has no preview.")?
    };
    let bytes = tauri::async_runtime::spawn_blocking(move || std::fs::read(path))
        .await
        .map_err(err)?
        .map_err(err)?;
    Ok(tauri::ipc::Response::new(bytes))
}

fn uncached(status: StatusCode, mime: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .body(body)
        .unwrap_or_default()
}

/// Kinds of file that open in their default app. Anything else, like a program, can only
/// be shown in its folder.
const OPENABLE: &[&str] =
    &["jpg", "jpeg", "png", "gif", "webp", "avif", "bmp", "heic", "heif", "tif", "tiff"];

/// Opens a received or sent picture in its default app.
#[tauri::command]
fn open_file(app: tauri::AppHandle, core: CoreState, hash: String) -> Result<(), String> {
    let (path, _) = core.file(&hash).map_err(err)?.ok_or(MISSING)?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_lowercase();
    if !OPENABLE.contains(&ext.as_str()) {
        return Err("Only pictures can be opened from here. Use Show in folder.".into());
    }
    app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(err)
}

/// Shows a file in Explorer or Finder.
#[tauri::command]
fn show_file(app: tauri::AppHandle, core: CoreState, hash: String) -> Result<(), String> {
    let (path, _) = core.file(&hash).map_err(err)?.ok_or(MISSING)?;
    app.opener().reveal_item_in_dir(path).map_err(err)
}

/// Copies a file to where the person chooses, starting in Pictures. `false` if they
/// cancelled.
#[tauri::command]
async fn save_file_copy(
    app: tauri::AppHandle,
    core: State<'_, Arc<Core>>,
    hash: String,
) -> Result<bool, String> {
    let (path, _) = core.file(&hash).map_err(err)?.ok_or(MISSING)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut dialog = app.dialog().file().set_title("Save a copy").set_file_name(&name);
    if let Ok(dir) = app.path().picture_dir().or_else(|_| app.path().download_dir()) {
        dialog = dialog.set_directory(dir);
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        dialog = dialog.add_filter(ext.to_uppercase(), &[ext]);
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    dialog.save_file(move |chosen| {
        let _ = tx.send(chosen);
    });
    let Some(chosen) = rx.await.map_err(err)? else { return Ok(false) };
    let to = chosen.into_path().map_err(err)?;
    tauri::async_runtime::spawn_blocking(move || std::fs::copy(path, to))
        .await
        .map_err(err)?
        .map_err(err)?;
    Ok(true)
}

/// For a file that was moved or changed outside the app: `Core::file` has already asked
/// for it again.
const MISSING: &str =
    "That file was moved or changed outside Tightbeam, so it's being fetched again.";

/// `tbfile://localhost/<hash>` (`http://tbfile.localhost/<hash>` on Windows): a stored
/// file, for the webview to show. Only files still attached to a message, and never
/// cached, so a deleted picture doesn't linger in the webview's cache.
fn file_response(app: &tauri::AppHandle, path: &str) -> Response<Vec<u8>> {
    let respond = uncached;
    let hash = path.trim_start_matches('/');
    let found = app.try_state::<Arc<Core>>().and_then(|core| core.file(hash).ok().flatten());
    let Some((file, mime)) = found else {
        return respond(StatusCode::NOT_FOUND, "text/plain", Vec::new());
    };
    let mime = if files::may_show(&mime) { mime } else { "application/octet-stream".into() };
    match std::fs::read(file) {
        Ok(bytes) => respond(StatusCode::OK, &mime, bytes),
        Err(_) => respond(StatusCode::NOT_FOUND, "text/plain", Vec::new()),
    }
}

/// `/status`.
#[tauri::command]
fn connection_info(core: CoreState) -> ConnectionInfo {
    core.connection_info()
}

/// `/dnd`: `until` in ms, or `None` to turn it off.
#[tauri::command]
fn set_dnd(core: CoreState, until: Option<i64>) -> Result<(), String> {
    core.set_dnd(until).map_err(err)
}

/// Sets my reaction to a message; `emoji` "" takes it back.
#[tauri::command]
fn react(core: CoreState, to: Ref, emoji: String) -> Result<(), String> {
    core.react(to, &emoji).map_err(err)
}

/// Replaces the text of one of my messages.
#[tauri::command]
fn edit_message(core: CoreState, of: Ref, text: String) -> Result<(), String> {
    core.edit(of, &text).map_err(err)
}

/// Deletes one of my messages, for both people.
#[tauri::command]
fn delete_message(core: CoreState, of: Ref) -> Result<(), String> {
    core.delete(of).map_err(err)
}

/// `/away`, with an optional note.
#[tauri::command]
fn set_away(core: CoreState, note: String) {
    core.set_away(&note);
}

/// `/back`.
#[tauri::command]
fn back(core: CoreState) {
    core.back();
}

#[tauri::command]
fn typing(core: CoreState) {
    core.typing()
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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .register_asynchronous_uri_scheme_protocol("tbfile", |ctx, request, responder| {
            let (app, path) = (ctx.app_handle().clone(), request.uri().path().to_string());
            // Reading a big file shouldn't hold up the webview.
            tauri::async_runtime::spawn_blocking(move || {
                responder.respond(file_response(&app, &path));
            });
        })
        .setup(|app| {
            // To run two copies on one machine: give each its own TIGHTBEAM_DATA_DIR and
            // set TIGHTBEAM_LOOPBACK=1 (see Core::start_with for why).
            let dir = match std::env::var_os("TIGHTBEAM_DATA_DIR") {
                Some(dir) => PathBuf::from(dir),
                None => app.path().app_data_dir()?,
            };
            let loopback = std::env::var_os("TIGHTBEAM_LOOPBACK").is_some();

            app.manage(Desktop::new());
            app.manage(Staging::default());
            let handle = app.handle().clone();
            let emit: Emit = Arc::new(move |event| {
                match &event {
                    Event::Message(message) => desktop::on_message(&handle, message),
                    Event::Nudge => desktop::on_nudge(&handle),
                    Event::Status(_)
                    | Event::Updated(_)
                    | Event::Typing
                    | Event::Transfer { .. } => {}
                }
                let _ = handle.emit("tightbeam", event);
            });
            let core = tauri::async_runtime::block_on(Core::start_with(dir, emit, loopback))?;
            let notifications = core.pref("notifications")?.as_deref() != Some("0");
            app.state::<Desktop>().set_notifications(notifications);
            app.manage(Updates::new(core.pref("updates")?.as_deref() != Some("0")));
            idle::watch(core.clone());
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
            get_messages_between,
            search,
            set_name,
            send_message,
            send_file,
            stage_paths,
            pick_files,
            send_staged,
            staged_picture,
            unstage,
            open_file,
            show_file,
            save_file_copy,
            react,
            edit_message,
            delete_message,
            typing,
            set_away,
            back,
            set_dnd,
            connection_info,
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
