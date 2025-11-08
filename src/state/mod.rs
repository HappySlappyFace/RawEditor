/// State management module
/// 
/// This module handles all application state, including:
/// - Database connections and queries (library.rs)
/// - Shared data structures (data.rs)
/// - Edit parameters and non-destructive editing (edit.rs)
/// - Edit history and undo/redo stacks (future)
/// - Background job queue (future)

pub mod library;
pub mod data;
pub mod edit;
