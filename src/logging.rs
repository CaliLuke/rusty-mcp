use std::sync::OnceLock;

use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// Configure tracing subscribers for stdout and optional file logging.
pub fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stdout_layer = fmt::layer().with_target(false).compact();

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer);

    if let Some(writer) = configure_file_writer() {
        let file_layer = fmt::layer()
            .with_writer(writer)
            .with_target(true)
            .with_ansi(false)
            .compact();

        registry.with(file_layer).init();
    } else {
        registry.init();
    }
}

fn configure_file_writer() -> Option<NonBlocking> {
    if let Ok(path) = std::env::var("RUSTY_MEM_LOG_FILE") {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(file) => {
                let (non_blocking, guard) = tracing_appender::non_blocking(file);
                let _ = LOG_GUARD.set(guard);
                Some(non_blocking)
            }
            Err(err) => {
                eprintln!("Failed to open log file {path}: {err}");
                None
            }
        }
    } else {
        if let Err(err) = std::fs::create_dir_all("logs") {
            eprintln!("Failed to create logs directory: {err}");
            return None;
        }
        let file_appender = tracing_appender::rolling::never("logs", "rusty-mem.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        let _ = LOG_GUARD.set(guard);
        Some(non_blocking)
    }
}
