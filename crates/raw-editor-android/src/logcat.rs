//! `tracing` output to logcat (tag `RawEditor`): stderr goes nowhere on
//! Android.

use std::ffi::CString;
use std::io::Write;

struct Logcat(Vec<u8>);

impl Write for Logcat {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for Logcat {
    /// One event per writer, written whole.
    fn drop(&mut self) {
        let text = String::from_utf8_lossy(&self.0);
        let Ok(text) = CString::new(text.trim_end().replace('\0', " ")) else { return };
        // SAFETY: two valid NUL-terminated strings.
        unsafe {
            ndk_sys::__android_log_write(ndk_sys::android_LogPriority::ANDROID_LOG_INFO.0 as i32, c"RawEditor".as_ptr(), text.as_ptr());
        }
    }
}

pub fn init() {
    // A panic's message goes to stderr, which nobody reads here.
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        default(info);
    }));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"))
        .with_ansi(false)
        .without_time()
        .with_writer(|| Logcat(Vec::new()))
        .try_init();
}
