//! RawEditor's editing session: the application logic (catalogue workflow,
//! loading, edits and history, render scheduling, export, background jobs),
//! independent of any UI toolkit. A front end feeds it the user's intents,
//! runs the [`Effects`] it returns, and feeds their results back as events.
//!
//! It is being moved here from the desktop app area by area; for now this
//! crate holds the plumbing.

mod changed;
mod effects;

pub use changed::Changed;
pub use effects::{BoxFuture, Effect, Effects};
