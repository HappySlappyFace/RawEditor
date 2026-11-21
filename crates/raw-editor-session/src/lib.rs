//! RawEditor's editing session: the application logic (catalogue workflow,
//! loading, edits and history, render scheduling, export, background jobs),
//! independent of any UI toolkit. A front end feeds it the user's intents,
//! runs the [`Effects`] it returns, and feeds their results back as events.
//!
//! No UI toolkit may appear here (`no_ui_toolkit_in_the_session` checks the
//! sources): the desktop app's iced layer, or any other front end, sits on
//! top.

#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

// The session's own paths name itself as `raw_editor_session::…`.
extern crate self as raw_editor_session;

mod changed;
mod effects;
pub mod session;

pub use changed::Changed;
pub use effects::{BoxFuture, Effect, Effects};

// The engine's modules at the root, as in the app, so `crate::core::…`,
// `crate::gpu::…` read the same on both sides.
pub use raw_editor_core::{color, core, database, gpu, ml, raw};

#[cfg(test)]
mod guard {
    /// The session is toolkit-free: no source file may name iced (or any
    /// other UI toolkit). A front end's types stay in the front end.
    #[test]
    fn no_ui_toolkit_in_the_session() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root];
        let mut offenders = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read src").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).expect("read source");
                    for (i, line) in text.lines().enumerate() {
                        let code = line.split("//").next().unwrap_or("");
                        if ["iced", "winit", "rfd"].iter().any(|t| code.contains(&format!("{t}::")) || code.contains(&format!("use {t}"))) {
                            offenders.push(format!("{}:{}", path.display(), i + 1));
                        }
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "UI toolkit in the session: {offenders:?}");
    }
}
