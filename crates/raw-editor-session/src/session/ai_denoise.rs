//! AI denoise (PMRID, `ml::denoise`): a dialog with a 1:1 preview of a small
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
//!
//! The dialog is open exactly while `AiDenoiseState::image_id` is set: the
//! front end shows it from then until the session closes it (Apply, Remove,
//! Cancel), and calls [`Session::close_ai_denoise`] if it closes first.

use std::sync::Arc;

use super::{Effects, LoupeView, ModelMissing, Session, SessionEvent};
use crate::gpu::loupe::{Loupe, LoupeFrame, MosaicOverride, Region};
use crate::ml::models::ModelId;
use crate::ml::Progress;

/// The preview's side, logical pixels (×`scale_factor` for 1:1).
pub const PREVIEW_SIZE: f32 = 480.0;
/// Sensor pixels around the window that PMRID also gets: context for the
/// network at the window's edges, and room to pan before a re-run.
const CROP_SLACK: u32 = 160;

/// What the dialog holds for its image.
struct Engine {
    loupe: Loupe,
    /// Unit-range noise model of the whole mosaic (a crop cannot measure it).
    model: crate::core::noise::NoiseModel,
    layout: crate::ml::denoise::MosaicLayout,
}

#[derive(Clone)]
pub struct DenoiseEngine(Arc<Engine>);

impl std::fmt::Debug for DenoiseEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AiDenoiseEngine")
    }
}

/// One denoised crop: the RAW's samples and PMRID's, over `region`.
struct AiCrop {
    region: Region,
    original: Vec<u16>,
    denoised: Vec<u16>,
    id: u64,
}

#[derive(Clone)]
pub struct DenoiseCrop(Arc<AiCrop>);

impl std::fmt::Debug for DenoiseCrop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AiCrop({:?})", self.0.region)
    }
}

#[derive(Default)]
pub struct AiDenoiseState {
    /// The image the dialog is open on (`None`: closed).
    pub image_id: Option<i64>,
    /// 0..=1, the slider.
    pub amount: f32,
    /// Preview centre, view UV.
    pub center: [f32; 2],
    /// Holding "Before": show the original.
    pub before: bool,
    /// The preview on screen.
    pub frame: Option<LoupeView>,
    engine: Option<DenoiseEngine>,
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

    /// Become `next`, keeping a background regeneration. It runs whether the
    /// dialog is open or not; dropping its entry would lose its progress, and
    /// `regenerate_missing_ai_denoise` would start a second one meanwhile.
    fn reset_to(&mut self, next: AiDenoiseState) {
        let regen = self.regen.take();
        *self = AiDenoiseState { regen, ..next };
    }
}

/// Compute and cache an image's result. Blocking (seconds).
fn regenerate(raw: &crate::raw::loader::RawDataResult, image_id: i64, progress: &Progress) -> Result<Vec<u16>, String> {
    let layout = crate::ml::denoise::layout_of(raw);
    let model = crate::ml::denoise::noise_model(&raw.data, &layout);
    let out = crate::ml::denoise::denoise(&raw.data, &layout, &model, progress)?;
    crate::ml::denoise::save_cache(image_id, raw.width, raw.height, &raw.data, &out)?;
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

impl Session {
    fn ai_denoise_preview_size(&self) -> u32 {
        (PREVIEW_SIZE * self.scale_factor).round().clamp(64.0, 2048.0) as u32
    }

    /// `AI Denoise…`: open the dialog on the selected image. Nothing opens
    /// (`ai_denoise.image_id` stays `None`) without a loaded image.
    pub fn open_ai_denoise(&mut self) -> Result<Effects, ModelMissing> {
        if !crate::ml::models::is_installed(ModelId::Denoise) {
            return Err(ModelMissing(ModelId::Denoise));
        }
        let (Some(image_id), Some(ctx), Some(main)) =
            (self.selected_image_id, self.gpu_context.clone(), self.image_resources.clone())
        else {
            return Ok(Effects::none());
        };
        if main.image_id != image_id {
            return Ok(Effects::none());
        }
        let current = self.current_edit_params.ai_denoise;
        self.ai_denoise.reset_to(AiDenoiseState {
            image_id: Some(image_id),
            amount: if current > 0.0 { current } else { 0.75 },
            center: self.loupe.center,
            ..AiDenoiseState::default()
        });

        let cached = self.raw_for(image_id);
        let path = self.images.iter().find(|i| i.id == image_id).map(|i| i.path.clone());
        Ok(Effects::perform("AI denoise engine", async move {
            let result = async move {
                let raw = match (cached, path) {
                    (Some(raw), _) => raw,
                    (None, Some(path)) => Arc::new(crate::raw::loader::load_raw_data(path).await?),
                    (None, None) => return Err("image is no longer in the catalogue".to_string()),
                };
                tokio::task::spawn_blocking(move || {
                    let layout = crate::ml::denoise::layout_of(&raw);
                    let model = crate::ml::denoise::noise_model(&raw.data, &layout);
                    let loupe = Loupe::new(&ctx, &main, raw)?;
                    Ok(DenoiseEngine(Arc::new(Engine { loupe, model, layout })))
                })
                .await
                .map_err(|e| format!("AI denoise failed: {e}"))?
            }
            .await;
            SessionEvent::AiDenoiseReady(image_id, result)
        }))
    }

    fn ai_denoise_open_for(&self, image_id: i64) -> bool {
        self.ai_denoise.image_id == Some(image_id)
    }

    pub fn ai_denoise_ready(&mut self, image_id: i64, result: Result<DenoiseEngine, String>) -> Effects {
        if !self.ai_denoise_open_for(image_id) {
            return Effects::none();
        }
        match result {
            Ok(engine) => {
                self.ai_denoise.engine = Some(engine);
                self.request_ai_denoise_preview()
            }
            Err(e) => {
                tracing::error!("AI denoise: {e}");
                self.ai_denoise.error = Some(e);
                Effects::none()
            }
        }
    }

    /// Bring the preview up to date: denoise a new crop if the area is off the
    /// current one, else render (one of each in flight; later requests coalesce).
    fn request_ai_denoise_preview(&mut self) -> Effects {
        let size = self.ai_denoise_preview_size();
        let st = &mut self.ai_denoise;
        let (Some(image_id), Some(engine), Some(main)) = (st.image_id, st.engine.clone(), self.image_resources.clone())
        else {
            return Effects::none();
        };
        let covered = st.crop.as_ref().is_some_and(|c| engine.0.loupe.covers(&main, st.center, size, &c.region));
        if !covered {
            if st.crop_running {
                st.crop_pending = true;
                return Effects::none();
            }
            st.crop_running = true;
            let center = st.center;
            return Effects::perform("AI denoise crop", async move {
                let result = tokio::task::spawn_blocking(move || {
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
                    Ok(DenoiseCrop(Arc::new(AiCrop { region, original, denoised, id: crate::ml::masks::new_id() })))
                })
                .await
                .map_err(|e| format!("AI denoise failed: {e}"))
                .and_then(|r| r);
                SessionEvent::AiDenoiseCropReady(image_id, result)
            });
        }
        if st.render_in_flight {
            st.render_pending = true;
            return Effects::none();
        }
        let (Some(ctx), Some(crop)) = (self.gpu_context.clone(), st.crop.clone()) else {
            return Effects::none();
        };
        st.render_in_flight = true;
        let amount = if st.before { 0.0 } else { st.amount };
        let center = st.center;
        // Preview what Apply will give: with AI denoise at this amount the
        // classic noise reduction stands down; "Before" (0) shows it as it is now.
        let mut previewed = self.view_params();
        previewed.ai_denoise = amount;
        let nr = crate::gpu::denoise::DenoiseSettings::from_params(&previewed);
        Effects::perform("AI denoise preview", async move {
            let data = crate::ml::denoise::blend(&crop.original, &crop.denoised, amount);
            // Same crop and amount → same key → no re-upload.
            let key = crop.id ^ ((amount * 1000.0).round() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let over = MosaicOverride { region: crop.region, data, key };
            let result = engine.0.loupe.render(&ctx, &main, center, size, nr, Some(&over)).await;
            SessionEvent::AiDenoiseRendered(image_id, result)
        })
    }

    pub fn ai_denoise_crop_ready(&mut self, image_id: i64, result: Result<DenoiseCrop, String>) -> Effects {
        if !self.ai_denoise_open_for(image_id) {
            return Effects::none();
        }
        let st = &mut self.ai_denoise;
        st.crop_running = false;
        match result {
            Ok(c) => st.crop = Some(c.0),
            Err(e) => {
                tracing::error!("AI denoise preview: {e}");
                st.error = Some(e);
                return Effects::none();
            }
        }
        st.crop_pending = false;
        self.request_ai_denoise_preview()
    }

    pub fn ai_denoise_rendered(&mut self, image_id: i64, result: Result<LoupeFrame, String>) -> Effects {
        if !self.ai_denoise_open_for(image_id) {
            return Effects::none();
        }
        let st = &mut self.ai_denoise;
        st.render_in_flight = false;
        match result {
            Ok(f) => {
                st.frame = Some(LoupeView { image_id, rgba: Arc::from(f.rgba), window: f.window, size: f.size });
            }
            // Panned past the crop between request and render: the next request
            // denoises the new area.
            Err(e) => tracing::debug!("AI denoise preview: {e}"),
        }
        if std::mem::take(&mut st.render_pending) {
            self.request_ai_denoise_preview()
        } else {
            Effects::none()
        }
    }

    pub fn set_ai_denoise_amount(&mut self, amount: f32) -> Effects {
        self.ai_denoise.amount = amount.clamp(0.0, 1.0);
        self.request_ai_denoise_preview()
    }

    pub fn set_ai_denoise_before(&mut self, before: bool) -> Effects {
        if self.ai_denoise.before == before {
            return Effects::none();
        }
        self.ai_denoise.before = before;
        self.request_ai_denoise_preview()
    }

    /// A drag in the preview, logical pixels: the content follows the pointer.
    pub fn pan_ai_denoise(&mut self, dx: f32, dy: f32) -> Effects {
        let Some(f) = &self.ai_denoise.frame else { return Effects::none() };
        let per_px = [f.window[2] / f.size as f32, f.window[3] / f.size as f32];
        let half = [f.window[2] / 2.0, f.window[3] / 2.0];
        let scale = self.scale_factor;
        let c = &mut self.ai_denoise.center;
        c[0] = (c[0] - dx * scale * per_px[0]).clamp(half[0], 1.0 - half[0]);
        c[1] = (c[1] - dy * scale * per_px[1]).clamp(half[1], 1.0 - half[1]);
        self.request_ai_denoise_preview()
    }

    /// Apply (button or Enter): denoise the whole image, then set the edit.
    pub fn apply_ai_denoise(&mut self) -> Effects {
        let st = &mut self.ai_denoise;
        let (Some(image_id), Some(engine)) = (st.image_id, st.engine.clone()) else { return Effects::none() };
        if st.job.is_some() {
            return Effects::none();
        }
        let amount = st.amount;
        if amount <= 0.0 {
            return self.remove_ai_denoise();
        }
        // Already computed for this image (an amount change): just set it.
        {
            let raw = engine.0.loupe.raw();
            if crate::ml::denoise::has_cache(image_id, &raw.data, raw.width, raw.height) {
                return self.finish_ai_denoise(image_id, amount);
            }
        }
        let progress = Arc::new(Progress::default());
        st.job = Some(progress.clone());
        Effects::perform("AI denoise", async move {
            let result = tokio::task::spawn_blocking(move || {
                let e = &engine.0;
                let raw = e.loupe.raw();
                let out = crate::ml::denoise::denoise(&raw.data, &e.layout, &e.model, &progress)?;
                crate::ml::denoise::save_cache(image_id, raw.width, raw.height, &raw.data, &out)
            })
            .await
            .map_err(|e| format!("AI denoise failed: {e}"))
            .and_then(|r| r);
            SessionEvent::AiDenoiseApplied(image_id, amount, result)
        })
    }

    pub fn ai_denoise_applied(&mut self, image_id: i64, amount: f32, result: Result<(), String>) -> Effects {
        self.ai_denoise.job = None;
        match result {
            Ok(()) if self.ai_denoise_open_for(image_id) => self.finish_ai_denoise(image_id, amount),
            Ok(()) => Effects::none(),
            Err(e) if e == crate::ml::CANCELLED => Effects::none(),
            Err(e) => {
                tracing::error!("AI denoise: {e}");
                self.ai_denoise.error = Some(e);
                Effects::none()
            }
        }
    }

    /// Set the edit (undoable, saved) and rebuild the image with it.
    fn finish_ai_denoise(&mut self, image_id: i64, amount: f32) -> Effects {
        self.close_ai_denoise();
        if self.selected_image_id != Some(image_id) {
            return Effects::none();
        }
        self.current_edit_params.ai_denoise = amount;
        self.mark_edits_dirty();
        self.commit_current_state();
        self.status = if amount > 0.0 {
            format!("AI denoise applied ({:.0}%)", amount * 100.0)
        } else {
            "AI denoise removed".to_string()
        };
        self.reload_selected_image()
    }

    pub fn remove_ai_denoise(&mut self) -> Effects {
        let Some(image_id) = self.ai_denoise.image_id else { return Effects::none() };
        self.finish_ai_denoise(image_id, 0.0)
    }

    /// Cancel: stop a running Apply, or close.
    pub fn cancel_ai_denoise(&mut self) {
        if let Some(p) = &self.ai_denoise.job {
            p.cancel();
            return;
        }
        self.close_ai_denoise();
    }

    /// Close the dialog and drop its engine (the loupe's mosaic, ~50 MB).
    pub fn close_ai_denoise(&mut self) {
        if let Some(p) = &self.ai_denoise.job {
            p.cancel();
        }
        self.ai_denoise.reset_to(AiDenoiseState::default());
    }

    /// After new resources land: an edit asks for AI denoise but no result was
    /// cached — compute it in the background, then rebuild.
    pub fn regenerate_missing_ai_denoise(&mut self) -> Effects {
        let Some(res) = &self.image_resources else { return Effects::none() };
        let (requested, applied) = res.ai_denoise;
        let image_id = res.image_id;
        if requested <= 0.0 || applied > 0.0 || self.ai_denoise.regen.is_some() {
            return Effects::none();
        }
        if !crate::ml::models::is_installed(ModelId::Denoise) {
            self.status = "AI denoise needs its model — Preferences › AI Models".into();
            return Effects::none();
        }
        let progress = Arc::new(Progress::default());
        self.ai_denoise.regen = Some((image_id, progress.clone()));
        self.status = "Re-computing AI denoise…".into();
        let cached = self.raw_for(image_id);
        let path = self.images.iter().find(|i| i.id == image_id).map(|i| i.path.clone());
        Effects::perform("AI denoise regenerate", async move {
            let result = async move {
                let raw = match (cached, path) {
                    (Some(raw), _) => raw,
                    (None, Some(path)) => Arc::new(crate::raw::loader::load_raw_data(path).await?),
                    (None, None) => return Err("image is no longer in the catalogue".to_string()),
                };
                tokio::task::spawn_blocking(move || regenerate(&raw, image_id, &progress).map(|_| ()))
                    .await
                    .map_err(|e| format!("AI denoise failed: {e}"))?
            }
            .await;
            SessionEvent::AiDenoiseRegenerated(image_id, result)
        })
    }

    pub fn ai_denoise_regenerated(&mut self, image_id: i64, result: Result<(), String>) -> Effects {
        self.ai_denoise.regen = None;
        match result {
            Ok(()) => {
                self.status.clear();
                if self.selected_image_id == Some(image_id) && self.current_edit_params.ai_denoise > 0.0 {
                    return self.reload_selected_image();
                }
                // The user may have stepped to another image whose own request
                // `regenerate_missing_ai_denoise` refused while this job ran.
                // Never for the finished image itself: its resources predate
                // the cache and would ask for it again.
                if self.image_resources.as_ref().is_some_and(|r| r.image_id != image_id) {
                    return self.regenerate_missing_ai_denoise();
                }
                Effects::none()
            }
            Err(e) => {
                tracing::error!("AI denoise for image {image_id}: {e}");
                self.status = format!("AI denoise failed: {e}");
                // Another image waiting behind this one still gets its turn; the
                // failed image itself is not retried (that would loop).
                if self.image_resources.as_ref().is_some_and(|r| r.image_id != image_id) {
                    return self.regenerate_missing_ai_denoise();
                }
                Effects::none()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opening or closing the dialog keeps a background regeneration's
    /// entry: it used to be dropped on open, losing the job's progress and
    /// the guard against starting a second one.
    #[test]
    fn the_dialog_keeps_a_background_regeneration() {
        let mut state = AiDenoiseState { regen: Some((7, Arc::new(Progress::default()))), ..AiDenoiseState::default() };
        state.reset_to(AiDenoiseState { image_id: Some(7), amount: 0.75, ..AiDenoiseState::default() });
        assert_eq!(state.image_id, Some(7));
        assert!(state.regen.as_ref().is_some_and(|(id, _)| *id == 7), "opening dropped the regeneration");
        assert!(state.busy());
        state.reset_to(AiDenoiseState::default());
        assert!(state.image_id.is_none());
        assert!(state.regen.is_some(), "closing dropped the regeneration");
    }
}
