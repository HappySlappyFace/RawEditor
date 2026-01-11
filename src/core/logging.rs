use tracing_appender::non_blocking::WorkerGuard;

pub fn init() -> Option<WorkerGuard> {
    #[cfg(debug_assertions)]
    {
        // Debug mode: Write to stdout with pretty formatting
        tracing_subscriber::fmt()
            .with_env_filter("raw_editor=debug,iced=warn,wgpu=warn")
            .with_target(false)
            .pretty()
            .init();
        None
    }

    #[cfg(not(debug_assertions))]
    {
        // Release mode: Write to rolling log files
        if let Some(data_dir) = dirs::data_dir() {
            let log_dir = data_dir.join("raw-editor").join("logs");
            
            // Ensure directory exists
            if let Err(e) = std::fs::create_dir_all(&log_dir) {
                eprintln!("Failed to create log directory: {}", e);
            }

            let file_appender = tracing_appender::rolling::daily(&log_dir, "raw-editor.log");
            let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

            tracing_subscriber::fmt()
                .with_env_filter("raw_editor=info,warn")
                .with_writer(non_blocking)
                .with_ansi(false)
                .init();
            
            Some(guard)
        } else {
            eprintln!("Failed to find data directory for logging!");
            None
        }
    }
}
