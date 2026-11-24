//! Automatic away: after a while with no keyboard or mouse input anywhere on the computer
//! (not just in Tightbeam), the peer sees me as away until input resumes.

use std::{sync::Arc, time::Duration};

use crate::chat::Core;

/// How long without input counts as away.
const AWAY_AFTER: Duration = Duration::from_secs(10 * 60);
/// How often to check. Coming back shows within this long.
const CHECK_EVERY: Duration = Duration::from_secs(5);

/// Checks for idleness in the background, for as long as the app runs.
pub fn watch(core: Arc<Core>) {
    tauri::async_runtime::spawn(async move {
        let mut ticks = tokio::time::interval(CHECK_EVERY);
        loop {
            ticks.tick().await;
            if let Some(idle) = idle_for() {
                core.set_idle(idle >= AWAY_AFTER);
            }
        }
    });
}

/// Time since the last keyboard or mouse input in this login session.
#[cfg(windows)]
fn idle_for() -> Option<Duration> {
    use windows_sys::Win32::{
        System::SystemInformation::GetTickCount,
        UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
    };
    let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // SAFETY: `info` is a valid LASTINPUTINFO with `cbSize` set, as the call requires.
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return None;
    }
    // Both are milliseconds since boot, wrapping after 49.7 days.
    // SAFETY: no arguments, no preconditions.
    let now = unsafe { GetTickCount() };
    Some(Duration::from_millis(now.wrapping_sub(info.dwTime).into()))
}

/// Time since the last keyboard or mouse input in this login session.
#[cfg(target_os = "macos")]
fn idle_for() -> Option<Duration> {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
    }
    const COMBINED_SESSION_STATE: i32 = 0; // kCGEventSourceStateCombinedSessionState
    const ANY_INPUT_EVENT: u32 = !0; // kCGAnyInputEventType
    // SAFETY: plain values in, a number out. Needs no permission (unlike reading events).
    let secs =
        unsafe { CGEventSourceSecondsSinceLastEventType(COMBINED_SESSION_STATE, ANY_INPUT_EVENT) };
    Duration::try_from_secs_f64(secs).ok()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn idle_for() -> Option<Duration> {
    None
}
