//! Wall-clock time, one source for the engine and the log.
//!
//! The app reads the system clock. Tests use a clock that follows tokio's paused time,
//! so the waits (`sleep`, `timeout`) and the wall clock move together when a test
//! advances time.

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub(crate) struct Clock(Source);

#[derive(Debug, Clone)]
enum Source {
    System,
    /// `epoch_ms` at `start`, then moving with tokio's clock.
    #[cfg(test)]
    Tokio {
        epoch_ms: i64,
        start: tokio::time::Instant,
    },
}

impl Clock {
    pub fn system() -> Self {
        Self(Source::System)
    }

    /// Starts at `epoch_ms` now, then follows tokio's clock (paused, in tests).
    #[cfg(test)]
    pub fn tokio(epoch_ms: i64) -> Self {
        Self(Source::Tokio { epoch_ms, start: tokio::time::Instant::now() })
    }

    /// Milliseconds since the Unix epoch.
    pub fn now_ms(&self) -> i64 {
        match &self.0 {
            Source::System => {
                SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
            }
            #[cfg(test)]
            Source::Tokio { epoch_ms, start } => epoch_ms + start.elapsed().as_millis() as i64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn follows_paused_time() {
        let clock = Clock::tokio(1_000);
        assert_eq!(clock.now_ms(), 1_000);
        tokio::time::sleep(std::time::Duration::from_secs(90)).await;
        assert_eq!(clock.now_ms(), 91_000);
    }
}
