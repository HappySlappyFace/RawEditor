pub mod message;
pub mod state;
pub mod update;
pub mod view;
pub mod views;
pub mod handlers;
pub mod loader;

/// Message-level test harness: an editor in memory, driven through `update`.
#[cfg(test)]
pub mod testkit;
#[cfg(test)]
mod tests;

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
