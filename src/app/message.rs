use crate::database::models::Image as ImageData;
pub use crate::session::{
    CopyCategory, CropHandle, DefringeParam, GtParam, LensParam, LensToggle, MaskAdjustment, MaskHandle,
    StrokeEvent,
};
use iced::{Point, Rectangle};
use std::path::PathBuf;

/// Brush tool settings (see `BrushSettingChanged`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrushSetting {
    Size,
    Feather,
    Flow,
}

/// The Develop sidebar's foldable sections, top to bottom (UI state only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SidebarSection {
    Light,
    ToneCurve,
    ToneMapping,
    Colour,
    Presence,
    /// Noise reduction, AI Denoise and sharpening.
    Detail,
    /// The 1:1 detail loupe inside Detail. It renders only while both are
    /// open (`handlers::loupe`).
    DetailPreview,
    Geometry,
    /// Lens profile corrections and defringe.
    Lens,
    Masks,
}

impl SidebarSection {
    pub const ALL: [SidebarSection; 10] = [
        Self::Light,
        Self::ToneCurve,
        Self::ToneMapping,
        Self::Colour,
        Self::Presence,
        Self::Detail,
        Self::DetailPreview,
        Self::Geometry,
        Self::Lens,
        Self::Masks,
    ];

    /// Folded on first launch: the set-once tools, and the two tallest
    /// blocks (the curve and the loupe).
    pub const FOLDED_BY_DEFAULT: [SidebarSection; 4] =
        [Self::ToneCurve, Self::DetailPreview, Self::Geometry, Self::Lens];

    /// The stable name saved in settings (`AppSettings::develop_folded`).
    /// Never rename one: a saved name that no longer matches is dropped.
    pub fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::ToneCurve => "tone_curve",
            Self::ToneMapping => "tone_mapping",
            Self::Colour => "colour",
            Self::Presence => "presence",
            Self::Detail => "detail",
            Self::DetailPreview => "detail_preview",
            Self::Geometry => "geometry",
            Self::Lens => "lens",
            Self::Masks => "masks",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }
}

pub use crate::session::AppTab;

/// Application messages (events)
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Message {
    /// A piece of work the session asked for has finished (`crate::session`).
    Session(crate::session::SessionEvent),
    // ========== Startup Messages (Phase 23) ==========
    /// Database loading completed (async background task)
    /// Phase 23: Only send images Vec, Library created on main thread (not Send)
    DatabaseLoaded(Result<Vec<ImageData>, String>),

    /// User clicked the "Import Folder" button
    ImportFolder,
    /// Placeholder for camera import workflow
    ImportFromCamera,

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
    /// A section header's reset: that section's edits back to default.
    ResetSection(SidebarSection),
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
    SetExportFormat(crate::session::ExportFormat),
    SetExportColorSpace(crate::core::output_space::OutputSpace),
    SetExportQuality(u8),
    ToggleExportResize(bool),
    /// JPEG only: add an HDR gain map (Ultra HDR).
    ToggleExportUltraHdr(bool),
    SetExportWidth(String),
    SetExportSubfolder(String),
    SetExportBasePath(PathBuf),
    PickExportBasePath,
    OpenExportModal,
    ExportConfirmed,
    /// Stop after the image currently rendering finishes.
    CancelExport,

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
    /// A double-click reset on a slider: like `SliderSet` with `commit`, but
    /// folded into the undo step the double click's first press made.
    SliderReset { change: Box<Message> },

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
    SetAppearance(crate::ui::theme::Appearance),
    ModalNoOp, // Swallows clicks
    /// An arrow key came up: ends a held-key follow scroll
    /// (`handlers::scroll::release`).
    NavigationKeyReleased,
    /// The window lost focus: the key-up of anything held (Space's peek, an
    /// arrow key's follow scroll) goes to another window, so end them here.
    WindowUnfocused,
    /// A shortcut that must not repeat while its key is held: `update`
    /// unwraps it on the first press only (`RawEditor::key_is_fresh`).
    ShortcutKey { key: String, message: Box<Message> },
    /// A character key came up: the next press of it counts again.
    ShortcutKeyUp(String),
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
    /// Library: a thumbnail was double-clicked — open it in Develop.
    OpenInDevelop(i64),
    /// A file or folder was dropped onto the window (one per item dropped).
    FileDropped(PathBuf),
    /// A dropped item's folder, scanned off the UI thread: the folder and the
    /// RAW files under it. `None` when the item was neither a folder nor a RAW.
    DropScanned { dropped: PathBuf, scan: Option<(PathBuf, Vec<PathBuf>)> },
    /// Import every folder in the dropped-import dialog.
    DropImportConfirmed,
    // ========== Updates (handlers::updates) ==========
    /// Help › Check for Updates, or Preferences' Check Now: check and show
    /// the result.
    CheckForUpdates,
    /// The first-run prompt's checkbox.
    UpdatePromptToggled(bool),
    /// The first-run prompt's Continue: save the answer.
    UpdatePromptConfirmed,
    /// Preferences › General › Check for updates at startup.
    ToggleUpdateCheck,
    /// Open the available release's page.
    OpenUpdatePage,
    /// Open the project page in the default browser.
    OpenRepository,
    // ========== AI masks (handlers::ai_mask) ==========
    /// `+ AI Select`: new AI mask, select into it.
    AiSelectStart,
    /// Reopen an AI mask's selection.
    AiSelectRefine(usize),
    /// A click while AI select is open, full-image display UV.
    AiPrompt { u: f32, v: f32, positive: bool },
    /// A box dragged while AI select is open, full-image display UV corners.
    AiRect(f32, f32, f32, f32),
    AiSelectDone,
    AiSelectCancel,

    // ========== AI models (handlers::models) ==========
    ModelDownload(crate::ml::models::ModelId),
    ModelDownloadCancel,
    ModelRemove(crate::ml::models::ModelId),
    /// Redraw while a download or an AI job reports progress.
    AiTick,

    // ========== AI denoise (handlers::ai_denoise) ==========
    OpenAiDenoise,
    AiDenoiseAmount(f32),
    /// Holding "Before" (true) or letting go.
    AiDenoiseBefore(bool),
    AiDenoisePan(f32, f32),
    AiDenoiseApply,
    AiDenoiseRemove,
    AiDenoiseCancel,

    /// Dragging inside the loupe, by this many logical pixels.
    LoupePan(f32, f32),
    /// Arm or disarm "click the image to place the loupe".
    ToggleLoupeTarget,
    /// The image was clicked while placing the loupe (display-space UV).
    LoupeTargetPicked(f32, f32),
    /// Ask lensfun what database it currently publishes.
    LensfunCheck,
    /// Download (or update) the lensfun database.
    LensfunDownload,
    /// Re-read the lens folders — after adding XML files by hand.
    LensfunRescan,
    /// Develop's profile dropdown: this image's camera profile.
    CameraProfileSelected(crate::core::camera_profiles::ProfileChoice),
    /// Preferences: a camera's default profile — a file name, `"builtin"`,
    /// or `None` for automatic.
    SetCameraProfileDefault(String, Option<String>),
    /// Download RawTherapee's profile for the open image's camera.
    DownloadCameraProfile,
    /// Pick `.dcp` files to import.
    ImportCameraProfiles,
    CameraProfilesPicked(Option<Vec<std::path::PathBuf>>),
    /// Refresh every downloaded profile from RawTherapee.
    UpdateCameraProfiles,
    /// Re-read the profile folders — after adding files by hand.
    RescanCameraProfiles,
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
    /// Selection made by the app, not a click: ignores the modifier keys.
    ImageMoved(i64),
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
