//! Tracing setup: a daily-rolling log file in `%APPDATA%\AT2020 Control Panel\logs`
//! (last 7 files kept). Logging never happens inside the audio callbacks.

use crate::settings;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::EnvFilter;

/// Keep the returned guard alive for the life of the process so buffered lines flush.
pub fn init() -> Option<WorkerGuard> {
    let dir = settings::logs_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let appender = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("at2020-control")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(filter)
        .try_init()
        .ok()?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        build = crate::BUILD_ID,
        "started"
    );
    Some(guard)
}
