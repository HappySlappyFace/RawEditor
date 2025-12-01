use std::path::PathBuf;
use std::sync::Arc;
use iced::{Point, Rectangle};
use crate::gpu;
use crate::raw;
use crate::state::data::Image as ImageData;
use crate::ui::preview_renderer::CropHandle;

/// Application tabs/modules
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppTab {
    Library,  // Browse, import, organize images
    Cull,
    Develop,  // Edit selected image with full preview
}

/// Result of a folder import operation
#[derive(Debug, Clone)]
pub struct ImportResult {
    pub imported_count: usize,
    pub skipped_count: usize,
}

/// Result of thumbnail generation
#[derive(Debug, Clone)]
pub struct ThumbnailResult {
    pub generated_count: usize,
}

/// Result of preview generation
#[derive(Debug, Clone)]
pub struct PreviewResult {
    pub image_id: i64,
    pub preview_path: Result<String, String>,
}

/// Application messages (events)
#[derive(Debug, Clone)]
pub enum Message {
    // ========== Startup Messages (Phase 23) ==========
    /// Database loading completed (async background task)
    /// Phase 23: Only send images Vec, Library created on main thread (not Send)
    DatabaseLoaded(Result<Vec<ImageData>, String>),
    
    /// User clicked the "Import Folder" button
    ImportFolder,
    /// Background import completed with results
    ImportComplete(ImportResult),
    /// Background thumbnail generation completed
    ThumbnailGenerated(ThumbnailResult),
    /// Phase 28: Multi-tier cache processing completed
    /// Result is (image_id, thumb_path, instant_path, working_path) or (image_id, error)
    CacheProcessed(Result<(i64, String, String, String), (i64, String)>),
    /// User selected an image from the grid
    ImageSelected(i64),
    /// Background preview generation completed
    PreviewGenerated(PreviewResult),
    /// User switched to a different tab
    TabChanged(AppTab),
    
    // ========== Edit Parameter Changes ==========
    /// User changed exposure slider
    ExposureChanged(f32),
    /// User changed contrast slider
    ContrastChanged(f32),
    /// User changed highlights slider
    HighlightsChanged(f32),
    /// User changed shadows slider
    ShadowsChanged(f32),
    /// User changed whites slider
    WhitesChanged(f32),
    /// User changed blacks slider
    BlacksChanged(f32),
    /// User changed manual black level offset (channel_index, value)
    BlackOffsetChanged(usize, f32),
    /// User changed black level phase (is_y, value)
    BlackPhaseChanged(bool, u32),
    
    // ========== Color Messages ==========
    /// User changed vibrance slider
    VibranceChanged(f32),
    /// User changed saturation slider
    SaturationChanged(f32),
    /// User changed temperature slider (Phase 18)
    TemperatureChanged(f32),
    /// User changed tint slider (Phase 18)
    TintChanged(f32),
    /// User changed noise reduction slider (Phase 49)
    NoiseReductionChanged(f32),
    /// User changed sharpening slider (Phase 50)
    SharpeningChanged(f32),
    /// User changed sharpen masking slider (Phase 51)
    SharpenMaskingChanged(f32),
    /// User changed rotation slider (Phase 52)
    RotationChanged(f32),
    /// User changed crop rectangle (Phase 66)
    SetCrop([f32; 4]),
    /// User clicked Reset button to clear all edits
    ResetEdits,
    // Phase 73: The Look-Ahead Cache
    PreloadPreview(i64),
    PreviewCached(i64, Result<iced::widget::image::Handle, String>),
    
    // Phase 67: Interactive Crop
    ToggleCropMode,
    CropHandleGrabbed(CropHandle, Rectangle),
    // Phase 63: Copy/Paste Edits
    CopyEdits,
    PasteEdits,
    
    // Phase 65: Undo/Redo
    Undo,
    Redo,
    CommitEdit,
    
    // ========== Phase 54: Settings Clipboard ==========
    /// Copy current edit settings to clipboard (Ctrl/Cmd+C)
    CopySettings,
    /// Paste edit settings from clipboard (Ctrl/Cmd+V)
    PasteSettings,
    
    // ========== Phase 55: Multi-Selection ==========
    /// Modifier keys changed (for Ctrl/Cmd+Click detection)
    ModifiersChanged(iced::keyboard::Modifiers),
    
    // ========== Phase 56: Ratings & Culling ==========
    /// Set rating for selected image(s) (0-5 stars)
    SetRating(u8),
    
    // ========== Phase 59: Rating Filter ==========
    /// Set minimum rating filter (0 = all, 1-5 = show rating or higher)
    SetMinRating(u8),

    // ========== Phase 60: Modern Layout & HUD ==========
    /// Toggle HUD overlay (ISO, Shutter, etc.)
    ToggleInfoHud,

    // ========== Phase 24: Workflow Messages ==========
    /// Toggle Before/After view (Spacebar)
    ToggleBeforeAfter,
    /// Select next image (Right arrow)
    SelectNextImage,
    /// Select previous image (Left arrow)
    SelectPreviousImage,
    
    // ========== Phase 25: Zoom & Pan Messages ==========
    /// User zoomed with mouse wheel (delta, cursor position)
    Zoom(f32, Point),
    /// User panned with mouse drag (delta in screen space)
    Pan(cgmath::Vector2<f32>),
    /// Mouse button pressed - start dragging
    MousePressed,
    /// Mouse button released - stop dragging
    MouseReleased,
    /// Mouse moved - track for panning
    MouseMoved(Point),
    
    // ========== Phase 26: Advanced Zoom Polish ==========
    /// Reset zoom and pan to default (1.0, 0.0)
    ResetView,
    
    // ========== GPU Pipeline Messages ==========
    /// Background RAW data loading completed
    RawDataLoaded(Result<raw::loader::RawDataResult, String>),
    /// GPU pipeline initialization completed
    GpuPipelineReady(Result<Arc<gpu::RenderPipeline>, String>),
    
    // ========== Export Messages (Phase 19) ==========
    /// User clicked Export button
    ExportImage,
    ExportComplete(Result<PathBuf, String>),
    
    // Window Controls
    MinimizeWindow,
    MaximizeWindow,
    CloseWindow,
    DragWindow,
    
    // ========== Histogram Messages (Phase 22) ==========
    /// User toggled histogram on/off
    HistogramToggled(bool),
    
    // ========== Phase 30: Multi-Tier Preview Loading ==========
    /// Background loading of 1280px working preview completed
    WorkingPreviewReady(iced::widget::image::Handle),
}
