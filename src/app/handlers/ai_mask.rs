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
//! Separately, `track` keeps the selected image's decoded bitmaps in step
//! with its edits (selection, undo, paste…) and ends a session the user
//! walked away from.

use std::sync::{Arc, Mutex};

use iced::Task;

use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::core::types::{MaskParams, MASK_AI, MAX_MASKS};
use crate::gpu::ai_mask::AiBitmaps;
use crate::ml::masks::{self, Bitmap, Point, Prompts};
use crate::ml::models::ModelId;
use crate::ml::segment::{self, Decoder, Embedding, Logits, SamInput};

/// The models and embeddings for one image, shared with the worker tasks.
pub struct AiEngine {
    input: SamInput,
    embedding: Embedding,
    decoder: Decoder,
}

/// `Message`s must be `Debug + Clone`; the engine is neither.
#[derive(Clone)]
pub struct EngineHandle(Arc<Mutex<AiEngine>>);

impl std::fmt::Debug for EngineHandle {
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
    pub engine: Option<EngineHandle>,
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

fn session_token() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// `+ AI Select`: a new AI mask, selecting into it straight away.
pub fn handle_start(editor: &mut RawEditor) -> Task<Message> {
    if editor.ai_select.is_some() {
        return Task::none();
    }
    if !crate::ml::models::is_installed(ModelId::Segment) {
        return crate::app::handlers::models::offer(editor, ModelId::Segment);
    }
    let count = editor.current_edit_params.mask_count as usize;
    if count >= MAX_MASKS {
        editor.status = format!("At most {MAX_MASKS} masks per image");
        return Task::none();
    }
    editor.current_edit_params.masks[count] = MaskParams::new_ai();
    editor.current_edit_params.mask_count = (count + 1) as u32;
    open(editor, count, true)
}

/// `Refine` on an existing AI mask: reopen it with its prompts.
pub fn handle_refine(editor: &mut RawEditor, slot: usize) -> Task<Message> {
    if editor.ai_select.is_some()
        || slot >= editor.current_edit_params.mask_count as usize
        || editor.current_edit_params.masks[slot].mask_type != MASK_AI
    {
        return Task::none();
    }
    if !crate::ml::models::is_installed(ModelId::Segment) {
        return crate::app::handlers::models::offer(editor, ModelId::Segment);
    }
    open(editor, slot, false)
}

fn open(editor: &mut RawEditor, slot: usize, created: bool) -> Task<Message> {
    let Some(image_id) = editor.selected_image_id else { return Task::none() };
    let Some(res) = editor.image_resources.clone() else { return Task::none() };
    if res.image_id != image_id {
        return Task::none();
    }
    let params = editor.current_edit_params;
    let orientation = res.display_orientation(params.quarter_turns);
    let prev_ai_id = params.masks[slot].ai_id;
    let prompts = if prev_ai_id != 0 { masks::load_prompts(image_id, prev_ai_id) } else { Prompts::default() };
    let token = session_token();
    editor.ai_select = Some(AiSelect {
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
    editor.mask_tool = crate::app::state::MaskTool::Inactive;
    editor.brush_armed = false;
    editor.color_picking_mask = None;
    editor.selected_mask = Some(slot);
    editor.status = "AI Select: analysing the image…".into();
    let draw = crate::app::handlers::develop::update_pipeline(editor);

    let cached = editor.raw_cache.peek(&image_id).cloned();
    let path = editor.images.iter().find(|i| i.id == image_id).map(|i| i.path.clone());
    let black_offsets = params.black_offsets;
    let build = Task::perform(
        async move {
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
                Ok(EngineHandle(Arc::new(Mutex::new(AiEngine { input, embedding, decoder }))))
            })
            .await
            .map_err(|e| format!("AI select failed: {e}"))?
        },
        move |result| Message::AiEngineReady(token, result),
    );
    Task::batch([draw, build])
}

pub fn handle_engine_ready(editor: &mut RawEditor, token: u64, result: Result<EngineHandle, String>) -> Task<Message> {
    let Some(s) = editor.ai_select.as_mut().filter(|s| s.token == token) else {
        return Task::none(); // the session ended meanwhile; the engine drops here
    };
    match result {
        Ok(engine) => {
            s.engine = Some(engine);
            editor.status = HINT.into();
            decode(editor)
        }
        Err(e) => {
            tracing::error!("AI select: {e}");
            editor.status = format!("AI Select unavailable: {e}");
            s.error = Some(e);
            Task::none()
        }
    }
}

pub const HINT: &str = "AI Select: click to add · right-click or Alt-click to subtract · drag a box · Enter when done";

/// Full-image display UV → the sensor UV of the content shown there, through
/// the colour pass's Straighten and lens remap as well as the orientation, so
/// SAM is prompted at the content that was clicked. The mask is read back at
/// the same source position (`core::viewport::frame_to_layer_uv`).
fn source_uv(editor: &RawEditor, u: f32, v: f32) -> (f32, f32) {
    let Some(s) = editor.ai_select.as_ref() else { return (u, v) };
    match editor.raw_cache.peek(&s.image_id) {
        Some(raw) => {
            crate::core::viewport::frame_to_source_uv(
                &editor.current_edit_params,
                &raw.lens_context,
                s.orientation,
                raw.width,
                raw.height,
                u,
                v,
            )
            .0
        }
        None => crate::color::display_to_sensor_uv(u.clamp(0.0, 1.0), v.clamp(0.0, 1.0), s.orientation),
    }
}

/// A click on the photo, in full-image display UV.
pub fn handle_prompt(editor: &mut RawEditor, u: f32, v: f32, positive: bool) -> Task<Message> {
    let (su, sv) = source_uv(editor, u, v);
    let Some(s) = editor.ai_select.as_mut() else { return Task::none() };
    s.prompts.points.push(Point { u: su, v: sv, positive });
    decode(editor)
}

/// A dragged box, in full-image display UV.
pub fn handle_rect(editor: &mut RawEditor, u0: f32, v0: f32, u1: f32, v1: f32) -> Task<Message> {
    let a = source_uv(editor, u0, v0);
    let b = source_uv(editor, u1, v1);
    let Some(s) = editor.ai_select.as_mut() else { return Task::none() };
    s.prompts.rect = Some([a.0, a.1, b.0, b.1]);
    decode(editor)
}

/// Run the decoder on the current prompts, unless it is already running
/// (the result handler starts another if prompts arrived meanwhile).
fn decode(editor: &mut RawEditor) -> Task<Message> {
    let Some(s) = editor.ai_select.as_mut() else { return Task::none() };
    let Some(engine) = s.engine.clone() else { return Task::none() };
    if s.decoding || s.prompts.is_empty() {
        return Task::none();
    }
    s.decoding = true;
    let (image_id, token, prompts, previous) = (s.image_id, s.token, s.prompts.clone(), s.logits.clone());
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let mut engine = engine.0.lock().map_err(|_| "AI select engine poisoned".to_string())?;
                let engine = &mut *engine;
                let logits = engine.decoder.decode(&engine.embedding, &engine.input, &prompts, previous.as_ref())?;
                let bitmap = segment::to_bitmap(&logits, &engine.input);
                let ai_id = masks::new_id();
                masks::save(image_id, ai_id, &bitmap, &prompts)?;
                Ok(Selection { ai_id, bitmap: Arc::new(bitmap), logits, prompts: prompts.points.len() + prompts.rect.is_some() as usize })
            })
            .await
            .map_err(|e| format!("AI select failed: {e}"))?
        },
        move |result| Message::AiMaskReady(token, result),
    )
}

pub fn handle_mask_ready(editor: &mut RawEditor, token: u64, result: Result<Selection, String>) -> Task<Message> {
    let Some(s) = editor.ai_select.as_mut().filter(|s| s.token == token) else {
        return Task::none();
    };
    s.decoding = false;
    let sel = match result {
        Ok(sel) => sel,
        Err(e) => {
            tracing::error!("AI select: {e}");
            editor.status = format!("AI Select: {e}");
            return Task::none();
        }
    };
    let (slot, image_id) = (s.slot, s.image_id);
    let stale = sel.prompts < s.prompts.points.len() + s.prompts.rect.is_some() as usize;
    s.logits = Some(sel.logits.clone());
    insert_bitmap(editor, image_id, sel.ai_id, sel.bitmap);
    if slot < editor.current_edit_params.mask_count as usize {
        editor.current_edit_params.masks[slot].ai_id = sel.ai_id;
    }
    let draw = crate::app::handlers::develop::update_pipeline(editor);
    if stale {
        return Task::batch([draw, decode(editor)]);
    }
    draw
}

/// Done (button or Enter): keep the selection.
pub fn handle_done(editor: &mut RawEditor) -> Task<Message> {
    let Some(s) = editor.ai_select.take() else { return Task::none() };
    editor.status = String::new();
    // Nothing was ever selected in a new mask: drop the empty slot.
    if s.created && editor.current_edit_params.masks.get(s.slot).is_some_and(|m| m.ai_id == 0) {
        return crate::app::handlers::masks::handle_delete_mask(editor, s.slot);
    }
    let task = crate::app::handlers::develop::update_pipeline(editor);
    editor.commit_current_state();
    task
}

/// Cancel (button or Escape): put the slot back the way it was.
pub fn handle_cancel(editor: &mut RawEditor) -> Task<Message> {
    let Some(s) = editor.ai_select.take() else { return Task::none() };
    editor.status = String::new();
    if s.created {
        return crate::app::handlers::masks::handle_delete_mask(editor, s.slot);
    }
    if s.slot < editor.current_edit_params.mask_count as usize {
        editor.current_edit_params.masks[s.slot].ai_id = s.prev_ai_id;
    }
    let task = crate::app::handlers::develop::update_pipeline(editor);
    editor.commit_current_state();
    task
}

/// Make a bitmap available to the renderer for `image_id`.
fn insert_bitmap(editor: &mut RawEditor, image_id: i64, ai_id: u64, bitmap: Arc<Bitmap>) {
    let st = &mut editor.ai_bitmaps;
    if st.image_id != Some(image_id) {
        *st = AiBitmapState { image_id: Some(image_id), ..AiBitmapState::default() };
    }
    Arc::make_mut(&mut st.bitmaps).insert(ai_id, bitmap);
    install(editor);
}

/// Hand the current bitmaps to the image's GPU resources.
pub fn install(editor: &RawEditor) {
    let st = &editor.ai_bitmaps;
    for res in [&editor.image_resources, &editor.preview_resources].into_iter().flatten() {
        if st.image_id == Some(res.image_id) {
            res.set_ai_bitmaps(st.bitmaps.clone());
        }
    }
}

/// Bitmaps for every AI mask in `params`, read from disk. Blocking; for
/// export, whose resources are its own.
pub fn load_all(image_id: i64, params: &crate::core::types::EditParams) -> AiBitmaps {
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

pub fn handle_bitmaps_loaded(
    editor: &mut RawEditor,
    image_id: i64,
    loaded: Vec<(u64, Result<Arc<Bitmap>, String>)>,
) -> Task<Message> {
    if editor.ai_bitmaps.image_id != Some(image_id) {
        return Task::none();
    }
    for (id, result) in loaded {
        editor.ai_bitmaps.loading.retain(|&l| l != id);
        match result {
            Ok(b) => {
                Arc::make_mut(&mut editor.ai_bitmaps.bitmaps).insert(id, b);
            }
            Err(e) => {
                tracing::error!("AI mask {id} of image {image_id} could not be read: {e}");
                editor.ai_bitmaps.failed.push(id);
            }
        }
    }
    install(editor);
    crate::app::handlers::navigation::throttled_render(editor)
}

/// After every message: keep the bitmaps in step with the selected image's
/// edits, and end a session whose image, tab or orientation went away.
pub fn track(editor: &mut RawEditor) -> Option<Task<Message>> {
    let mut tasks = Vec::new();
    if let Some(s) = &editor.ai_select {
        if editor.selected_image_id != Some(s.image_id) {
            // The params are another image's now: only drop the session.
            // (`handle_image_selected` ends it properly before the switch.)
            editor.ai_select = None;
            editor.status = String::new();
            return track(editor);
        }
        let gone = editor.current_tab != crate::app::message::AppTab::Develop
            || editor
                .image_resources
                .as_ref()
                .is_some_and(|r| r.image_id == s.image_id
                    && r.display_orientation(editor.current_edit_params.quarter_turns) != s.orientation)
            || s.slot >= editor.current_edit_params.mask_count as usize;
        if gone {
            // Keep whatever was selected: leaving is not cancelling.
            tasks.push(handle_done(editor));
        }
    }

    let Some(image_id) = editor.selected_image_id else {
        return (!tasks.is_empty()).then(|| Task::batch(tasks));
    };
    if editor.ai_bitmaps.image_id != Some(image_id) {
        editor.ai_bitmaps = AiBitmapState { image_id: Some(image_id), ..AiBitmapState::default() };
    }
    let st = &editor.ai_bitmaps;
    let missing: Vec<u64> = editor
        .current_edit_params
        .ai_masks()
        .map(|(_, id)| id)
        .filter(|id| !st.bitmaps.contains_key(id) && !st.loading.contains(id) && !st.failed.contains(id))
        .collect();
    if !missing.is_empty() {
        editor.ai_bitmaps.loading.extend(&missing);
        tasks.push(Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    missing.into_iter().map(|id| (id, masks::load(image_id, id).map(Arc::new))).collect()
                })
                .await
                .unwrap_or_default()
            },
            move |loaded| Message::AiBitmapsLoaded(image_id, loaded),
        ));
    }
    (!tasks.is_empty()).then(|| Task::batch(tasks))
}

/// The session's clicks in full-image display UV, for the overlay.
pub fn display_points(editor: &RawEditor) -> Vec<(f32, f32, bool)> {
    editor
        .ai_select
        .as_ref()
        .map(|s| {
            s.prompts
                .points
                .iter()
                .map(|p| {
                    let (u, v) = crate::color::sensor_to_display_uv(p.u, p.v, s.orientation);
                    (u, v, p.positive)
                })
                .collect()
        })
        .unwrap_or_default()
}
