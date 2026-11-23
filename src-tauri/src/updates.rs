//! Automatic updates from GitHub Releases.
//!
//! The Release workflow publishes signed bundles and a `latest.json`; the URL and the
//! public key are in tauri.conf.json (`plugins.updater`). Updates download in the
//! background and are verified against that key, then install when the person chooses
//! to restart.
//!
//! This is the only connection Tightbeam makes beyond the LAN, so it can be turned off
//! (the "updates" preference). Development and test builds never check.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, bail};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};
use tauri_plugin_window_state::AppHandleExt;

use crate::{WINDOW_STATE, chat::Core};

/// Emitted with [`Ready`] once an update has downloaded.
pub const READY_EVENT: &str = "update-ready";
/// Leave startup alone; the network and the window come first.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// A downloaded, verified update waiting for a restart.
#[derive(Clone, Serialize)]
pub struct Ready {
    pub version: String,
}

pub struct Updates {
    auto: AtomicBool,
    pending: Mutex<Option<(Update, Vec<u8>)>>,
    /// Held while checking, so the timer and "Check for updates" never download twice.
    checking: tokio::sync::Mutex<()>,
}

/// Release builds only, outside test mode.
pub fn available() -> bool {
    !cfg!(debug_assertions) && !crate::test_mode()
}

impl Updates {
    pub fn new(auto: bool) -> Self {
        Self {
            auto: AtomicBool::new(auto),
            pending: Mutex::new(None),
            checking: tokio::sync::Mutex::new(()),
        }
    }

    pub fn auto(&self) -> bool {
        self.auto.load(Ordering::Relaxed)
    }

    pub fn set_auto(&self, on: bool) {
        self.auto.store(on, Ordering::Relaxed);
    }

    pub fn ready(&self) -> Option<Ready> {
        self.pending
            .lock()
            .unwrap()
            .as_ref()
            .map(|(update, _)| Ready { version: update.version.clone() })
    }
}

/// Starts the background checks, if updates are available in this build.
pub fn start(app: &AppHandle) {
    if !available() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_AFTER).await;
        loop {
            // Being offline is normal; the next check will try again.
            if app.state::<Updates>().auto()
                && let Err(e) = check(&app).await
            {
                eprintln!("update check failed: {e:#}");
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// Checks for an update and downloads it. `Some` once one is ready to install.
pub async fn check(app: &AppHandle) -> Result<Option<Ready>> {
    if !available() {
        bail!("updates are off in development and test builds");
    }
    let updates = app.state::<Updates>();
    let _checking = updates.checking.lock().await;
    if let Some(ready) = updates.ready() {
        return Ok(Some(ready));
    }
    let handle = app.clone();
    let updater = app.updater_builder().on_before_exit(move || before_exit(&handle)).build()?;
    let Some(update) = updater.check().await? else {
        return Ok(None);
    };
    // Verified against the public key as part of the download.
    let bytes = update.download(|_, _| {}, || {}).await?;
    let ready = Ready { version: update.version.clone() };
    *updates.pending.lock().unwrap() = Some((update, bytes));
    let _ = app.emit(READY_EVENT, &ready);
    Ok(Some(ready))
}

/// Installs the downloaded update and restarts into it.
///
/// On Windows the installer replaces the app, so the process exits inside
/// `Update::install`, after `before_exit`. On macOS the app bundle is replaced in place
/// and the app restarts through the normal exit (which shuts the engine down).
pub fn install(app: &AppHandle) -> Result<()> {
    let Some((update, bytes)) = app.state::<Updates>().pending.lock().unwrap().take() else {
        bail!("no update has been downloaded");
    };
    update.install(&bytes)?;
    app.request_restart();
    Ok(())
}

/// Windows, just before the installer takes over: what a normal exit would do.
fn before_exit(app: &AppHandle) {
    let _ = app.save_window_state(WINDOW_STATE);
    if let Some(core) = app.try_state::<Arc<Core>>() {
        let core = Arc::clone(&core);
        // On its own thread: this can run on an async runtime thread, where block_on panics.
        let _ = std::thread::spawn(move || tauri::async_runtime::block_on(core.shutdown())).join();
    }
}
