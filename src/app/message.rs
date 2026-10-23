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
    Hue,
    LumaLo,
    LumaHi,
    LumaSmooth,
    ColorTolerance,
}

/// Brush tool settings (see `BrushSettingChanged`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrushSetting {
    Size,
    Feather,
    Flow,
}

/// One phase of a brush stroke on the preview, in full-image display UV.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StrokeEvent {
    Begin(f32, f32),
    Move(f32, f32),
    End,
}

/// Which GT (Uchimura) shape parameter a slider targets. Mirrors the
/// `MaskAdjustment` pattern: one message for a family of sliders that all
/// land in the same handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GtParam {
    Contrast,
    LinearStart,
    LinearLength,
    BlackTightness,
}

/// Which lens-correction slider changed (see `LensParamChanged`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensParam {
    ProfileDistortion,
    ProfileVignetting,
    Distortion,
    Vignetting,
    CaRed,
    CaBlue,
}

/// Which lens-correction checkbox changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensToggle {
    Profile,
    ProfileCa,
    Constrain,
}

/// Which defringe slider changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefringeParam {
    PurpleAmount,
    PurpleHue,
    GreenAmount,
    GreenHue,
}

/// Collapsible Develop sidebar sections (UI state only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SidebarSection {
    /// The 1:1 detail loupe above Noise Reduction.
    DetailPreview,
    ToneCurve,
    Lens,
    Defringe,
}

/// Which group of adjustments a Copy Settings checkbox targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyCategory {
    Tone,
    Color,
    WhiteBalance,
    Presence,
    Noise,
    Detail,
    Geometry,
    Profile,
    Masks,
    Lens,
    BlackLevels,
}

/// Application tabs/modules
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppTab {
    Library, // Browse, import, organize images
    Cull,
    Develop, // Edit selected image with full preview
    Export,  // Batch export queue and settings
}

/// Result of a folder import operation
#[derive(Debug, Clone)]
pub struct ImportResult {
    pub imported_count: usize,
    pub skipped_count: usize,
}

/// Everything a completed develop render hands back to the UI thread.
#[derive(Debug, Clone)]
pub struct RenderOutput {
    /// Image the render was made from. A render still in flight when the
    /// user navigates must not land on the next image's viewport.
    pub image_id: i64,
    /// The on-screen window's pixels (RGBA), `dims` in size.
    pub display: std::sync::Arc<[u8]>,
    pub dims: (u32, u32),
    /// The window `display` covers. Travels with the pixels so the viewport
    /// never draws a buffer against a window it wasn't rendered for.
    pub view_rect: [f32; 4],
    /// Small whole-image render (RGBA), `overview_dims` in size: histogram
    /// source and edited-proxy candidate.
    pub overview: std::sync::Arc<[u8]>,
    pub overview_dims: (u32, u32),
    pub histogram: crate::core::histogram::HistogramData,
    /// The params rendered, when they are the user's real edit — `None` for
    /// the before/after peek and the crop tool's full-sensor visualisation,
    /// neither of which may stand in for the edited image.
    pub edit: Option<crate::core::types::EditParams>,
    pub upload_ms: f32,
    pub render_ms: f32,
    pub update_ms: f32,
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
    /// Background import finished (or failed — e.g. the catalogue could not
    /// be opened).
    ImportComplete(Result<ImportResult, String>),
    /// Phase 28: Multi-tier cache processing completed
    /// Result is (image_id, thumb_path, instant_path, working_path) or (image_id, error)
    CacheProcessed(Result<(i64, String, String, String), (i64, String)>),

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
    /// A point curve was edited in the curve editor: (channel, new curve,
    /// commit an undo step now). Drags send `false` per move and commit on
    /// release via `CommitEdit`; discrete edits (delete) send `true`.
    CurveChanged(crate::core::curves::CurveChannel, crate::core::curves::Curve, bool),
    /// Show another channel's curve in the editor.
    SetCurveChannel(crate::core::curves::CurveChannel),
    /// Reset the curve of the channel being shown.
    ResetCurve,
    /// A lens-correction slider moved.
    LensParamChanged(LensParam, f32),
    /// A lens-correction checkbox toggled.
    LensToggled(LensToggle, bool),
    /// A defringe slider moved.
    DefringeChanged(DefringeParam, f32),
    /// Expand/collapse a Develop sidebar section.
    ToggleSection(SidebarSection),
    /// Select the tone mapping operator (see `core::tonemap`).
    ToneMapperChanged(crate::core::tonemap::ToneMapper),
    /// Adjust one of the GT curve's shape parameters.
    GtParamChanged(GtParam, f32),
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
    /// Presence sliders (see `gpu::local_contrast`, `core::haze`)
    TextureChanged(f32),
    ClarityChanged(f32),
    DehazeChanged(f32),
    /// User changed noise reduction slider (Phase 49)
    LumaNoiseChanged(f32),
    /// Luminance NR detail retention
    LumaDetailChanged(f32),
    ColorNoiseChanged(f32),
    /// User changed sharpening slider (Phase 50)
    SharpeningChanged(f32),
    /// User changed sharpen masking slider (Phase 51)
    SharpenMaskingChanged(f32),
    /// User changed rotation slider (Phase 52)
    RotationChanged(f32),
    /// ⟲ / ⟳: a quarter turn of the frame, crop and masks with it.
    TurnImage { clockwise: bool },
    /// Arm/disarm the level tool (drag along a horizon to straighten).
    ToggleLevelTool,
    /// A level line was drawn on the preview: its direction in screen
    /// pixels, start to end.
    LevelLineDrawn(f32, f32),
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
    /// Stop after the image currently rendering finishes.
    CancelExport,
    ProcessNextExport,
    /// Carries the exported image's OWN saved edit params alongside its RAW
    /// data. They must travel with the job rather than be read from
    /// `editor.current_edit_params`, which is whatever image happens to be open
    /// in Develop — a batch export previously applied that one image's edits to
    /// every image in the queue.
    ///
    /// Boxed, like every `EditParams` a message carries: it is ~20 KB (the
    /// brush arrays), and an enum is as big as its largest variant — so an
    /// inline one made EVERY message that size. Debug builds give each
    /// temporary its own stack slot, and the Develop view's ~50 sliders
    /// overflowed the stack (see `message_stays_small`).
    ExportRawLoaded(
        i64,
        Box<crate::core::types::EditParams>,
        Result<std::sync::Arc<raw::loader::RawDataResult>, String>,
    ),
    ExportPipelineReady(
        i64,
        Box<crate::core::types::EditParams>,
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
    /// Arm/disarm painting into the selected mask.
    ToggleBrushArmed,
    /// Paint (false) or erase (true).
    SetBrushErase(bool),
    BrushSettingChanged(BrushSetting, f32),
    /// Painting on the preview.
    BrushStroke(StrokeEvent),
    /// Remove every stroke of the selected mask.
    ClearMaskBrush,
    /// Show the selected mask's coverage as a red overlay.
    ToggleMaskOverlay(bool),
    /// Luminance-range refinement on/off for the selected mask.
    ToggleMaskLumaRange(bool),
    /// Colour-range refinement on/off for the selected mask. Turning it on
    /// arms the eyedropper.
    ToggleMaskColorRange(bool),
    /// Re-arm the colour-range eyedropper.
    PickMaskColor,
    /// Eyedropper click: display-space UV within the rendered image, like
    /// `WbPicked`.
    MaskColorPicked(f32, f32),
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
    /// A click on a Develop slider's value: type a number into it instead.
    /// `key` identifies the slider (`views::develop::slider_key`), `text` is
    /// the value as shown.
    SliderEntryStart { key: String, text: String },
    SliderEntryChanged(String),
    /// Leave the typed value without applying it (Escape, or the user moved
    /// on to something else).
    SliderEntryCancel,
    /// Set a slider in one step — a typed value (Enter) or a double-click
    /// reset. `change` is the slider's own change message; `commit` makes it
    /// an undo step as a drag's release would (false for tool settings like
    /// brush size, which are not edits).
    SliderSet { change: Box<Message>, commit: bool },

    // ========== Phase 54: Settings Clipboard ==========
    /// Open the category picker (Ctrl/Cmd+C)
    CopySettings,
    /// Copy every category immediately, no modal (Ctrl/Cmd+Shift+C)
    CopyAllSettings,
    /// Confirm the category picker
    CopySettingsConfirmed,
    /// Toggle one category in the picker
    ToggleCopyCategory(CopyCategory, bool),
    /// Picker's All / None buttons
    SetAllCopyCategories(bool),
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
    /// A closed modal's exit animation is over; unmount it (token-matched,
    /// see `state::ModalExit`).
    ModalExitFinished(u64),
    /// Preferences › Reduce motion.
    ToggleReduceMotion,
    ModalNoOp, // Swallows clicks
    /// An arrow key came up: ends a held-key follow scroll
    /// (`handlers::scroll::release`).
    NavigationKeyReleased,
    /// A title-bar dropdown entry was chosen: closes the dropdown, then runs
    /// the entry's own message. See `views::layout::view_title_bar`.
    MenuItem(Box<Message>),
    Escape,    // Global Escape key
    /// Ctrl/Cmd+A — select every image the current tab displays.
    SelectAll,
    /// Ask to forget a folder (opens the confirmation).
    RemoveFolderRequested(String),
    /// Confirm forgetting the pending folder.
    RemoveFolderConfirmed,
    /// Result of the async folder picker; `None` = cancelled.
    ImportFolderPicked(Option<PathBuf>),
    /// Help > Check for Updates — opens the releases page.
    CheckForUpdates,
    /// Open the project page in the default browser.
    OpenRepository,
    /// The detail loupe for an image was prepared (or could not be).
    LoupeReady(i64, Result<std::sync::Arc<crate::gpu::loupe::Loupe>, String>),
    /// A detail-loupe render finished.
    LoupeRendered(i64, Result<crate::gpu::loupe::LoupeFrame, String>),
    /// Dragging inside the loupe, by this many logical pixels.
    LoupePan(f32, f32),
    /// Arm or disarm "click the image to place the loupe".
    ToggleLoupeTarget,
    /// The image was clicked while placing the loupe (display-space UV).
    LoupeTargetPicked(f32, f32),
    /// A noise-reduction drag paused: let the main view catch up, if this is
    /// still the latest tick's timer.
    NoiseSettled(u64),
    /// Ask lensfun what database it currently publishes.
    LensfunCheck,
    LensfunChecked(Result<crate::core::lensfun::Remote, String>),
    /// Download (or update) the lensfun database.
    LensfunDownload,
    LensfunInstalled(Result<crate::core::lensfun::Installed, String>),
    /// Re-read the lens folders — after adding XML files by hand.
    LensfunRescan,
    /// The lens database was (re)loaded in the background. `true` when it
    /// may have changed, so the open image's profile lookup is stale.
    LensDbLoaded(bool),
    /// Develop's profile dropdown: this image's camera profile.
    CameraProfileSelected(crate::core::camera_profiles::ProfileChoice),
    /// Preferences: a camera's default profile — a file name, `"builtin"`,
    /// or `None` for automatic.
    SetCameraProfileDefault(String, Option<String>),
    /// Download RawTherapee's profile for the open image's camera.
    DownloadCameraProfile,
    CameraProfileDownloaded(Result<Option<String>, String>),
    /// Pick `.dcp` files to import.
    ImportCameraProfiles,
    CameraProfilesPicked(Option<Vec<std::path::PathBuf>>),
    /// Imported names, and the failures.
    CameraProfilesImported(Vec<String>, Vec<String>),
    /// Refresh every downloaded profile from RawTherapee.
    UpdateCameraProfiles,
    CameraProfilesUpdated(Result<crate::core::camera_profiles::UpdateReport, String>),
    /// Re-read the profile folders — after adding files by hand.
    RescanCameraProfiles,
    /// The profile folders were (re)scanned; `true` when they may have
    /// changed, so the open image's profile may resolve differently.
    CameraProfilesScanned(bool),
    /// Open a URL in the default browser (Preferences › Licenses).
    OpenLink(&'static str),
    /// Preferences side menu: show this page.
    SetPreferencesSection(crate::app::state::PreferencesSection),
    /// Preferences › Folders: reveal one of RawEditor's folders in the
    /// system file manager.
    OpenAppFolder(crate::core::paths::AppFolder),
    /// Enter pressed — routed to whichever modal is open.
    ///
    /// `subscription()` can only emit ONE message per key event, so Enter
    /// cannot be bound separately per modal. This dispatches on
    /// `active_modal` instead of each modal claiming the key.
    ModalConfirm,

    // Phase 85: Preferences
    SetCacheCapacity(f32),
    SetRawPreloadBudget(f32),
    SetPreviewPreloadBehind(f32),
    SetPreviewPreloadAhead(f32),
    SetRawPreloadBehind(f32),
    SetRawPreloadAhead(f32),
    SetMinFreeRam(f32),
    /// A Preferences slider was released: persist the settings once.
    SavePreferences,
    ResetPreferences,

    /// User selected an image from the grid
    ImageSelected(i64),
    // ========== Phase 59: Filtering ==========
    /// Set minimum rating filter (0 = all, 1-5 = show rating or higher)
    SetMinRating(u8),
    /// Filter library by folder path prefix (None = all folders)
    SetLibraryFolderFilter(Option<String>),

    // ========== Phase 24: Workflow Messages ==========
    /// Hold Space to peek at the unedited original; release to return.
    /// `true` on key down, `false` on key up (and on focus loss).
    SetBeforePeek(bool),

    /// Select next image (Right arrow)
    SelectNextImage,
    /// Select previous image (Left arrow)
    SelectPreviousImage,

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
    /// The full-resolution rebuild (`loading::trigger_full_res_upgrade`)
    /// finished. Separate from `ImageResourcesReady` so only its own result
    /// releases the one-at-a-time upgrade guard, and so a failed upgrade
    /// keeps the working preview instead of failing the editor.
    FullResReady(
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
    /// The develop viewport's logical size changed: (width, height).
    ViewportResized(f32, f32),

    /// A develop render completed. Boxed: the histogram alone is 3 KB.
    RenderFinished(Box<RenderOutput>),
    /// An edited proxy finished saving to disk: (image, proxy generation,
    /// path or error). See `handlers::proxy`.
    EditedProxySaved(i64, u64, Result<String, String>),
    /// A saved edited proxy was read back into memory: (image, pixels).
    EditedProxyLoaded(i64, Result<(u32, u32, std::sync::Arc<[u8]>), String>),
    /// Phase 104: Toggle Profiler Overlay
    ToggleProfiler,

    /// Phase 22: User toggled histogram on/off
    HistogramToggled(bool),

    // ========== Scroll boost + filmstrip momentum ==========
    /// Raw wheel event from the global subscription, applied to the library
    /// grid while the cursor is over it (the filmstrip has its own dedicated
    /// interceptor — see FilmstripWheel — so it isn't handled here).
    WheelScrolled(iced::mouse::ScrollDelta),
    /// Cursor entered (`true`) or left the library grid.
    GridHovered(bool),
    /// The library grid scrolled or resized: (vertical offset, viewport
    /// width, viewport height) in px.
    GridScrolled(f32, f32, f32),
    /// The filmstrip scrolled: (horizontal offset, visible width) in px.
    FilmstripScrolled(f32, f32),
    /// Window resize — the moment the scale factor may have changed too
    /// (dragging the window to a monitor with a different DPI).
    WindowResized(iced::Size),
    /// The window's DPI scale factor, as reported by the windowing system.
    ScaleFactorChanged(f32),
    /// One frame tick of the filmstrip's kinetic (momentum) scroll.
    KineticTick(std::time::Instant),
    /// Wheel event captured directly by the filmstrip's own overlay
    /// (src/ui/filmstrip.rs), which sits above its scrollable in a `stack!`
    /// specifically so iced's native ~60px/line jump never fires there —
    /// all filmstrip scrolling comes from our own physics instead.
    FilmstripWheel(iced::mouse::ScrollDelta),
}
