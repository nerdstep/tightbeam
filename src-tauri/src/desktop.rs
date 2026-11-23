//! Desktop integration: the tray icon, hiding instead of quitting when the window
//! closes, the unread count (title, tray tooltip, Dock badge / taskbar dot),
//! notifications, nudges, and when messages count as seen. Lives in Rust so it keeps
//! working while the window is hidden.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use tauri::{
    AppHandle, Manager, UserAttentionType, WebviewWindow,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_notification::NotificationExt;

use crate::{chat::Core, db::Message};

pub const MAIN_WINDOW: &str = "main";
const TRAY: &str = "main";
/// Passed by "start at login", so the app starts in the tray.
pub const MINIMIZED_ARG: &str = "--minimized";
/// Catch-up after being offline can deliver dozens of messages at once.
const NOTIFY_AT_MOST_EVERY: Duration = Duration::from_secs(4);

pub struct Desktop {
    unread: AtomicU32,
    notifications: AtomicBool,
    last_notified: Mutex<Option<Instant>>,
}

impl Desktop {
    pub fn new() -> Self {
        Self {
            unread: AtomicU32::new(0),
            notifications: AtomicBool::new(true),
            last_notified: Mutex::new(None),
        }
    }

    pub fn notifications(&self) -> bool {
        self.notifications.load(Ordering::Relaxed)
    }

    pub fn set_notifications(&self, on: bool) {
        self.notifications.store(on, Ordering::Relaxed);
    }
}

pub fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Tightbeam", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Tightbeam", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::with_id(TRAY)
        .tooltip("Tightbeam")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Called for every message the engine reports, ours included.
pub fn on_message(app: &AppHandle, message: &Message) {
    let (Some(core), Some(desktop), Some(window)) = (
        app.try_state::<Arc<Core>>(),
        app.try_state::<Desktop>(),
        app.get_webview_window(MAIN_WINDOW),
    ) else {
        return;
    };
    if message.author == core.id() {
        return;
    }
    if looking(&window) {
        let _ = core.mark_seen();
        return;
    }
    let unread = desktop.unread.fetch_add(1, Ordering::Relaxed) + 1;
    show_unread(app, &window, unread);
    if desktop.notifications() && !recently_notified(&desktop) {
        let mut body: String = message.text.chars().take(140).collect();
        if body.len() < message.text.len() {
            body.push('…');
        }
        notify(app, &title(core.peer_name(), "New message"), &body, false);
    }
}

/// The peer nudged us. While the window is open, the UI shakes it and plays a sound;
/// this flashes the taskbar or Dock icon if it's behind other windows. From the tray,
/// a notification with a sound instead.
pub fn on_nudge(app: &AppHandle) {
    let (Some(core), Some(desktop), Some(window)) = (
        app.try_state::<Arc<Core>>(),
        app.try_state::<Desktop>(),
        app.get_webview_window(MAIN_WINDOW),
    ) else {
        return;
    };
    if !window.is_visible().unwrap_or(false) {
        if desktop.notifications() {
            notify(app, &title(core.peer_name(), "Tightbeam"), "Nudged you", true);
        }
    } else if !looking(&window) {
        let _ = window.request_user_attention(Some(UserAttentionType::Informational));
    }
}

fn looking(window: &WebviewWindow) -> bool {
    window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
}

/// The window got focus: the person has seen everything. Clears the unread count and
/// tells the peer (a seen receipt).
pub fn on_focused(app: &AppHandle) {
    if let Some(core) = app.try_state::<Arc<Core>>() {
        let _ = core.mark_seen();
    }
    let Some(desktop) = app.try_state::<Desktop>() else { return };
    if desktop.unread.swap(0, Ordering::Relaxed) != 0
        && let Some(window) = app.get_webview_window(MAIN_WINDOW)
    {
        show_unread(app, &window, 0);
    }
}

fn show_unread(app: &AppHandle, window: &WebviewWindow, unread: u32) {
    let (title, tooltip) = if unread == 0 {
        ("Tightbeam".to_string(), "Tightbeam".to_string())
    } else {
        (format!("Tightbeam ({unread})"), format!("Tightbeam — {unread} unread"))
    };
    let _ = window.set_title(&title);
    // The tray icon gets a dot too: with the window hidden there's no taskbar button.
    if let Some(tray) = app.tray_by_id(TRAY) {
        let _ = tray.set_tooltip(Some(tooltip));
        if let Some(icon) = app.default_window_icon() {
            let _ = tray.set_icon(Some(if unread > 0 { with_dot(icon) } else { icon.clone() }));
        }
    }
    // Windows has no badge count; a dot over the taskbar icon instead.
    #[cfg(target_os = "windows")]
    let _ = window.set_overlay_icon((unread > 0).then(unread_dot));
    #[cfg(not(target_os = "windows"))]
    let _ = window.set_badge_count((unread > 0).then_some(i64::from(unread)));
}

/// True if a message notification went out moments ago; otherwise records one now.
fn recently_notified(desktop: &Desktop) -> bool {
    let mut last = desktop.last_notified.lock().unwrap();
    if last.is_some_and(|t| t.elapsed() < NOTIFY_AT_MOST_EVERY) {
        return true;
    }
    *last = Some(Instant::now());
    false
}

/// The peer's name, or `fallback` if they have none.
fn title(peer_name: Option<String>, fallback: &str) -> String {
    peer_name.filter(|n| !n.is_empty()).unwrap_or_else(|| fallback.into())
}

/// `sound`: Windows always plays one; macOS only when asked.
fn notify(app: &AppHandle, title: &str, body: &str, sound: bool) {
    let mut builder = app.notification().builder().title(title).body(body);
    if sound && cfg!(target_os = "macos") {
        builder = builder.sound("NSUserNotificationDefaultSoundName");
    }
    if let Err(e) = builder.show() {
        eprintln!("notification failed: {e}");
    }
}

const DOT_COLOR: [u8; 3] = [0xe5, 0x48, 0x4d];

/// Coverage (0–1) of a soft-edged disc at pixel (x, y).
fn disc(x: u32, y: u32, center: (f32, f32), radius: f32) -> f32 {
    let distance = ((x as f32 - center.0).powi(2) + (y as f32 - center.1).powi(2)).sqrt();
    (radius - distance + 0.5).clamp(0.0, 1.0)
}

/// Blends `color` at `alpha` over the RGBA pixel at `i`.
fn blend(rgba: &mut [u8], i: usize, color: [u8; 3], alpha: f32) {
    for c in 0..3 {
        rgba[i + c] = (color[c] as f32 * alpha + rgba[i + c] as f32 * (1.0 - alpha)) as u8;
    }
    rgba[i + 3] = (alpha * 255.0 + rgba[i + 3] as f32 * (1.0 - alpha)) as u8;
}

/// The app icon with an unread dot in the top-right corner, ringed so it stands out
/// against the icon.
fn with_dot(icon: &tauri::image::Image<'_>) -> tauri::image::Image<'static> {
    let (w, h) = (icon.width(), icon.height());
    let mut rgba = icon.rgba().to_vec();
    let radius = w as f32 * 0.24;
    let ring = (w as f32 * 0.07).max(1.0);
    let center = (w as f32 - radius - ring, radius + ring);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let outer = disc(x, y, center, radius + ring);
            if outer > 0.0 {
                blend(&mut rgba, i, [0x0b, 0x0d, 0x1a], outer);
                blend(&mut rgba, i, DOT_COLOR, disc(x, y, center, radius));
            }
        }
    }
    tauri::image::Image::new_owned(rgba, w, h)
}

/// A 16×16 dot for the Windows taskbar overlay.
#[cfg(target_os = "windows")]
fn unread_dot() -> tauri::image::Image<'static> {
    const SIZE: u32 = 16;
    let center = ((SIZE as f32 - 1.0) / 2.0, (SIZE as f32 - 1.0) / 2.0);
    let mut rgba = vec![0; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let i = ((y * SIZE + x) * 4) as usize;
            blend(&mut rgba, i, DOT_COLOR, disc(x, y, center, SIZE as f32 / 2.0 - 0.5));
        }
    }
    tauri::image::Image::new_owned(rgba, SIZE, SIZE)
}
