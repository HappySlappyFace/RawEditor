//! Custom widgets. Most exist for motion — see `ui::motion` for the timing
//! primitives they share and why they animate themselves.

pub mod button;
pub mod collapse;
pub mod double_click;
pub mod fade_image;
pub mod indicator;
pub mod modal;
pub mod no_operate;
pub mod profiler_graph;
pub mod transition;

/// `use crate::ui::widgets::button;` brings in both the module (for
/// `button::Style`, `button::Status`) and the constructor, like iced's own.
pub use button::button;
