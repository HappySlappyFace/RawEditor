//! The AI-select tool: click (or drag a box) on the photo and SAM 2.1
//! proposes the object; more clicks add (left) or subtract (right / Alt).
//!
//! Lifecycle, all off the UI thread (constraint 2):
//! 1. **Start / Refine** opens a session on a mask slot and builds the
//!    engine in `spawn_blocking`: the mosaic → SAM's input, the encoder run
//!    (~1–3 s), and the small decoder session.
//! 2. **Each prompt** runs the decoder (~20 ms), refines the edge, writes the
//!    bitmap under a NEW id, and only then points the slot at it — a saved
//!    edit's id always has its file (`ml::masks`).
//! 3. **Done / Enter** commits; **Cancel / Escape** restores the slot. The
//!    engine — embeddings plus the decoder, ~100 MB — is dropped with the
//!    session, so nothing stays in memory once the tool closes.
//!
//! Separately, `track_ai_masks` keeps the selected image's decoded bitmaps in
//! step with its edits (selection, undo, paste…) and ends a session the user
//! walked away from.

use std::sync::{Arc, Mutex};

use super::{Effects, Session, SessionEvent};
use crate::session::AppTab;
use crate::session::MaskTool;
use crate::core::types::{MaskParams, MASK_AI, MAX_MASKS};
use crate::gpu::ai_mask::AiBitmaps;
use crate::ml::masks::{self, Bitmap, Point, Prompts};
use crate::ml::models::ModelId;
use crate::ml::segment::{self, Decoder, Embedding, Logits, SamInput};

/// A feature needs this AI model, which is not installed: the front end
/// offers to download it (and starts the feature again once it is).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelMissing(pub ModelId);

/// The models and embeddings for one image, shared with the worker tasks.
struct AiEngine {
    input: SamInput,
    embedding: Embedding,
    decoder: Decoder,
}

/// Events must be `Debug + Clone`; the engine is neither.
#[derive(Clone)]
pub struct SelectEngine(Arc<Mutex<AiEngine>>);

impl std::fmt::Debug for SelectEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AiEngine")
    }
}

/// One decoder answer, already on disk under `ai_id`.
#[derive(Clone)]
pub struct Selection {
    ai_id: u64,
    bitmap: Arc<Bitmap>,
    logits: Logits,
    /// How many prompts it answers: a result for an older prompt set is
    /// still shown, but another decode follows it.
    prompts: usize,
}

impl std::fmt::Debug for Selection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Selection({}, {}×{})", self.ai_id, self.bitmap.width, self.bitmap.height)
    }
}

/// An open AI-select session.
pub struct AiSelect {
    pub image_id: i64,
    pub slot: usize,
    /// Display orientation the engine was built for: a turn ends the session.
    pub orientation: u32,
    pub prompts: Prompts,
    /// What the slot pointed at before the session (restored on cancel).
    prev_ai_id: u64,
    /// The session added the slot (cancel removes it).
    created: bool,
    pub engine: Option<SelectEngine>,
    pub error: Option<String>,
    logits: Option<Logits>,
    decoding: bool,
    token: u64,
}

impl std::fmt::Debug for AiSelect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AiSelect(image {}, slot {}, {} prompts)", self.image_id, self.slot, self.prompts.points.len())
    }
}

impl AiSelect {
    pub fn analysing(&self) -> bool {
        self.engine.is_none() && self.error.is_none()
    }
}

/// The selected image's decoded AI-mask bitmaps.
#[derive(Debug, Default)]
pub struct AiBitmapState {
    pub image_id: Option<i64>,
    pub bitmaps: Arc<AiBitmaps>,
    /// Ids being read from disk right now.
    loading: Vec<u64>,
    /// Ids that failed to load (not retried for this image).
    failed: Vec<u64>,
}

#[cfg(any(test, feature = "test-support"))]
impl AiSelect {
    /// A session without an engine or a GPU, for message-level tests.
    pub fn for_test(image_id: i64, slot: usize, created: bool) -> Self {
        Self {
            image_id,
            slot,
            orientation: 0,
            prompts: Prompts::default(),
            prev_ai_id: 0,
            created,
            engine: None,
            error: None,
            logits: None,
            decoding: false,
            token: session_token(),
        }
    }

    /// The token its results carry.
    pub fn token(&self) -> u64 {
        self.token
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Selection {
    /// A decoder answer that never ran the decoder.
    pub fn for_test(ai_id: u64, bitmap: Bitmap) -> Self {
        Self { ai_id, bitmap: Arc::new(bitmap), logits: Logits::default_for_tests(), prompts: 0 }
    }
}

fn session_token() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

const HINT: &str = "AI Select: click to add · right-click or Alt-click to subtract · drag a box · Enter when done";

/// Bitmaps for every AI mask in `params`, read from disk. Blocking; for
/// export, whose resources are its own.
pub fn load_ai_bitmaps(image_id: i64, params: &crate::core::types::EditParams) -> AiBitmaps {
    params
        .ai_masks()
        .filter_map(|(_, id)| match masks::load(image_id, id) {
            Ok(b) => Some((id, Arc::new(b))),
            Err(e) => {
                tracing::error!("AI mask {id} of image {image_id}: {e}");
                None
            }
        })
        .collect()
}

impl Session {
    /// `+ AI Select`: a new AI mask, selecting into it straight away.
    pub fn ai_select_start(&mut self) -> Result<Effects, ModelMissing> {
        if self.ai_select.is_some() {
            return Ok(Effects::none());
        }
        if !crate::ml::models::is_installed(ModelId::Segment) {
            return Err(ModelMissing(ModelId::Segment));
        }
        let count = self.current_edit_params.mask_count as usize;
        if count >= MAX_MASKS {
            self.status = format!("At most {MAX_MASKS} masks per image");
            return Ok(Effects::none());
        }
        self.current_edit_params.masks[count] = MaskParams::new_ai();
        self.current_edit_params.mask_count = (count + 1) as u32;
        Ok(self.open_ai_select(count, true))
    }

    /// `Refine` on an existing AI mask: reopen it with its prompts.
    pub fn ai_select_refine(&mut self, slot: usize) -> Result<Effects, ModelMissing> {
        if self.ai_select.is_some()
            || slot >= self.current_edit_params.mask_count as usize
            || self.current_edit_params.masks[slot].mask_type != MASK_AI
        {
            return Ok(Effects::none());
        }
        if !crate::ml::models::is_installed(ModelId::Segment) {
            return Err(ModelMissing(ModelId::Segment));
        }
        Ok(self.open_ai_select(slot, false))
    }

    fn open_ai_select(&mut self, slot: usize, created: bool) -> Effects {
        let Some(image_id) = self.selected_image_id else { return Effects::none() };
        let Some(res) = self.image_resources.clone() else { return Effects::none() };
        if res.image_id != image_id {
            return Effects::none();
        }
        let params = self.current_edit_params;
        let orientation = res.display_orientation(params.quarter_turns);
        let prev_ai_id = params.masks[slot].ai_id;
        let prompts = if prev_ai_id != 0 { masks::load_prompts(image_id, prev_ai_id) } else { Prompts::default() };
        let token = session_token();
        self.ai_select = Some(AiSelect {
            image_id,
            slot,
            orientation,
            prompts,
            prev_ai_id,
            created,
            engine: None,
            error: None,
            logits: None,
            decoding: false,
            token,
        });
        // Other tools off: the canvas belongs to AI select now.
        self.mask_tool = MaskTool::Inactive;
        self.brush_armed = false;
        self.color_picking_mask = None;
        self.selected_mask = Some(slot);
        self.status = "AI Select: analysing the image…".into();
        let draw = self.update_pipeline();

        let cached = self.raw_for(image_id);
        let path = self.images.iter().find(|i| i.id == image_id).map(|i| i.path.clone());
        let black_offsets = params.black_offsets;
        let build = Effects::perform("AI select engine", async move {
            let result = async move {
                let raw = match (cached, path) {
                    (Some(raw), _) => raw,
                    (None, Some(path)) => Arc::new(crate::raw::loader::load_raw_data(path).await?),
                    (None, None) => return Err("image is no longer in the catalogue".to_string()),
                };
                tokio::task::spawn_blocking(move || {
                    let input = segment::prepare(&raw, black_offsets, orientation);
                    drop(raw);
                    let embedding = segment::embed(&input)?;
                    let decoder = Decoder::new()?;
                    Ok(SelectEngine(Arc::new(Mutex::new(AiEngine { input, embedding, decoder }))))
                })
                .await
                .map_err(|e| format!("AI select failed: {e}"))?
            }
            .await;
            SessionEvent::AiEngineReady(token, result)
        });
        Effects::batch([draw, build])
    }

    pub fn ai_engine_ready(&mut self, token: u64, result: Result<SelectEngine, String>) -> Effects {
        let Some(s) = self.ai_select.as_mut().filter(|s| s.token == token) else {
            return Effects::none(); // the session ended meanwhile; the engine drops here
        };
        match result {
            Ok(engine) => {
                s.engine = Some(engine);
                self.status = HINT.into();
                self.ai_decode()
            }
            Err(e) => {
                tracing::error!("AI select: {e}");
                self.status = format!("AI Select unavailable: {e}");
                s.error = Some(e);
                Effects::none()
            }
        }
    }

    /// Full-image display UV → the sensor UV of the content shown there, through
    /// the colour pass's Straighten and lens remap as well as the orientation, so
    /// SAM is prompted at the content that was clicked. The mask is read back at
    /// the same source position (`core::viewport::frame_to_layer_uv`).
    fn ai_source_uv(&self, u: f32, v: f32) -> (f32, f32) {
        let Some(s) = self.ai_select.as_ref() else { return (u, v) };
        match self.source_geometry(s.image_id) {
            Some(geo) => {
                crate::core::viewport::frame_to_source_uv(
                    &self.current_edit_params,
                    &geo.lens,
                    s.orientation,
                    geo.width,
                    geo.height,
                    u,
                    v,
                )
                .0
            }
            None => crate::color::display_to_sensor_uv(u.clamp(0.0, 1.0), v.clamp(0.0, 1.0), s.orientation),
        }
    }

    /// A click on the photo, in full-image display UV.
    pub fn ai_prompt(&mut self, u: f32, v: f32, positive: bool) -> Effects {
        let (su, sv) = self.ai_source_uv(u, v);
        let Some(s) = self.ai_select.as_mut() else { return Effects::none() };
        s.prompts.points.push(Point { u: su, v: sv, positive });
        self.ai_decode()
    }

    /// A dragged box, in full-image display UV. On a straightened image the
    /// box drawn on screen is a turned rectangle in the sensor frame, so SAM
    /// (whose boxes are axis-aligned) gets the bounds of all four corners.
    pub fn ai_rect(&mut self, u0: f32, v0: f32, u1: f32, v1: f32) -> Effects {
        let corners = [(u0, v0), (u1, v0), (u1, v1), (u0, v1)].map(|(u, v)| self.ai_source_uv(u, v));
        let Some(s) = self.ai_select.as_mut() else { return Effects::none() };
        let lo = corners.iter().fold((f32::MAX, f32::MAX), |m, c| (m.0.min(c.0), m.1.min(c.1)));
        let hi = corners.iter().fold((f32::MIN, f32::MIN), |m, c| (m.0.max(c.0), m.1.max(c.1)));
        s.prompts.rect = Some([lo.0, lo.1, hi.0, hi.1]);
        self.ai_decode()
    }

    /// Run the decoder on the current prompts, unless it is already running
    /// (the result handler starts another if prompts arrived meanwhile).
    fn ai_decode(&mut self) -> Effects {
        let Some(s) = self.ai_select.as_mut() else { return Effects::none() };
        let Some(engine) = s.engine.clone() else { return Effects::none() };
        if s.decoding || s.prompts.is_empty() {
            return Effects::none();
        }
        s.decoding = true;
        let (image_id, token, prompts, previous) = (s.image_id, s.token, s.prompts.clone(), s.logits.clone());
        Effects::perform("AI select decode", async move {
            let result = tokio::task::spawn_blocking(move || {
                let mut engine = engine.0.lock().map_err(|_| "AI select engine poisoned".to_string())?;
                let engine = &mut *engine;
                let logits = engine.decoder.decode(&engine.embedding, &engine.input, &prompts, previous.as_ref())?;
                let bitmap = segment::to_bitmap(&logits, &engine.input);
                let ai_id = masks::new_id();
                masks::save(image_id, ai_id, &bitmap, &prompts)?;
                Ok(Selection {
                    ai_id,
                    bitmap: Arc::new(bitmap),
                    logits,
                    prompts: prompts.points.len() + prompts.rect.is_some() as usize,
                })
            })
            .await
            .map_err(|e| format!("AI select failed: {e}"))
            .and_then(|r| r);
            SessionEvent::AiMaskReady(token, result)
        })
    }

    pub fn ai_mask_ready(&mut self, token: u64, result: Result<Selection, String>) -> Effects {
        let Some(s) = self.ai_select.as_mut().filter(|s| s.token == token) else {
            return Effects::none();
        };
        s.decoding = false;
        let sel = match result {
            Ok(sel) => sel,
            Err(e) => {
                tracing::error!("AI select: {e}");
                self.status = format!("AI Select: {e}");
                return Effects::none();
            }
        };
        let (slot, image_id) = (s.slot, s.image_id);
        let stale = sel.prompts < s.prompts.points.len() + s.prompts.rect.is_some() as usize;
        s.logits = Some(sel.logits.clone());
        self.insert_ai_bitmap(image_id, sel.ai_id, sel.bitmap);
        // Only into a slot that is still an AI mask: if the list shifted under the
        // session, the index may now name another kind of mask.
        if slot < self.current_edit_params.mask_count as usize
            && self.current_edit_params.masks[slot].mask_type == MASK_AI
        {
            self.current_edit_params.masks[slot].ai_id = sel.ai_id;
        }
        let draw = self.update_pipeline();
        if stale {
            return Effects::batch([draw, self.ai_decode()]);
        }
        draw
    }

    /// Done (button or Enter): keep the selection.
    pub fn ai_select_done(&mut self) -> Effects {
        let Some(s) = self.ai_select.take() else { return Effects::none() };
        self.status = String::new();
        // Nothing was ever selected in a new mask: drop the empty slot.
        if s.created && self.current_edit_params.masks.get(s.slot).is_some_and(|m| m.ai_id == 0) {
            return self.delete_mask(s.slot);
        }
        let effects = self.update_pipeline();
        self.commit_current_state();
        effects
    }

    /// Cancel (button or Escape): put the slot back the way it was.
    pub fn ai_select_cancel(&mut self) -> Effects {
        let Some(s) = self.ai_select.take() else { return Effects::none() };
        self.status = String::new();
        if s.created {
            return self.delete_mask(s.slot);
        }
        if s.slot < self.current_edit_params.mask_count as usize {
            self.current_edit_params.masks[s.slot].ai_id = s.prev_ai_id;
        }
        let effects = self.update_pipeline();
        self.commit_current_state();
        effects
    }

    /// Make a bitmap available to the renderer for `image_id`.
    fn insert_ai_bitmap(&mut self, image_id: i64, ai_id: u64, bitmap: Arc<Bitmap>) {
        let st = &mut self.ai_bitmaps;
        if st.image_id != Some(image_id) {
            *st = AiBitmapState { image_id: Some(image_id), ..AiBitmapState::default() };
        }
        Arc::make_mut(&mut st.bitmaps).insert(ai_id, bitmap);
        self.install_ai_bitmaps();
    }

    /// Hand the current bitmaps to the image's GPU resources.
    pub fn install_ai_bitmaps(&self) {
        let st = &self.ai_bitmaps;
        for res in [&self.image_resources, &self.preview_resources].into_iter().flatten() {
            if st.image_id == Some(res.image_id) {
                res.set_ai_bitmaps(st.bitmaps.clone());
            }
        }
    }

    pub fn ai_bitmaps_loaded(&mut self, image_id: i64, loaded: Vec<(u64, Result<Arc<Bitmap>, String>)>) -> Effects {
        if self.ai_bitmaps.image_id != Some(image_id) {
            return Effects::none();
        }
        for (id, result) in loaded {
            self.ai_bitmaps.loading.retain(|&l| l != id);
            match result {
                Ok(b) => {
                    Arc::make_mut(&mut self.ai_bitmaps.bitmaps).insert(id, b);
                }
                Err(e) => {
                    tracing::error!("AI mask {id} of image {image_id} could not be read: {e}");
                    self.ai_bitmaps.failed.push(id);
                }
            }
        }
        self.install_ai_bitmaps();
        self.throttled_render()
    }

    /// After every intent: keep the bitmaps in step with the selected image's
    /// edits, and end a session whose image, tab or orientation went away.
    pub fn track_ai_masks(&mut self) -> Effects {
        let mut effects = Vec::new();
        if let Some(s) = &self.ai_select {
            if self.selected_image_id != Some(s.image_id) {
                // The params are another image's now: only drop the session.
                // (The selection change ends it properly before the switch.)
                self.ai_select = None;
                self.status = String::new();
                return self.track_ai_masks();
            }
            let gone = self.current_tab != AppTab::Develop
                || self.image_resources.as_ref().is_some_and(|r| {
                    r.image_id == s.image_id && r.display_orientation(self.current_edit_params.quarter_turns) != s.orientation
                })
                || s.slot >= self.current_edit_params.mask_count as usize;
            if gone {
                // Keep whatever was selected: leaving is not cancelling.
                effects.push(self.ai_select_done());
            }
        }

        let Some(image_id) = self.selected_image_id else {
            return Effects::batch(effects);
        };
        if self.ai_bitmaps.image_id != Some(image_id) {
            self.ai_bitmaps = AiBitmapState { image_id: Some(image_id), ..AiBitmapState::default() };
        }
        let st = &self.ai_bitmaps;
        let missing: Vec<u64> = self
            .current_edit_params
            .ai_masks()
            .map(|(_, id)| id)
            .filter(|id| !st.bitmaps.contains_key(id) && !st.loading.contains(id) && !st.failed.contains(id))
            .collect();
        if !missing.is_empty() {
            self.ai_bitmaps.loading.extend(&missing);
            effects.push(Effects::perform("AI mask bitmaps", async move {
                let loaded = tokio::task::spawn_blocking(move || {
                    missing.into_iter().map(|id| (id, masks::load(image_id, id).map(Arc::new))).collect()
                })
                .await
                .unwrap_or_default();
                SessionEvent::AiBitmapsLoaded(image_id, loaded)
            }));
        }
        Effects::batch(effects)
    }

    /// The session's clicks in full-image display UV, for the overlay: where the
    /// picture SHOWS the content they were made on, through Straighten and the
    /// lens remap, so a dot stays on its subject (and under the cursor that made
    /// it) however the image is straightened.
    pub fn ai_display_points(&self) -> Vec<(f32, f32, bool)> {
        let Some(s) = self.ai_select.as_ref() else { return Vec::new() };
        let geo = self.source_geometry(s.image_id);
        s.prompts
            .points
            .iter()
            .map(|p| {
                let (u, v) = match &geo {
                    Some(g) => crate::core::viewport::source_to_frame_uv(
                        &self.current_edit_params,
                        &g.lens,
                        s.orientation,
                        g.width,
                        g.height,
                        p.u,
                        p.v,
                    ),
                    None => crate::color::sensor_to_display_uv(p.u, p.v, s.orientation),
                };
                (u, v, p.positive)
            })
            .collect()
    }
}
