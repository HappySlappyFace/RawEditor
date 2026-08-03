use crate::database::models::Image as ImageData;
use crate::raw;
use crate::ui::preview_renderer::{CropHandle, MaskHandle};
use iced::{Point, Rectangle};
use std::path::PathBuf;

/// Which per-mask adjustment a slider targets (see MaskAdjustmentChanged)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskAdjustment {
    Exposure,
    Contrast,
    Saturation,
    Warmth,
    Highlights,
    Shadows,
    Feather,
    Tint,
    Rotation,
}

/// Application tabs/modules
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppTab {
    Library, // Browse, import, organize images
    Cull,
    Develop, // Edit selected image with full preview
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
#[allow(clippy::large_enum_variant)]
pub enum Message {
    // ========== Startup Messages (Phase 23) ==========
    /// Database loading completed (async background task)
    /// Phase 23: Only send images Vec, Library created on main thread (not Send)
    DatabaseLoaded(Result<Vec<ImageData>, String>),

    /// User clicked the "Import Folder" button
    ImportFolder,
    /// Placeholder for camera import workflow
    ImportFromCamera,
    /// Background import completed with results
    ImportComplete(ImportResult),
    /// Background thumbnail generation completed
    ThumbnailGenerated(ThumbnailResult),
    /// Phase 28: Multi-tier cache processing completed
    /// Result is (image_id, thumb_path, instant_path, working_path) or (image_id, error)
    CacheProcessed(Result<(i64, String, String, String), (i64, String)>),

    /// Background preview generation completed
    PreviewGenerated(PreviewResult),
    /// User switched to a different tab
    TabChanged(AppTab),

    /// Phase 88: Resize thumbnails
    SetThumbnailSize(f32),

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
    /// User changed DCP profile tone-curve strength (0.0..=1.0)
    ProfileCurveChanged(f32),
    /// Toggle the WB eyedropper tool
    ToggleWbPicker,
    /// User clicked the preview with the WB picker active.
    /// Payload: display-space UV (0..1) within the rendered image bounds.
    WbPicked(f32, f32),
    /// User changed saturation slider
    SaturationChanged(f32),
    /// User changed temperature slider (Phase 18)
    TemperatureChanged(f32),
    /// User changed tint slider (Phase 18)
    TintChanged(f32),
    /// User changed noise reduction slider (Phase 49)
    LumaNoiseChanged(f32),
    ColorNoiseChanged(f32),
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

    // ========== Export Messages (Phase 89) ==========
    SetExportFormat(crate::app::state::ExportFormat),
    SetExportQuality(u8),
    ToggleExportResize(bool),
    SetExportWidth(u32),
    SetExportSubfolder(String),
    SetExportBasePath(PathBuf),
    PickExportBasePath,
    OpenExportModal,
    ExportConfirmed,
    ProcessNextExport,
    /// Carries the exported image's OWN saved edit params alongside its RAW
    /// data. They must travel with the job rather than be read from
    /// `editor.current_edit_params`, which is whatever image happens to be open
    /// in Develop — a batch export previously applied that one image's edits to
    /// every image in the queue.
    ExportRawLoaded(
        i64,
        crate::core::types::EditParams,
        Result<std::sync::Arc<raw::loader::RawDataResult>, String>,
    ),
    ExportPipelineReady(
        i64,
        crate::core::types::EditParams,
        Result<
            (
                std::sync::Arc<crate::gpu::shared::SharedContext>,
                std::sync::Arc<crate::gpu::shared::ImageResources>,
            ),
            String,
        >,
    ),
    ExportSaveComplete(i64, Result<PathBuf, String>),

    /// Phase 60: Toggle for HUD overlay (ISO, Shutter, etc.)
    ToggleInfoHud,
    /// Phase 67: Interactive Crop
    ToggleCrop,
    CropHandleGrabbed(CropHandle, Rectangle),

    // ========== Local Adjustment Masks ==========
    /// Toggle mask placement mode (0 = linear, 1 = radial)
    ToggleMaskPlacement(u32),
    /// Select a mask in the list (None = deselect)
    SelectMask(Option<usize>),
    /// Delete a mask by index
    DeleteMask(usize),
    /// Enable/disable a mask by index
    ToggleMaskEnabled(usize, bool),
    /// Invert the selected mask
    ToggleMaskInvert(bool),
    /// Change one adjustment of the selected mask
    MaskAdjustmentChanged(MaskAdjustment, f32),
    /// User clicked the preview while placing a mask.
    /// Payload: full-image display UV (0..1) — crop remap already applied.
    MaskPlacementStarted(f32, f32),
    /// User grabbed a handle of the selected mask on the canvas
    MaskHandleGrabbed(MaskHandle, Rectangle),
    /// Phase 67: Delete Image — opens the confirm modal, or (if every
    /// targeted image is already marked) instantly toggles the mark off.
    DeleteImage,
    /// "Mark for Removal" button in the delete modal.
    MarkForRemovalConfirmed,
    /// "Delete from Disk" button in the delete modal, and the global Enter key.
    DeleteFromDiskConfirmed,
    /// Background hard-delete (trash move + DB row removal) batch finished.
    HardDeleteComplete(crate::app::handlers::delete::HardDeleteOutcome),
    /// Phase 65: Undo/Redo History
    Undo,
    Redo,
    /// Phase 65: Commit current edit state to history
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
    /// Phase 83: Set Pick/Reject flag (-1, 0, 1)
    SetFlag(i8),
    /// Phase 83: Toggle auto-advance feature
    ToggleAutoAdvance,

    // Phase 84: Generic Modal System
    OpenModal(crate::app::state::Modal),
    CloseModal,
    ModalNoOp, // Swallows clicks
    Escape,    // Global Escape key

    // Phase 85: Preferences
    SetCacheCapacity(f32),
    SetRawPreloadBudget(f32),
    SetPreviewPreloadBehind(f32),
    SetPreviewPreloadAhead(f32),
    SetRawPreloadBehind(f32),
    SetRawPreloadAhead(f32),
    ResetPreferences,

    /// User selected an image from the grid
    ImageSelected(i64),
    // ========== Phase 59: Filtering ==========
    /// Set minimum rating filter (0 = all, 1-5 = show rating or higher)
    SetMinRating(u8),
    /// Filter library by folder path prefix (None = all folders)
    SetLibraryFolderFilter(Option<String>),

    // ========== Phase 24: Workflow Messages ==========
    /// Toggle Before/After view (Spacebar)
    ToggleBeforeAfter,

    /// Select next image (Right arrow)
    SelectNextImage,
    /// Select previous image (Left arrow)
    SelectPreviousImage,

    /// Phase 73: Preload preview for adjacent image
    PreloadPreview(i64),
    /// Phase 73: Preview loaded from cache/disk
    /// Phase 76: Eager Background Decoding (width, height, pixels)
    PreviewCached(i64, Result<(u32, u32, std::sync::Arc<[u8]>), String>),
    /// Phase 77: Working Preview Ready
    WorkingPreviewReady(
        i64,
        iced::widget::image::Handle,
        Option<std::sync::Arc<[u8]>>,
        /// Phase 115: Actual pixel dimensions of the byte buffer
        (u32, u32),
    ),

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
    RawDataLoaded(i64, Result<raw::loader::RawDataResult, String>),
    /// Background RAW preload for adjacent images completed
    RawPreloaded(i64, Result<std::sync::Arc<raw::loader::RawDataResult>, String>),
    /// GPU pipeline initialization completed
    // Phase 95: GPU ImageResources loaded (wrapped in Arc for Clone)
    ImageResourcesReady(
        i64,
        Result<
            (
                std::sync::Arc<crate::gpu::shared::SharedContext>,
                std::sync::Arc<crate::gpu::shared::ImageResources>,
            ),
            String,
        >,
    ),

    // ========== Export Messages (Phase 19) ==========
    // Legacy export messages removed

    // Window Controls
    MinimizeWindow,
    MaximizeWindow,
    CloseWindow,
    DragWindow,

    // ========== Histogram Messages (Phase 22) ==========
    /// Phase 116: Interaction & Coordinate System Fixes
    ViewportResized(f32, f32, f32),

    /// Render task failed (GPU error / shader panic) — releases the throttle lock
    RenderFailed,
    /// A develop render completed: display window bytes, their dimensions, the
    /// whole-image histogram, and timings. Both GPU passes land together, so
    /// this is the only render-completion message.
    RenderFinished(
        std::sync::Arc<[u8]>,
        (u32, u32),
        /// The window these bytes cover. Travels with them so the viewport
        /// never draws a buffer against a window it wasn't rendered for.
        [f32; 4],
        crate::core::histogram::HistogramData,
        f32,
        f32,
        f32,
    ),
    /// Phase 104: Toggle Profiler Overlay
    ToggleProfiler,

    /// Phase 22: User toggled histogram on/off
    HistogramToggled(bool),

    // ========== Scroll boost + filmstrip momentum ==========
    /// Raw wheel event from the global subscription, routed by cursor
    /// position to the library grid (the filmstrip has its own dedicated
    /// interceptor — see FilmstripWheel — so it isn't handled here).
    WheelScrolled(iced::mouse::ScrollDelta),
    /// Window-space cursor position, tracked globally so wheel routing knows
    /// which region the cursor is over (WheelScrolled itself carries no position).
    GlobalCursorMoved(iced::Point),
    /// Window resize, needed for the same region routing.
    WindowResized(iced::Size),
    /// One frame tick of the filmstrip's kinetic (momentum) scroll.
    KineticTick(std::time::Instant),
    /// Wheel event captured directly by the filmstrip's own overlay
    /// (src/ui/filmstrip.rs), which sits above its scrollable in a `stack!`
    /// specifically so iced's native ~60px/line jump never fires there —
    /// all filmstrip scrolling comes from our own physics instead.
    FilmstripWheel(iced::mouse::ScrollDelta),
}
