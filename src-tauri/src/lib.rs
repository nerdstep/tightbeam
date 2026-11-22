pub mod chat;
pub mod db;

use std::{path::PathBuf, sync::Arc};

use chat::{Core, Emit, Status};
use db::Message;
use tauri::{Emitter, Manager, State};

type CoreState<'a> = State<'a, Arc<Core>>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
fn get_status(core: CoreState) -> Status {
    core.status()
}

#[tauri::command]
fn get_messages(core: CoreState) -> Result<Vec<Message>, String> {
    core.messages().map_err(err)
}

#[tauri::command]
fn set_name(core: CoreState, name: String) -> Result<(), String> {
    core.set_name(&name).map_err(err)
}

#[tauri::command]
fn send_message(core: CoreState, text: String) -> Result<Message, String> {
    core.send_message(&text).map_err(err)
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

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // To run two copies on one machine: give each its own TIGHTBEAM_DATA_DIR and
            // set TIGHTBEAM_LOOPBACK=1 (see Core::start_with for why).
            let dir = match std::env::var_os("TIGHTBEAM_DATA_DIR") {
                Some(dir) => PathBuf::from(dir),
                None => app.path().app_data_dir()?,
            };
            let loopback = std::env::var_os("TIGHTBEAM_LOOPBACK").is_some();
            let handle = app.handle().clone();
            let emit: Emit = Arc::new(move |event| {
                let _ = handle.emit("tightbeam", event);
            });
            let core = tauri::async_runtime::block_on(Core::start_with(dir, emit, loopback))?;
            app.manage(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_messages,
            set_name,
            send_message,
            pair_with,
            answer_pair,
            unpair,
            dismiss_notice,
        ])
        .build(tauri::generate_context!())
        .expect("error while starting Tightbeam")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event
                && let Some(core) = app.try_state::<Arc<Core>>()
            {
                tauri::async_runtime::block_on(core.shutdown());
            }
        });
}
