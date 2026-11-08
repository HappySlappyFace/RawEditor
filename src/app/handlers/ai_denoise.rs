//! AI denoise (PMRID, `ml::denoise`): a modal with a 1:1 preview of a small
//! area, an Amount slider that updates it live, and Apply for the whole image.
//!
//! The preview is the detail loupe's machinery (`gpu::loupe`) with its mosaic
//! swapped: PMRID runs on just the sensor crop the window needs (~0.1–0.3 s,
//! again only when the area moves off that crop), and Amount only re-blends
//! the crop and re-renders it — a few milliseconds — so the slider is live.
//! Everything after the mosaic is the real pipeline with the user's current
//! edits, NR included, so the preview is what Apply will give.
//!
//! Apply denoises the whole mosaic once (seconds, with progress, cancellable)
//! and caches the result (`ml::denoise::save_cache`); `EditParams::ai_denoise`
//! then blends it in wherever a mosaic is uploaded. A missing cache (cleared,
//! another machine) is regenerated in the background, and by export itself.

use std::sync::Arc;

use iced::Task;

use crate::app::message::Message;
use crate::app::state::{Modal, RawEditor};
use crate::gpu::loupe::{Loupe, LoupeFrame, MosaicOverride, Region};
use crate::ml::models::ModelId;
use crate::ml::Progress;

/// The preview's side, logical pixels (×`scale_factor` for 1:1).
pub const PREVIEW_SIZE: f32 = 480.0;
/// Sensor pixels around the window that PMRID also gets: context for the
/// network at the window's edges, and room to pan before a re-run.
const CROP_SLACK: u32 = 160;

/// What the modal holds for its image.
pub struct Engine {
    loupe: Loupe,
    /// Unit-range noise model of the whole mosaic (a crop cannot measure it).
    model: crate::core::noise::NoiseModel,
    layout: crate::ml::denoise::MosaicLayout,
}

#[derive(Clone)]
pub struct EngineHandle(Arc<Engine>);

impl std::fmt::Debug for EngineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AiDenoiseEngine")
    }
}

/// One denoised crop: the RAW's samples and PMRID's, over `region`.
pub struct AiCrop {
    region: Region,
    original: Vec<u16>,
    denoised: Vec<u16>,
    id: u64,
}

#[derive(Clone)]
pub struct CropHandle(Arc<AiCrop>);

impl std::fmt::Debug for CropHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AiCrop({:?})", self.0.region)
    }
}

/// The preview frame on screen.
pub struct Frame {
    pub handle: iced::widget::image::Handle,
    pub window: [f32; 4],
    pub size: u32,
}

#[derive(Default)]
pub struct AiDenoiseState {
    pub image_id: Option<i64>,
    /// 0..=1, the slider.
    pub amount: f32,
    /// Preview centre, view UV.
    pub center: [f32; 2],
    /// Holding "Before": show the original.
    pub before: bool,
    pub frame: Option<Frame>,
    engine: Option<EngineHandle>,
    crop: Option<Arc<AiCrop>>,
    /// PMRID running on a crop (and whether the area moved meanwhile).
    crop_running: bool,
    crop_pending: bool,
    render_in_flight: bool,
    render_pending: bool,
    /// The whole-image run behind Apply.
    pub job: Option<Arc<Progress>>,
    /// A background regeneration of a missing cache: `(image, progress)`.
    regen: Option<(i64, Arc<Progress>)>,
    pub error: Option<String>,
}

impl std::fmt::Debug for AiDenoiseState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiDenoiseState").field("image_id", &self.image_id).field("amount", &self.amount).finish_non_exhaustive()
    }
}

impl AiDenoiseState {
    /// A job whose progress the UI shows is running.
    pub fn busy(&self) -> bool {
        self.job.is_some() || self.regen.is_some()
    }

    pub fn preparing(&self) -> bool {
        self.engine.is_none() && self.error.is_none()
    }

    pub fn engine_ready(&self) -> bool {
        self.engine.is_some()
    }

    /// PMRID is working on the preview area.
    pub fn thinking(&self) -> bool {
        self.crop_running
    }
}

fn physical_size(editor: &RawEditor) -> u32 {
    (PREVIEW_SIZE * editor.scale_factor).round().clamp(64.0, 2048.0) as u32
}

/// `AI Denoise…`: open the modal on the selected image.
pub fn handle_open(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::None {
        return Task::none();
    }
    if !crate::ml::models::is_installed(ModelId::Denoise) {
        return crate::app::handlers::models::offer(editor, ModelId::Denoise);
    }
    let (Some(image_id), Some(ctx), Some(main)) =
        (editor.selected_image_id, editor.gpu_context.clone(), editor.image_resources.clone())
    else {
        return Task::none();
    };
    if main.image_id != image_id {
        return Task::none();
    }
    let current = editor.current_edit_params.ai_denoise;
    editor.ai_denoise = AiDenoiseState {
        image_id: Some(image_id),
        amount: if current > 0.0 { current } else { 0.75 },
        center: editor.loupe.center,
        ..AiDenoiseState::default()
    };
    editor.active_modal = Modal::AiDenoise;

    let cached = editor.raw_cache.peek(&image_id).cloned();
    let path = editor.images.iter().find(|i| i.id == image_id).map(|i| i.path.clone());
    Task::perform(
        async move {
            let raw = match (cached, path) {
                (Some(raw), _) => raw,
                (None, Some(path)) => Arc::new(crate::raw::loader::load_raw_data(path).await?),
                (None, None) => return Err("image is no longer in the catalogue".to_string()),
            };
            tokio::task::spawn_blocking(move || {
                let layout = crate::ml::denoise::layout_of(&raw);
                let model = crate::ml::denoise::noise_model(&raw.data, &layout);
                let loupe = Loupe::new(&ctx, &main, raw)?;
                Ok(EngineHandle(Arc::new(Engine { loupe, model, layout })))
            })
            .await
            .map_err(|e| format!("AI denoise failed: {e}"))?
        },
        move |r| Message::AiDenoiseReady(image_id, r),
    )
}

fn open_for(editor: &RawEditor, image_id: i64) -> bool {
    editor.active_modal == Modal::AiDenoise && editor.ai_denoise.image_id == Some(image_id)
}

pub fn handle_ready(editor: &mut RawEditor, image_id: i64, result: Result<EngineHandle, String>) -> Task<Message> {
    if !open_for(editor, image_id) {
        return Task::none();
    }
    match result {
        Ok(engine) => {
            editor.ai_denoise.engine = Some(engine);
            request(editor)
        }
        Err(e) => {
            tracing::error!("AI denoise: {e}");
            editor.ai_denoise.error = Some(e);
            Task::none()
        }
    }
}

/// Bring the preview up to date: denoise a new crop if the area is off the
/// current one, else render (one of each in flight; later requests coalesce).
fn request(editor: &mut RawEditor) -> Task<Message> {
    let size = physical_size(editor);
    let st = &mut editor.ai_denoise;
    let (Some(image_id), Some(engine), Some(main)) = (st.image_id, st.engine.clone(), editor.image_resources.clone())
    else {
        return Task::none();
    };
    let covered = st.crop.as_ref().is_some_and(|c| engine.0.loupe.covers(&main, st.center, size, &c.region));
    if !covered {
        if st.crop_running {
            st.crop_pending = true;
            return Task::none();
        }
        st.crop_running = true;
        let center = st.center;
        return Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let e = &engine.0;
                    let region = e.loupe.region_for(&main, center, size, CROP_SLACK);
                    let original = e.loupe.cut(region);
                    // The crop keeps the mosaic's CFA phase (even origin), so
                    // the whole image's layout applies with its own size.
                    let layout = crate::ml::denoise::MosaicLayout {
                        width: region.w as usize,
                        height: region.h as usize,
                        ..e.layout
                    };
                    let denoised = crate::ml::denoise::denoise(&original, &layout, &e.model, &Progress::default())?;
                    Ok(CropHandle(Arc::new(AiCrop { region, original, denoised, id: crate::ml::masks::new_id() })))
                })
                .await
                .map_err(|e| format!("AI denoise failed: {e}"))?
            },
            move |r| Message::AiDenoiseCropReady(image_id, r),
        );
    }
    if st.render_in_flight {
        st.render_pending = true;
        return Task::none();
    }
    let (Some(ctx), Some(crop)) = (editor.gpu_context.clone(), st.crop.clone()) else {
        return Task::none();
    };
    st.render_in_flight = true;
    let amount = if st.before { 0.0 } else { st.amount };
    let center = st.center;
    let nr = crate::gpu::denoise::DenoiseSettings::from_params(&editor.view_params());
    Task::perform(
        async move {
            let data = crate::ml::denoise::blend(&crop.original, &crop.denoised, amount);
            // Same crop and amount → same key → no re-upload.
            let key = crop.id ^ ((amount * 1000.0).round() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let over = MosaicOverride { region: crop.region, data, key };
            engine.0.loupe.render(&ctx, &main, center, size, nr, Some(&over)).await
        },
        move |r| Message::AiDenoiseRendered(image_id, r),
    )
}

pub fn handle_crop_ready(editor: &mut RawEditor, image_id: i64, result: Result<CropHandle, String>) -> Task<Message> {
    if !open_for(editor, image_id) {
        return Task::none();
    }
    let st = &mut editor.ai_denoise;
    st.crop_running = false;
    match result {
        Ok(c) => st.crop = Some(c.0),
        Err(e) => {
            tracing::error!("AI denoise preview: {e}");
            st.error = Some(e);
            return Task::none();
        }
    }
    st.crop_pending = false;
    request(editor)
}

pub fn handle_rendered(editor: &mut RawEditor, image_id: i64, result: Result<LoupeFrame, String>) -> Task<Message> {
    if !open_for(editor, image_id) {
        return Task::none();
    }
    let st = &mut editor.ai_denoise;
    st.render_in_flight = false;
    match result {
        Ok(f) => {
            st.frame = Some(Frame {
                handle: iced::widget::image::Handle::from_rgba(f.size, f.size, f.rgba),
                window: f.window,
                size: f.size,
            });
        }
        // Panned past the crop between request and render: the next request
        // denoises the new area.
        Err(e) => tracing::debug!("AI denoise preview: {e}"),
    }
    if std::mem::take(&mut st.render_pending) {
        request(editor)
    } else {
        Task::none()
    }
}

pub fn handle_amount(editor: &mut RawEditor, amount: f32) -> Task<Message> {
    editor.ai_denoise.amount = amount.clamp(0.0, 1.0);
    request(editor)
}

pub fn handle_before(editor: &mut RawEditor, before: bool) -> Task<Message> {
    if editor.ai_denoise.before == before {
        return Task::none();
    }
    editor.ai_denoise.before = before;
    request(editor)
}

/// A drag in the preview, logical pixels: the content follows the pointer.
pub fn handle_pan(editor: &mut RawEditor, dx: f32, dy: f32) -> Task<Message> {
    let Some(f) = &editor.ai_denoise.frame else { return Task::none() };
    let per_px = [f.window[2] / f.size as f32, f.window[3] / f.size as f32];
    let half = [f.window[2] / 2.0, f.window[3] / 2.0];
    let c = &mut editor.ai_denoise.center;
    c[0] = (c[0] - dx * editor.scale_factor * per_px[0]).clamp(half[0], 1.0 - half[0]);
    c[1] = (c[1] - dy * editor.scale_factor * per_px[1]).clamp(half[1], 1.0 - half[1]);
    request(editor)
}

/// Apply (button or Enter): denoise the whole image, then set the edit.
pub fn handle_apply(editor: &mut RawEditor) -> Task<Message> {
    let st = &mut editor.ai_denoise;
    let (Some(image_id), Some(engine)) = (st.image_id, st.engine.clone()) else { return Task::none() };
    if st.job.is_some() {
        return Task::none();
    }
    let amount = st.amount;
    if amount <= 0.0 {
        return handle_remove(editor);
    }
    // Already computed for this image (an amount change): just set it.
    if crate::ml::denoise::has_cache(image_id) {
        return finish(editor, image_id, amount);
    }
    let progress = Arc::new(Progress::default());
    st.job = Some(progress.clone());
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let e = &engine.0;
                let raw = e.loupe.raw();
                let out = crate::ml::denoise::denoise(&raw.data, &e.layout, &e.model, &progress)?;
                crate::ml::denoise::save_cache(image_id, raw.width, raw.height, &out)
            })
            .await
            .map_err(|e| format!("AI denoise failed: {e}"))?
        },
        move |r| Message::AiDenoiseApplied(image_id, amount, r),
    )
}

pub fn handle_applied(editor: &mut RawEditor, image_id: i64, amount: f32, result: Result<(), String>) -> Task<Message> {
    editor.ai_denoise.job = None;
    match result {
        Ok(()) if open_for(editor, image_id) => finish(editor, image_id, amount),
        Ok(()) => Task::none(),
        Err(e) if e == crate::ml::CANCELLED => Task::none(),
        Err(e) => {
            tracing::error!("AI denoise: {e}");
            editor.ai_denoise.error = Some(e);
            Task::none()
        }
    }
}

/// Set the edit (undoable, saved) and rebuild the image with it.
fn finish(editor: &mut RawEditor, image_id: i64, amount: f32) -> Task<Message> {
    close(editor);
    if editor.selected_image_id != Some(image_id) {
        return Task::none();
    }
    editor.current_edit_params.ai_denoise = amount;
    editor.mark_edits_dirty();
    editor.commit_current_state();
    editor.status = if amount > 0.0 {
        format!("AI denoise applied ({:.0}%)", amount * 100.0)
    } else {
        "AI denoise removed".to_string()
    };
    crate::app::handlers::navigation::reload_selected_image(editor)
}

pub fn handle_remove(editor: &mut RawEditor) -> Task<Message> {
    let Some(image_id) = editor.ai_denoise.image_id else { return Task::none() };
    finish(editor, image_id, 0.0)
}

/// Cancel: stop a running Apply, or close.
pub fn handle_cancel(editor: &mut RawEditor) -> Task<Message> {
    if let Some(p) = &editor.ai_denoise.job {
        p.cancel();
        return Task::none();
    }
    close(editor);
    Task::none()
}

/// Close the modal and drop its engine (the loupe's mosaic, ~50 MB).
pub fn close(editor: &mut RawEditor) {
    if let Some(p) = &editor.ai_denoise.job {
        p.cancel();
    }
    let regen = editor.ai_denoise.regen.take();
    editor.ai_denoise = AiDenoiseState { regen, ..AiDenoiseState::default() };
    if editor.active_modal == Modal::AiDenoise {
        editor.active_modal = Modal::None;
    }
}

/// After new resources land: an edit asks for AI denoise but no result was
/// cached — compute it in the background, then rebuild.
pub fn after_resources(editor: &mut RawEditor) -> Task<Message> {
    let Some(res) = &editor.image_resources else { return Task::none() };
    let (requested, applied) = res.ai_denoise;
    let image_id = res.image_id;
    if requested <= 0.0 || applied > 0.0 || editor.ai_denoise.regen.is_some() {
        return Task::none();
    }
    if !crate::ml::models::is_installed(ModelId::Denoise) {
        editor.status = "AI denoise needs its model — Preferences › AI Models".into();
        return Task::none();
    }
    let progress = Arc::new(Progress::default());
    editor.ai_denoise.regen = Some((image_id, progress.clone()));
    editor.status = "Re-computing AI denoise…".into();
    let cached = editor.raw_cache.peek(&image_id).cloned();
    let path = editor.images.iter().find(|i| i.id == image_id).map(|i| i.path.clone());
    Task::perform(
        async move {
            let raw = match (cached, path) {
                (Some(raw), _) => raw,
                (None, Some(path)) => Arc::new(crate::raw::loader::load_raw_data(path).await?),
                (None, None) => return Err("image is no longer in the catalogue".to_string()),
            };
            tokio::task::spawn_blocking(move || regenerate(&raw, image_id, &progress).map(|_| ()))
                .await
                .map_err(|e| format!("AI denoise failed: {e}"))?
        },
        move |r| Message::AiDenoiseRegenerated(image_id, r),
    )
}

pub fn handle_regenerated(editor: &mut RawEditor, image_id: i64, result: Result<(), String>) -> Task<Message> {
    editor.ai_denoise.regen = None;
    match result {
        Ok(()) => {
            editor.status.clear();
            if editor.selected_image_id == Some(image_id) && editor.current_edit_params.ai_denoise > 0.0 {
                return crate::app::handlers::navigation::reload_selected_image(editor);
            }
            Task::none()
        }
        Err(e) => {
            tracing::error!("AI denoise for image {image_id}: {e}");
            editor.status = format!("AI denoise failed: {e}");
            Task::none()
        }
    }
}

/// Compute and cache an image's result. Blocking (seconds).
fn regenerate(raw: &crate::raw::loader::RawDataResult, image_id: i64, progress: &Progress) -> Result<Vec<u16>, String> {
    let layout = crate::ml::denoise::layout_of(raw);
    let model = crate::ml::denoise::noise_model(&raw.data, &layout);
    let out = crate::ml::denoise::denoise(&raw.data, &layout, &model, progress)?;
    crate::ml::denoise::save_cache(image_id, raw.width, raw.height, &out)?;
    Ok(out)
}

/// The mosaic an export uploads: AI denoise applied, regenerating its cache
/// if needed (an export must not silently drop an edit). Blocking.
pub fn export_mosaic(
    raw: &crate::raw::loader::RawDataResult,
    image_id: i64,
    amount: f32,
) -> std::borrow::Cow<'_, [u16]> {
    let (mosaic, applied) = crate::ml::denoise::effective_mosaic(raw, image_id, amount);
    if amount <= 0.0 || applied > 0.0 {
        return mosaic;
    }
    if !crate::ml::models::is_installed(ModelId::Denoise) {
        tracing::warn!("Exporting image {image_id} without its AI denoise: the model is not installed");
        return mosaic;
    }
    match regenerate(raw, image_id, &Progress::default()) {
        Ok(denoised) => std::borrow::Cow::Owned(crate::ml::denoise::blend(&raw.data, &denoised, amount)),
        Err(e) => {
            tracing::error!("Exporting image {image_id} without its AI denoise: {e}");
            mosaic
        }
    }
}

/// The open image's resources were built with a different AI denoise amount
/// than its edit now has (Apply, Remove, undo, paste…): rebuild.
pub fn resources_stale(editor: &RawEditor) -> bool {
    editor
        .image_resources
        .as_ref()
        .is_some_and(|r| r.ai_denoise.0 != editor.current_edit_params.ai_denoise)
}
