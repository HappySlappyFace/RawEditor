use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// Daily log files kept in release builds before the oldest is deleted.
#[cfg(not(debug_assertions))]
const MAX_LOG_FILES: usize = 14;

/// `RUST_LOG` when it is set, otherwise `default`. Lets a user turn on
/// debug logging for a bug report without a rebuild — the filter used to be
/// hardcoded in both build profiles.
fn filter(default: &str) -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default))
}

/// Route panics through `tracing` as well as stderr. A GUI app's stderr goes
/// nowhere for most users, so without this a crash left no trace in the log
/// file that a bug report would include.
fn log_panics() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        default_hook(info);
    }));
}

pub fn init() -> Option<WorkerGuard> {
    log_panics();

    #[cfg(debug_assertions)]
    {
        // Debug mode: Write to stdout with pretty formatting
        tracing_subscriber::fmt()
            .with_env_filter(filter("raw_editor=debug,raw_editor_core=debug,iced=warn,wgpu=warn"))
            .with_target(false)
            .pretty()
            .init();
        None
    }

    #[cfg(not(debug_assertions))]
    {
        // Release mode: Write to rolling log files
        let log_dir = crate::core::paths::data_dir().join("logs");
        // Before `build`: its old-file pruning lists the directory first, and
        // complains on stderr when it does not exist yet (first launch).
        if let Err(e) = std::fs::create_dir_all(&log_dir) {
            eprintln!("Failed to create log directory: {e}");
        }

        // Same file names as before (`raw-editor.log.YYYY-MM-DD`), but capped:
        // `rolling::daily` never deleted anything, so the directory grew by a
        // file a day forever.
        let file_appender = match tracing_appender::rolling::RollingFileAppender::builder()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("raw-editor.log")
            .max_log_files(MAX_LOG_FILES)
            .build(&log_dir)
        {
            Ok(appender) => appender,
            Err(e) => {
                eprintln!("Failed to open log directory {}: {e}", log_dir.display());
                return None;
            }
        };
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

        tracing_subscriber::fmt()
            .with_env_filter(filter("raw_editor=info,raw_editor_core=info,warn"))
            .with_writer(non_blocking)
            .with_ansi(false)
            .init();

        Some(guard)
    }
}
