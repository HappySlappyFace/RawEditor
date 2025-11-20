/// Centralized debug logging configuration
/// 
/// Toggle these flags to control which debug logs are printed.
/// This helps avoid terminal overflow when debugging specific systems.

/// GPU rendering and uniform updates
pub const DEBUG_GPU: bool = false;

/// Preview renderer coordinate calculations
pub const DEBUG_PREVIEW: bool = true;

/// Image loading and caching
pub const DEBUG_LOADING: bool = false;

/// Mouse events and interactions
pub const DEBUG_MOUSE: bool = false;

/// General application state
pub const DEBUG_APP: bool = false;

/// Macro for conditional debug printing
#[macro_export]
macro_rules! debug_log {
    ($flag:expr, $($arg:tt)*) => {
        if $flag {
            println!($($arg)*);
        }
    };
}
