// UI module for custom widgets and renderers

pub mod curve_editor; // Point curves (RGB + per channel)
pub mod filmstrip; // Phase 53: Image timeline for Develop tab
pub mod histogram; // Phase 21: Real-time histogram
pub mod icons; // Phase 58: Nerd Font icon constants
pub mod loupe; // Drag surface of the 1:1 detail loupe
pub mod motion; // Springs, easing curves and timelines behind every animation
pub mod text; // The repeated text roles: title, heading, group label, hint
pub mod theme; // Design tokens: colours by role, type, spacing, radius
pub mod preview_renderer; // Phase 32: Canvas-based JPEG preview renderer
pub mod styles;
pub mod widgets;
