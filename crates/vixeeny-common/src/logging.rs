// SPDX-License-Identifier: GPL-3.0-or-later
//! Rotating log files in the OS log directory (plan 0.3). `RUST_LOG` overrides the `info`
//! default. The writer is synchronous: no helper thread, no wake-ups while idle.

use std::sync::Mutex;

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

/// Initialises logging to `<log dir>/<name>.<date>`, keeping a week of files. Failing to open
/// the log directory is not fatal: the daemon then runs without logs.
pub fn init(name: &str) {
    let Some(dir) = crate::paths::log_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(name)
        .filename_suffix("log")
        .max_log_files(7)
        .build(dir);
    let Ok(appender) = appender else { return };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // Ignore the error: a subscriber may already be installed (tests).
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(Mutex::new(appender))
        .try_init();
    // The programs have no console (Windows subsystem): a panic would vanish without a trace.
    std::panic::set_hook(Box::new(|info| {
        tracing::error!("panic: {info}");
    }));
}
