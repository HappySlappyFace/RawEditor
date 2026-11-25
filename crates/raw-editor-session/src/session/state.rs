//! Plain types and limits the session shares with its front end: which
//! tab is open, how far the open image has loaded, the mask tools, and the
//! bounds of the settings the session enforces.

use std::sync::Arc;

/// Application tabs/modules
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppTab {
    Library, // Browse, import, organize images
    Cull,
    Develop, // Edit selected image with full preview
    Export,  // Batch export queue and settings
}

/// Phase 95: Simplified editor status (pipeline is now in separate fields)
#[derive(Clone, Debug, PartialEq)]
pub enum EditorReadiness {
    /// No image selected
    NoSelection,
    /// Loading RAW data
    Loading(i64),
    /// Image loaded and ready
    Ready(i64),
    /// Failed to load
    Failed(i64, String),
}

/// Brush tool settings (UI state, remembered across masks).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushSettings {
    /// Radius as a fraction of the image height.
    pub size: f32,
    pub feather: f32,
    pub flow: f32,
    /// Erase instead of paint (Alt inverts it while held).
    pub erase: bool,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self { size: 0.05, feather: 0.6, flow: 1.0, erase: false }
    }
}

/// Local adjustment mask tool mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskTool {
    Inactive,
    /// Next click on the preview places a linear gradient
    PlacingLinear,
    /// Next click on the preview places a radial gradient
    PlacingRadial,
}

/// `(kelvin, tone, interpolation, the profile it came from)`: the memo is
/// valid only for the same profile (pointer-compared; kept alive here).
pub type DcpMemo = (
    f32,
    crate::core::tonemap::ToneSettings,
    Arc<crate::raw::dcp::InterpolatedProfile>,
    Arc<crate::raw::dcp::DcpProfile>,
);

/// Shape assumed for the viewport before any preview or render exists.
pub const PLACEHOLDER_DIMS: (u32, u32) = (1280, 853);

/// How soon after a slider's commit a double-click reset still counts as
/// the second half of the same gesture.
pub const SLIDER_AMEND_WINDOW: std::time::Duration = std::time::Duration::from_millis(700);

/// Undo steps kept per image.
pub const HISTORY_LIMIT: usize = 100;
/// Pixel size of the `crop` sub-rect (normalised `[x, y, w, h]`) of a `w`×`h`
/// image, never collapsing to zero — `fitted_size` divides by it.
pub fn crop_dims(w: u32, h: u32, crop: [f32; 4]) -> (u32, u32) {
    (
        ((w as f32 * crop[2]).round() as u32).max(1),
        ((h as f32 * crop[3]).round() as u32).max(1),
    )
}

/// True when `path` lies inside `folder` or any of its subfolders.
///
/// Component-wise, not a string prefix: `/photos/2024` must not match
/// `/photos/2024-old/a.NEF`, which a bare `str::starts_with` happily did —
/// clicking one folder in the sidebar also showed its similarly-named siblings.
pub fn path_is_within(path: &str, folder: &str) -> bool {
    std::path::Path::new(path).starts_with(folder)
}

pub const RAW_CACHE_ENTRY_CAPACITY: usize = 4096;
pub const PREVIEW_PRELOAD_BEHIND_MAX: usize = 30;
pub const PREVIEW_PRELOAD_AHEAD_MAX: usize = 100;
pub const RAW_PRELOAD_BEHIND_MAX: usize = 10;
pub const RAW_PRELOAD_AHEAD_MAX: usize = 20;
/// Upper bound for the free-RAM headroom slider. Above this the setting would
/// pause exports permanently on any ordinary machine.
pub const MIN_FREE_RAM_MB_MAX: u32 = 16384;
