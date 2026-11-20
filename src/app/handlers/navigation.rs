use iced::{Task, Point};
use iced::widget::image::Handle;
use crate::app::state::{RawEditor, DragMode, PreviewHandle as _};
use crate::app::message::{Message, AppTab};
use crate::app::update::run;
use crate::session::{CachedPreview, ImageLoad, Moved, PreviewHandle, SelectionChange, TabLoad};
use crate::ui::preview_renderer::{CropHandle, MaskHandle};

pub fn handle_image_selected(editor: &mut RawEditor, image_id: i64) -> Task<Message> {
    select_image(editor, image_id, true)
}

/// Selection the app makes itself (after a delete, auto-advance, a marked
/// image): never reads the modifier keys, which may still be held from the
/// shortcut that caused it and would turn "move on" into a range select or
/// toggle the image out of the selection.
pub fn handle_image_moved(editor: &mut RawEditor, image_id: i64) -> Task<Message> {
    select_image(editor, image_id, false)
}

fn select_image(editor: &mut RawEditor, image_id: i64, from_click: bool) -> Task<Message> {
    // End an AI-select session where the image is left, while the params are
    // still this image's (the clean-up edits masks by slot index).
    let ai_done = if editor.session.selected_image_id != Some(image_id) {
        crate::app::handlers::ai_mask::handle_done(editor)
    } else {
        Task::none()
    };
    // Saves the outgoing image's edits and thumbnail: before anything moves.
    let persist = run(editor.session.leave_image(image_id));
    let shift = from_click && editor.last_modifiers.shift();
    let command = from_click && editor.last_modifiers.command();

    let change = if shift {
        // Range select from the anchor. Falls back to a plain click when
        // there is no anchor yet (first ever click held Shift).
        if let Some(ids) = select_range(editor, image_id) {
            // Shift alone replaces the selection; Shift+Ctrl adds to it.
            SelectionChange::Range { ids, add: command }
        } else {
            editor.selection_anchor = Some(image_id);
            SelectionChange::Only
        }
    } else if command {
        editor.selection_anchor = Some(image_id);
        SelectionChange::Toggle
    } else {
        editor.selection_anchor = Some(image_id);
        SelectionChange::Only
    };
    editor.session.change_selection(image_id, change);
    let load = editor.session.open_image(image_id);
    editor.canvas_cache.clear();

    let reveal = reveal_selection(editor);
    let load = match load {
        Some(load) => apply_image_load(editor, load),
        None => Task::none(),
    };
    Task::batch([ai_done, persist, load, reveal])
}

/// Carry out an image load the session started: run its work and show what
/// it says on the camera-preview image.
pub fn apply_image_load(editor: &mut RawEditor, load: ImageLoad) -> Task<Message> {
    match load.preview {
        PreviewHandle::Keep => {}
        PreviewHandle::Clear => editor.working_preview = None,
        PreviewHandle::Cached(preview) => editor.working_preview = Some(preview.to_handle()),
        PreviewHandle::Path(path) => editor.working_preview = Some(Handle::from_path(path)),
    }
    run(load.effects)
}

/// Follow the session's move of the selection.
pub fn apply_moved(editor: &mut RawEditor, moved: Moved) -> Task<Message> {
    match moved {
        Moved::Stay => Task::none(),
        Moved::To(next_id) => Task::done(Message::ImageMoved(next_id)),
        Moved::Cleared => {
            editor.working_preview = None;
            Task::none()
        }
    }
}

/// Scroll the current tab's grid or filmstrip so the selection is visible.
///
/// Arrow-key navigation used to walk straight off the edge of both: the
/// selection moved, the view did not. A no-op when the selection is already
/// in view, so a click never makes anything jump.
pub fn reveal_selection(editor: &mut RawEditor) -> Task<Message> {
    reveal(editor, true)
}

/// [`reveal_selection`] without the glide: for a tab switch, whose grid /
/// filmstrip has just been rebuilt at offset 0 — gliding from there would
/// sweep past (and load) every thumbnail between the start and the
/// selection, and there is no previous position for the eye to follow.
pub fn reveal_selection_instantly(editor: &mut RawEditor) -> Task<Message> {
    reveal(editor, false)
}

fn reveal(editor: &mut RawEditor, animate: bool) -> Task<Message> {
    use crate::app::handlers::scroll::{glide_destination, glide_to};
    use crate::app::state::GlideTarget;

    let Some(selected) = editor.session.selected_image_id else {
        return Task::none();
    };
    let Some(index) = editor.session.displayed_ids().iter().position(|&id| id == selected) else {
        return Task::none();
    };
    // "Is it in view?" is asked of where the view is *heading*: mid-glide,
    // the current offset is a transient, and holding an arrow key would
    // otherwise keep re-aiming at stale positions.
    match editor.session.current_tab {
        AppTab::Library => {
            let geom = crate::app::views::library::GridGeometry::of(editor);
            let from = glide_destination(editor, GlideTarget::Grid).unwrap_or(editor.grid_scroll_y);
            if let Some(y) = geom.scroll_to_reveal(index / geom.columns, from) {
                return glide_to(editor, GlideTarget::Grid, y, animate);
            }
        }
        AppTab::Cull | AppTab::Develop => {
            let view_width = if editor.filmstrip_view_w > 0.0 {
                editor.filmstrip_view_w
            } else {
                editor.window_size.width
            };
            let from = glide_destination(editor, GlideTarget::Filmstrip)
                .unwrap_or(editor.filmstrip_scroll_x);
            // Stepping through: just far enough. Arriving: centred.
            let how = if animate {
                crate::ui::filmstrip::Reveal::Edge
            } else {
                crate::ui::filmstrip::Reveal::Center
            };
            if let Some(x) = crate::ui::filmstrip::scroll_to_reveal(index, from, view_width, how) {
                // A reveal overrides any momentum glide.
                editor.filmstrip_velocity = 0.0;
                editor.last_kinetic_tick = None;
                return glide_to(editor, GlideTarget::Filmstrip, x, animate);
            }
        }
        AppTab::Export => {}
    }
    Task::none()
}

/// Library: a double-clicked thumbnail opens in Develop. The double click's
/// first press has already selected it (a plain click); the select here
/// covers a double click that arrives without one.
pub fn handle_open_in_develop(editor: &mut RawEditor, image_id: i64) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    let select = if editor.session.selected_image_id != Some(image_id) {
        handle_image_selected(editor, image_id)
    } else {
        Task::none()
    };
    Task::batch([select, handle_tab_changed(editor, AppTab::Develop)])
}

pub fn handle_tab_changed(editor: &mut RawEditor, tab: AppTab) -> Task<Message> {
    if editor.session.current_tab != tab {
        // A tab switch rebuilds the grid / filmstrip scrollables from scratch,
        // at offset 0. Match that before anything virtualises against stale
        // offsets (which would build slots far from what is on screen: a
        // blank strip). Re-clicking the active tab rebuilds nothing, so it
        // must not reset them either.
        editor.grid_scroll_y = 0.0;
        editor.filmstrip_scroll_x = 0.0;
        editor.filmstrip_velocity = 0.0;
        editor.last_kinetic_tick = None;
    }
    let persist = run(editor.session.set_tab(tab));
    let reveal = reveal_selection_instantly(editor);
    let load = match editor.session.load_for_tab(tab) {
        TabLoad::Nothing => Task::none(),
        TabLoad::Moved(moved) => apply_moved(editor, moved),
        TabLoad::Load(load) => apply_image_load(editor, load),
    };
    Task::batch([persist, reveal, load])
}


/// Ids between the selection anchor and `to_id` inclusive, in display order.
///
/// Returns `None` when there is no anchor, or when either endpoint is not
/// currently displayed — a range spanning images the active filter is hiding
/// would select things the user cannot see.
fn select_range(editor: &RawEditor, to_id: i64) -> Option<Vec<i64>> {
    let anchor = editor.selection_anchor?;
    let ids = editor.session.displayed_ids();
    let a = ids.iter().position(|id| *id == anchor)?;
    let b = ids.iter().position(|id| *id == to_id)?;
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    Some(ids[lo..=hi].to_vec())
}

/// Ctrl+A — select every image the current tab is displaying.
pub fn handle_select_all(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    editor.session.select_all();
    Task::none()
}



/// If Develop is the active tab and its selection is marked for removal,
/// move on to the next unmarked image (`Session::ensure_selection_not_marked`).
pub fn ensure_develop_selection_not_marked(editor: &mut RawEditor) -> Task<Message> {
    let moved = editor.session.ensure_selection_not_marked();
    apply_moved(editor, moved)
}

/// The next (or previous) image the current tab displays.
pub(crate) fn step_selection(editor: &RawEditor, forward: bool) -> Option<i64> {
    editor.session.step_selection(forward)
}

pub fn handle_select_next_image(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    match step_selection(editor, true) {
        Some(next) => Task::done(Message::ImageMoved(next)),
        None => Task::none(),
    }
}

pub fn handle_select_previous_image(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None {
        return Task::none();
    }
    match step_selection(editor, false) {
        Some(prev) => Task::done(Message::ImageMoved(prev)),
        None => Task::none(),
    }
}

pub fn handle_zoom(editor: &mut RawEditor, d: f32, mut p: Point) -> Task<Message> {
    if editor.session.is_cropping { return Task::none(); }
    if p.x < 0.0 { p = editor.last_cursor_position.unwrap_or(Point::ORIGIN); }

    if editor.session.image_resources.is_some() {
        let old_zoom = editor.session.zoom;
        let (dw, dh) = editor.session.image_display_dims();
        let (vw, vh) = editor.session.viewport_size;
        let (fw, fh) = crate::core::viewport::fitted_size(dw, dh, vw, vh);
        let ib = crate::core::viewport::image_rect(dw, dh, vw, vh, old_zoom, editor.session.pan_offset);

        // Fraction of the CURRENT on-screen image rect under the cursor —
        // this is the point we want to keep fixed on screen as zoom changes.
        let tx = ((p.x - ib.x) / ib.width.max(1.0)).clamp(0.0, 1.0);
        let ty = ((p.y - ib.y) / ib.height.max(1.0)).clamp(0.0, 1.0);

        let new_zoom = if d > 0.0 { old_zoom * (1.0 + d * 0.8) } else { old_zoom / (1.0 + (-d * 0.8)) }.clamp(0.1, 10.0);
        editor.session.zoom = new_zoom;

        // Closed-form inversion of image_rect: solve pan so that the point at
        // fraction (tx, ty) of the rect lands back under the cursor at the
        // new zoom. From image_rect: p.x = vw/2 - fw*zoom/2 + offset.x*fw + tx*fw*zoom
        //   => offset.x = (p.x - vw/2)/fw - zoom*(tx - 0.5)
        editor.session.pan_offset.x = (p.x - vw / 2.0) / fw.max(1.0) - new_zoom * (tx - 0.5);
        editor.session.pan_offset.y = (p.y - vh / 2.0) / fh.max(1.0) - new_zoom * (ty - 0.5);
    } else {
        editor.session.zoom = if d > 0.0 { editor.session.zoom * (1.0 + d * 0.8) } else { editor.session.zoom / (1.0 + (-d * 0.8)) }.clamp(0.1, 10.0);
    }
    editor.canvas_cache.clear();

    // Re-render at zoom-appropriate resolution (which upgrades to the full-res
    // debayer once the subsampled texture is past its useful range).
    use crate::app::state::EditorReadiness;
    if editor.session.current_tab == AppTab::Develop
        && matches!(editor.session.editor_readiness, EditorReadiness::Ready(_))
    {
        // Zoom always re-renders: the window and its resolution both change.
        // Throttled so a stream of scroll events can't pile up GPU tasks.
        // The render itself asks for full resolution once the view needs it
        // (`develop::trigger_async_render`).
        return throttled_render(editor);
    }
    Task::none()
}

pub fn handle_pan(editor: &mut RawEditor, d: cgmath::Vector2<f32>) -> Task<Message> {
    if editor.session.is_cropping { return Task::none(); }
    // Phase 116: Remove 1/zoom scaling. Standardised to 1:1 mapping with screen pixels.
    editor.session.pan_offset.x += d.x;
    editor.session.pan_offset.y += d.y;
    editor.canvas_cache.clear();

    // The pan itself stays free — the viewport shader just re-projects the
    // existing texture. But that texture now only covers `rendered_view_rect`,
    // so once the user drags past the overscan margin there is nothing left to
    // project and the window has to be re-rendered.
    if needs_render_for_view(editor) {
        return throttled_render(editor);
    }
    Task::none()
}

/// True when the on-screen region has moved outside what the current preview
/// buffer covers. Always false at fit, where the buffer holds the whole image.
fn needs_render_for_view(editor: &RawEditor) -> bool {
    use crate::app::state::EditorReadiness;
    if editor.session.current_tab != AppTab::Develop
        || !matches!(editor.session.editor_readiness, EditorReadiness::Ready(_))
    {
        return false;
    }
    let (dw, dh) = editor.session.image_display_dims();
    let (vw, vh) = editor.session.viewport_size;
    // Overscan 1.0: ask only what is strictly visible right now. The rendered
    // window already carries the margin, so this asks "have we run out of
    // margin?" rather than "has anything moved at all?".
    let needed = crate::core::viewport::visible_view_rect(
        dw, dh, vw, vh, editor.session.zoom, editor.session.pan_offset, 1.0,
    );
    // Against the REQUESTED window: if a render covering this area is already
    // on its way, asking for another one would just churn.
    !crate::core::viewport::view_rect_contains(editor.session.requested_view_rect, needed)
}

/// Start a render unless one is already in flight, in which case mark a
/// follow-up. At most one running plus one pending — without this, a stream of
/// scroll or drag events spawns a GPU task each, piling up readback buffers.
pub fn throttled_render(editor: &mut RawEditor) -> Task<Message> {
    if editor.session.is_rendering {
        editor.session.pending_render = true;
        Task::none()
    } else {
        editor.session.is_rendering = true;
        editor.session.pending_render = false;
        crate::app::handlers::develop::trigger_async_render(editor)
    }
}

pub fn handle_reset_view(editor: &mut RawEditor) -> Task<Message> {
    editor.session.zoom = 1.0;
    editor.session.pan_offset = cgmath::Vector2::new(0.0, 0.0);
    editor.canvas_cache.clear();
    // Must re-render: the buffer holds whatever window the previous zoom
    // needed, so without this the viewport would draw that small slice
    // correctly positioned inside the now fit-sized image — the rest blank.
    throttled_render(editor)
}

pub fn handle_mouse_pressed(editor: &mut RawEditor) -> Task<Message> {
    let now = std::time::Instant::now();
    let double = editor.last_click_time.map(|t| now.duration_since(t).as_millis() < 300).unwrap_or(false);
    editor.last_click_time = Some(now);
    if double { return Task::done(Message::ResetView); }
    if !editor.session.is_cropping && !editor.session.is_wb_picking && !editor.session.is_leveling && !editor.session.mask_overlay_active() {
        editor.is_dragging = true;
        editor.drag_mode = DragMode::Pan;
    }
    Task::none()
}

pub fn handle_mouse_released(editor: &mut RawEditor) -> Task<Message> {
    if editor.is_dragging
        && matches!(
            editor.drag_mode,
            DragMode::CropHandle(_) | DragMode::MaskHandle(_)
        )
    {
        editor.session.commit_current_state();
        if let (Some(ctx), Some(res)) = (&editor.session.gpu_context, &editor.session.image_resources) {
            let params = editor.session.view_params();
            let (interpolated, wb_override) =
                crate::app::handlers::develop::resolve_wb_and_dcp(editor, &params);
            res.update_uniforms(ctx, &params, interpolated.as_deref(), wb_override);
        }
    }
    editor.is_dragging = false;
    editor.drag_mode = DragMode::None;
    editor.last_cursor_position = None;
    Task::none()
}

pub fn handle_mouse_moved(editor: &mut RawEditor, pos: Point) -> Task<Message> {
    // Phase 116: Remove broken viewport_size auto-inflation loop.
    // viewport_size is now managed via Message::ViewportResized.
    
    if editor.session.is_cropping {
        handle_crop_interaction(editor, pos)
    } else if editor.session.mask_overlay_active()
        || matches!(editor.drag_mode, DragMode::MaskHandle(_))
    {
        handle_mask_interaction(editor, pos)
    } else {
        handle_pan_interaction(editor, pos)
    }
}

pub fn handle_working_preview_ready(
    editor: &mut RawEditor,
    id: i64,
    bytes: Option<std::sync::Arc<[u8]>>,
    dims: (u32, u32),
    path: String,
) -> Task<Message> {
    if editor.session.working_preview_ready(id, bytes.clone(), dims) {
        editor.working_preview = Some(match bytes {
            Some(bytes) => CachedPreview { width: dims.0, height: dims.1, bytes }.to_handle(),
            None => Handle::from_path(path),
        });
    }
    Task::none()
}

pub fn handle_preview_cached(
    editor: &mut RawEditor,
    id: i64,
    result: Result<(u32, u32, std::sync::Arc<[u8]>), String>,
) -> Task<Message> {
    editor.session.preview_cached(id, result);
    Task::none()
}

// Helpers

fn handle_pan_interaction(editor: &mut RawEditor, pos: Point) -> Task<Message> {
    if editor.is_dragging && editor.drag_mode == DragMode::Pan {
        if let Some(last) = editor.last_cursor_position {
            let delta = pos - last;
            editor.last_cursor_position = Some(pos);

            let (vw, vh) = editor.session.viewport_size;
            let (dw, dh) = editor.session.image_display_dims();
            let (fw, fh) = crate::core::viewport::fitted_size(dw, dh, vw, vh);

            // Pan is in fitted-size fractions with no zoom term (see
            // core::viewport::image_rect), so this is exact 1:1 cursor
            // tracking at any zoom level.
            let dx = if fw > 0.0 { delta.x / fw } else { 0.0 };
            let dy = if fh > 0.0 { delta.y / fh } else { 0.0 };
            return Task::done(Message::Pan(cgmath::Vector2::new(dx, dy)));
        }
    }
    editor.last_cursor_position = Some(pos);
    Task::none()
}

fn handle_crop_interaction(editor: &mut RawEditor, pos: Point) -> Task<Message> {
    if editor.is_dragging {
        if let DragMode::CropHandle(h) = editor.drag_mode {
            if let Some(last) = editor.last_cursor_position {
                apply_crop_drag(editor, pos, last, h);
                editor.last_cursor_position = Some(pos);
                editor.canvas_cache.clear();
            }
        }
    } else {
        editor.last_cursor_position = Some(pos);
    }
    Task::none()
}

fn apply_crop_drag(editor: &mut RawEditor, pos: Point, last: Point, h: CropHandle) {
    let resources = match &editor.session.image_resources {
        Some(r) => r,
        None => return,
    };
    
    let (dw, dh) = editor.session.image_display_dims();
    let (bw, bh) = editor.session.viewport_size;
    let (fw, fh) = crate::core::viewport::fitted_size(dw, dh, bw, bh);

    let zw = fw * editor.session.zoom;
    let zh = fh * editor.session.zoom;
    
    // Scale delta from screen pixels to normalized image [0,1]
    let dx = (pos.x - last.x) / zw;
    let dy = (pos.y - last.y) / zh;
    
    let c = editor.session.current_edit_params.crop;
    let (mut l, mut t, mut r, mut b) = (c[0], c[1], c[0]+c[2], c[1]+c[3]);
    
    match h {
        CropHandle::TopLeft => { l += dx; t += dy; }
        CropHandle::TopRight => { t += dy; r += dx; }
        CropHandle::BottomLeft => { l += dx; b += dy; }
        CropHandle::BottomRight => { r += dx; b += dy; }
        CropHandle::Body => { 
            l += dx; t += dy; r += dx; b += dy; 
            if l < 0.0 { r -= l; l = 0.0; } 
            if r > 1.0 { l -= r - 1.0; r = 1.0; } 
            if t < 0.0 { b -= t; t = 0.0; } 
            if b > 1.0 { t -= b - 1.0; b = 1.0; } 
        }
    }
    
    if h != CropHandle::Body {
        let min = 0.01;
        match h {
            CropHandle::TopLeft => { l = l.min(r - min).max(0.0); t = t.min(b - min).max(0.0); }
            CropHandle::TopRight => { r = r.max(l + min).min(1.0); t = t.min(b - min).max(0.0); }
            CropHandle::BottomLeft => { l = l.min(r - min).max(0.0); b = b.max(t + min).min(1.0); }
            CropHandle::BottomRight => { r = r.max(l + min).min(1.0); b = b.max(t + min).min(1.0); }
            _ => {}
        }
    }
    
    let l = l.clamp(0.0, 1.0);
    let t = t.clamp(0.0, 1.0);
    let r = r.clamp(0.0, 1.0);
    let b = b.clamp(0.0, 1.0);
    
    editor.session.current_edit_params.crop = [l, t, (r - l).max(0.0), (b - t).max(0.0)];
    // The release commits via commit_current_state, whose flush only writes
    // when dirty. Nothing else marks a crop drag dirty, so every drag after
    // the first in one crop session was dropped on close. (The field, not
    // `mark_edits_dirty()`: `resources` still borrows `editor.session.image_resources`.)
    editor.session.edits_dirty = true;

    if let Some(ctx) = &editor.session.gpu_context {
        let params = editor.session.view_params();
        let (interpolated, wb_override) =
            crate::app::handlers::develop::resolve_wb_and_dcp(editor, &params);
        resources.update_uniforms(ctx, &params, interpolated.as_deref(), wb_override);
    }
}

fn handle_mask_interaction(editor: &mut RawEditor, pos: Point) -> Task<Message> {
    if editor.is_dragging {
        if let DragMode::MaskHandle(h) = editor.drag_mode {
            if let Some(last) = editor.last_cursor_position {
                apply_mask_drag(editor, pos, last, h);
                editor.last_cursor_position = Some(pos);
                editor.canvas_cache.clear();
                // Unlike crop (a canvas-only overlay), the mask's effect is
                // baked into the render — re-render live, throttled.
                return crate::app::handlers::develop::update_pipeline(editor);
            }
        }
    } else {
        editor.last_cursor_position = Some(pos);
    }
    Task::none()
}

fn apply_mask_drag(editor: &mut RawEditor, pos: Point, last: Point, h: MaskHandle) {
    let Some(index) = editor.session.selected_mask else {
        return;
    };
    if index >= editor.session.current_edit_params.mask_count as usize {
        return;
    }
    if editor.session.image_resources.is_none() {
        return;
    }

    // Same letterbox/zoom math as apply_crop_drag, sharing the same dims
    // source as every other on-screen geometry consumer.
    let (dw, dh) = editor.session.image_display_dims();
    let (bw, bh) = editor.session.viewport_size;
    let (fw, fh) = crate::core::viewport::fitted_size(dw, dh, bw, bh);
    // Captured before `m` borrows editor.session.current_edit_params below — needed
    // by the Rotation arm's image_rect call.
    let zoom = editor.session.zoom;
    let pan_offset = editor.session.pan_offset;

    let zw = fw * zoom;
    let zh = fh * zoom;

    // Screen deltas map to visible-area UV; mask geometry lives in full-image
    // UV (the crop sub-rect is what's displayed), so scale by the crop extent.
    let crop = editor.session.current_edit_params.crop;
    let du = (pos.x - last.x) / zw.max(1.0) * crop[2];
    let dv = (pos.y - last.y) / zh.max(1.0) * crop[3];
    // dw/dh are already the oriented display dims (editor.session.image_display_dims()), so
    // no separate orientation swap is needed here — unlike the shader, which
    // starts from raw sensor dims. The crop term matters: the displayed image
    // is the crop sub-rect stretched to the full target, so under a crop whose
    // aspect differs from the frame's the two UV axes carry different
    // pixel-per-unit scales (see core::viewport::mask_uv_aspect).
    let aspect = crate::core::viewport::mask_uv_aspect(dw, dh, crop);

    let m = &mut editor.session.current_edit_params.masks[index];
    match h {
        MaskHandle::LinearStart => {
            m.ax += du;
            m.ay += dv;
        }
        MaskHandle::LinearEnd => {
            m.bx += du;
            m.by += dv;
        }
        MaskHandle::Center => {
            // Radial: move the center. Linear: move the whole gradient.
            // Translation commutes with rotation, so no rotation-aware math
            // is needed here even for a rotated ellipse.
            m.ax += du;
            m.ay += dv;
            if m.mask_type == 0 {
                m.bx += du;
                m.by += dv;
            }
        }
        MaskHandle::RadiusX | MaskHandle::RadiusY => {
            // The handles themselves are drawn rotated with the ellipse
            // (see mask_handle_positions), so dragging must project the
            // screen delta onto the ellipse's own (rotated) local axes —
            // otherwise the handle would visually stop responding to drags
            // once rotated away from the image axes. Same isotropic
            // un-distort/rotate/redistort technique as mask_weight's
            // aspect correction, just for a delta vector instead of a point.
            let rot = m.rotation.to_radians();
            let (rc, rs) = (rot.cos(), rot.sin());
            let iso_du = du * aspect;
            let iso_dv = dv;
            let local_dx = (iso_du * rc + iso_dv * rs) / aspect.max(1e-6);
            let local_dy = -iso_du * rs + iso_dv * rc;
            if h == MaskHandle::RadiusX {
                m.bx = (m.bx + local_dx).max(0.005);
            } else {
                m.by = (m.by + local_dy).max(0.005);
            }
        }
        MaskHandle::RadiusUniform => {
            // Grow both radii from the same horizontal delta so a single
            // drag makes a circle by default. UV isn't square-pixel — a
            // radius of `r` physical pixels is `r/width` in u but `r/height`
            // in v — so the v radius must be scaled by width/height to look
            // circular on screen rather than squashed/stretched.
            m.bx = (m.bx + du).max(0.005);
            m.by = (m.by + du * aspect).max(0.005);
        }
        MaskHandle::Rotation => {
            // Angle-based, not delta-based: compute the ellipse's
            // screen-space center via the same shared transform/crop-remap
            // CropOverlay's uv_to_screen uses, then rotate by the change in
            // atan2 angle from center to cursor between the last and
            // current positions. Matches iced's Elliptical.rotation
            // convention (clockwise for positive angles, screen Y down).
            let ib = crate::core::viewport::image_rect(dw, dh, bw, bh, zoom, pan_offset);
            let to_screen = |u: f32, v: f32| {
                let vu = (u - crop[0]) / crop[2].max(1e-6);
                let vv = (v - crop[1]) / crop[3].max(1e-6);
                Point::new(ib.x + vu * ib.width, ib.y + vv * ib.height)
            };
            let center = to_screen(m.ax, m.ay);
            let a0 = (last.y - center.y).atan2(last.x - center.x);
            let a1 = (pos.y - center.y).atan2(pos.x - center.x);
            // atan2 returns (-pi, pi], so the raw difference jumps by ~360deg
            // whenever the cursor crosses the -x ray from the center. Wrap the
            // DELTA (not just the sum) or a 0.6deg move reads as -359.4deg.
            let delta_deg = crate::core::viewport::wrap_degrees((a1 - a0).to_degrees());
            // Wrap rather than clamp: angles are cyclic, and clamping detached
            // the handle from the cursor once a drag passed +/-180deg.
            m.rotation = crate::core::viewport::wrap_degrees(m.rotation + delta_deg);
        }
    }
}

/// Decode the selected image again (`Session::reload_selected_image`).
pub fn reload_selected_image(editor: &mut RawEditor) -> Task<Message> {
    let load = editor.session.reload_selected_image();
    apply_image_load(editor, load)
}










#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::message::AppTab;
    use crate::database::models::Image as ImageData;

    fn img(id: i64, path: &str) -> ImageData {
        ImageData {
            id,
            filename: format!("{id}.NEF"),
            path: path.to_string(),
            cache_path_thumb: None,
            cache_path_instant: None,
            cache_path_working: None,
            cache_path_edited: None,
            file_status: "exists".to_string(),
            rating: 0,
            flag: 0,
        }
    }

    fn editor_with(ids: &[i64]) -> RawEditor {
        let mut e = RawEditor::new().0;
        e.session.current_tab = AppTab::Library;
        e.session.images = ids.iter().map(|i| img(*i, "/p/x.NEF")).collect();
        e
    }

    /// A move the app makes (after a delete, auto-advance) must not be read as
    /// a Shift/Ctrl click because a key from the triggering shortcut is
    /// still held; a real click still is.
    #[test]
    fn app_driven_selection_ignores_held_modifiers() {
        let mut e = editor_with(&[1, 2, 3]);
        let _ = handle_image_selected(&mut e, 1);
        e.last_modifiers = iced::keyboard::Modifiers::SHIFT;
        let _ = crate::app::update::update(&mut e, Message::ImageMoved(3));
        assert_eq!(e.session.multi_selection, [3].into_iter().collect(), "a range select");
        assert_eq!(e.session.selected_image_id, Some(3));

        e.last_modifiers = iced::keyboard::Modifiers::CTRL;
        let _ = crate::app::update::update(&mut e, Message::ImageMoved(2));
        assert_eq!(e.session.multi_selection, [2].into_iter().collect(), "toggled out");

        // A real Shift-click still extends.
        e.last_modifiers = iced::keyboard::Modifiers::SHIFT;
        let _ = crate::app::update::update(&mut e, Message::ImageSelected(3));
        assert_eq!(e.session.multi_selection.len(), 2);
    }

    #[test]
    fn range_is_inclusive_of_both_endpoints() {
        let mut e = editor_with(&[1, 2, 3, 4, 5]);
        e.selection_anchor = Some(2);
        assert_eq!(select_range(&e, 4), Some(vec![2, 3, 4]));
    }

    /// Dragging a range upward must select the same set as dragging downward.
    #[test]
    fn range_is_direction_independent() {
        let mut e = editor_with(&[1, 2, 3, 4, 5]);
        e.selection_anchor = Some(4);
        assert_eq!(select_range(&e, 2), Some(vec![2, 3, 4]));
    }

    #[test]
    fn range_of_one_is_just_that_image() {
        let mut e = editor_with(&[1, 2, 3]);
        e.selection_anchor = Some(2);
        assert_eq!(select_range(&e, 2), Some(vec![2]));
    }

    #[test]
    fn range_needs_an_anchor() {
        let e = editor_with(&[1, 2, 3]);
        assert_eq!(select_range(&e, 2), None);
    }

    /// A range must never reach images the active filter is hiding — the ids
    /// come from `displayed_ids`, so an endpoint outside it has no position.
    #[test]
    fn range_ignores_endpoints_the_filter_hides() {
        let mut e = editor_with(&[1, 2, 3]);
        e.session.images[1].rating = 0;
        e.session.min_filter_rating = 5; // hides everything
        e.selection_anchor = Some(1);
        assert_eq!(select_range(&e, 3), None);
    }
}
