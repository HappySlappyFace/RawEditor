//! The detail loupe, and holding noise reduction back on the main view.
//!
//! Every slider but three re-renders the main view in ~2–3 ms. The noise
//! sliders re-run their stage over every source pixel: ~30 ms a tick on the
//! develop preview, ~270 ms at full resolution. So, as in Lightroom's Detail
//! panel, while one of them moves the main view keeps the noise reduction it
//! had (`RawEditor::held_noise`, applied by `view_params`), and the loupe in
//! the sidebar shows the new setting live at 1:1 (`gpu::loupe`, ~5 ms a
//! tick). The main view catches up once, when the drag pauses for
//! [`NOISE_SETTLE`] or ends.

use std::sync::Arc;
use std::time::Duration;

use iced::Task;

use crate::app::message::{AppTab, Message, SidebarSection};
use crate::app::state::{HeldNoise, LoupeView, RawEditor};
use crate::core::types::EditParams;
use crate::gpu::denoise::DenoiseSettings;

/// The loupe's side, in logical pixels. It renders at the window's scale
/// factor, so it is 1:1 in physical pixels on any display.
pub const LOUPE_SIZE: f32 = 256.0;

/// How long a noise-slider drag must rest before the main view applies it.
pub const NOISE_SETTLE: Duration = Duration::from_millis(250);

/// Physical pixels per side at the current scale factor.
fn physical_size(editor: &RawEditor) -> u32 {
    (LOUPE_SIZE * editor.scale_factor).round().max(16.0) as u32
}

/// Showing, with a loaded image to show? Nothing renders otherwise.
fn visible(editor: &RawEditor) -> bool {
    editor.current_tab == AppTab::Develop
        && !editor.collapsed_sections.contains(&SidebarSection::DetailPreview)
        && editor
            .image_resources
            .as_ref()
            .is_some_and(|r| Some(r.image_id) == editor.selected_image_id)
}

/// Bring the loupe up to date with the current edit: prepare it for this
/// image if needed, then render (coalesced — one render in flight).
pub fn request(editor: &mut RawEditor) -> Task<Message> {
    if !visible(editor) {
        return Task::none();
    }
    let (Some(ctx), Some(main)) = (editor.gpu_context.clone(), editor.image_resources.clone()) else {
        return Task::none();
    };
    let loupe = match &editor.loupe.gpu {
        Some(l) if l.matches(&main) => l.clone(),
        _ => return build(editor, ctx, main),
    };
    if editor.loupe.in_flight {
        editor.loupe.pending = true;
        return Task::none();
    }
    editor.loupe.in_flight = true;
    editor.loupe.pending = false;

    // Live noise settings — the point of the loupe — except while peeking at
    // the original, when it shows what the main view shows.
    let nr_source = if editor.before_peek { main.pushed_params() } else { editor.current_edit_params };
    let nr = DenoiseSettings::from_params(&nr_source);
    let center = editor.loupe.center;
    let size = physical_size(editor);
    let image_id = main.image_id;
    Task::perform(
        async move { loupe.render(&ctx, &main, center, size, nr).await },
        move |result| Message::LoupeRendered(image_id, result),
    )
}

/// Prepare the loupe for `main`'s image: its full-resolution mosaic (cached,
/// or read again) and sensor noise model. Off the UI thread.
fn build(
    editor: &mut RawEditor,
    ctx: Arc<crate::gpu::shared::SharedContext>,
    main: Arc<crate::gpu::shared::ImageResources>,
) -> Task<Message> {
    let image_id = main.image_id;
    if editor.loupe.building == Some(image_id)
        || editor.loupe.failed.as_ref().is_some_and(|(id, _)| *id == image_id)
    {
        return Task::none();
    }
    // The previous image's loupe holds its full mosaic: let it go now.
    editor.loupe.gpu = None;
    editor.loupe.building = Some(image_id);
    let cached = editor.raw_cache.peek(&image_id).cloned();
    let path = editor.images.iter().find(|i| i.id == image_id).map(|i| i.path.clone());
    Task::perform(
        async move {
            let raw = match (cached, path) {
                (Some(raw), _) => raw,
                (None, Some(path)) => Arc::new(crate::raw::loader::load_raw_data(path).await?),
                (None, None) => return Err("image is no longer in the catalogue".to_string()),
            };
            tokio::task::spawn_blocking(move || crate::gpu::loupe::Loupe::new(&ctx, &main, raw).map(Arc::new))
                .await
                .map_err(|e| format!("loupe preparation panicked: {e}"))?
        },
        move |result| Message::LoupeReady(image_id, result),
    )
}

pub fn handle_ready(
    editor: &mut RawEditor,
    image_id: i64,
    result: Result<Arc<crate::gpu::loupe::Loupe>, String>,
) -> Task<Message> {
    if editor.loupe.building == Some(image_id) {
        editor.loupe.building = None;
    }
    if editor.selected_image_id != Some(image_id) {
        return Task::none();
    }
    match result {
        Ok(loupe) => {
            editor.loupe.gpu = Some(loupe);
            request(editor)
        }
        Err(e) => {
            tracing::error!("Detail loupe unavailable for image {image_id}: {e}");
            editor.loupe.failed = Some((image_id, e));
            Task::none()
        }
    }
}

pub fn handle_rendered(
    editor: &mut RawEditor,
    image_id: i64,
    result: Result<crate::gpu::loupe::LoupeFrame, String>,
) -> Task<Message> {
    editor.loupe.in_flight = false;
    match result {
        Ok(frame) if editor.selected_image_id == Some(image_id) => {
            editor.loupe.frame = Some(LoupeView {
                image_id,
                handle: iced::widget::image::Handle::from_rgba(frame.size, frame.size, frame.rgba),
                window: frame.window,
                size: frame.size,
            });
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("Detail loupe render failed: {e}"),
    }
    if editor.loupe.pending {
        request(editor)
    } else {
        Task::none()
    }
}

/// Re-render only if what is shown is not this image's, at this size — for
/// paths that change the loaded image or the display without pushing params
/// (image load, full-resolution upgrade, DPI change).
pub fn refresh_if_stale(editor: &mut RawEditor) -> Task<Message> {
    let size = physical_size(editor);
    let current = editor
        .loupe
        .frame
        .as_ref()
        .is_some_and(|f| Some(f.image_id) == editor.selected_image_id && f.size == size);
    if current {
        Task::none()
    } else {
        request(editor)
    }
}

/// A drag inside the loupe, in logical pixels: move the content with the
/// pointer, like sliding a print under a magnifier.
pub fn handle_pan(editor: &mut RawEditor, dx: f32, dy: f32) -> Task<Message> {
    let Some(frame) = &editor.loupe.frame else { return Task::none() };
    let per_px = [frame.window[2] / frame.size as f32, frame.window[3] / frame.size as f32];
    let half = [frame.window[2] / 2.0, frame.window[3] / 2.0];
    let c = &mut editor.loupe.center;
    // Clamped to where the window can actually go, so dragging past the
    // image's edge doesn't wind up distance that must be undone first.
    c[0] = (c[0] - dx * editor.scale_factor * per_px[0]).clamp(half[0], 1.0 - half[0]);
    c[1] = (c[1] - dy * editor.scale_factor * per_px[1]).clamp(half[1], 1.0 - half[1]);
    request(editor)
}

pub fn handle_toggle_target(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != crate::app::state::Modal::None || editor.current_tab != AppTab::Develop {
        return Task::none();
    }
    editor.loupe.targeting = !editor.loupe.targeting;
    if editor.loupe.targeting {
        // Tools are mutually exclusive: a click can only mean one thing.
        editor.is_wb_picking = false;
        editor.is_leveling = false;
        editor.mask_tool = crate::app::state::MaskTool::Inactive;
        editor.brush_armed = false;
        editor.color_picking_mask = None;
        editor.status = "Click the image to place the detail loupe".to_string();
    } else {
        editor.status.clear();
    }
    Task::none()
}

pub fn handle_target_picked(editor: &mut RawEditor, u: f32, v: f32) -> Task<Message> {
    editor.loupe.targeting = false;
    editor.status.clear();
    editor.loupe.center = [u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)];
    request(editor)
}

/// One tick of a noise-reduction slider. The edit changes now (and is saved
/// at release like any other); the main view keeps its current noise
/// reduction until the drag settles, and the loupe shows the new value.
pub fn handle_noise_changed(editor: &mut RawEditor, apply: impl FnOnce(&mut EditParams)) -> Task<Message> {
    if editor.held_noise.is_none() {
        // What the main view shows right now is what it keeps.
        let shown = editor.view_params();
        editor.held_noise = Some(HeldNoise {
            image_id: editor.selected_image_id,
            luma: shown.luma_noise,
            detail: shown.luma_detail,
            color: shown.color_noise,
        });
    }
    apply(&mut editor.current_edit_params);
    editor.stage_drag = true;
    editor.mark_edits_dirty();
    editor.sync_tool_flags();

    editor.noise_settle_token = editor.noise_settle_token.wrapping_add(1);
    let token = editor.noise_settle_token;
    // An async block, so the timer is created on the executor's runtime
    // when polled rather than here on the update thread.
    let settle = Task::perform(async { tokio::time::sleep(NOISE_SETTLE).await }, move |_| {
        Message::NoiseSettled(token)
    });
    Task::batch([request(editor), settle])
}

pub fn handle_noise_settled(editor: &mut RawEditor, token: u64) -> Task<Message> {
    if token != editor.noise_settle_token {
        return Task::none();
    }
    release_noise(editor)
}

/// Let the main view catch up with the noise sliders. `None` → no-op.
pub fn release_noise(editor: &mut RawEditor) -> Task<Message> {
    if editor.held_noise.take().is_none() {
        return Task::none();
    }
    let params = editor.view_params();
    crate::app::handlers::develop::push_uniforms_and_render(editor, &params)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> RawEditor {
        let (mut e, _) = RawEditor::new();
        e.selected_image_id = Some(7);
        e
    }

    /// The main view keeps the noise reduction it had for the whole drag;
    /// the edit itself (saved, exported, undone) moves with the slider.
    #[test]
    fn a_noise_drag_holds_the_main_view_and_not_the_edit() {
        let mut e = editor();
        e.current_edit_params.luma_noise = 0.2;
        let _ = handle_noise_changed(&mut e, |p| p.luma_noise = 0.5);
        let _ = handle_noise_changed(&mut e, |p| p.luma_noise = 0.7);
        assert_eq!(e.current_edit_params.luma_noise, 0.7);
        assert!(e.edits_dirty, "the drag would not be saved");
        assert_eq!(e.view_params().luma_noise, 0.2, "held the value of a later tick");
        // Everything else renders as edited.
        e.current_edit_params.exposure = 1.0;
        assert_eq!(e.view_params().exposure, 1.0);
    }

    /// Only the latest tick's timer releases it: a drag that keeps moving
    /// keeps holding.
    #[test]
    fn only_the_latest_settle_timer_releases_the_hold() {
        let mut e = editor();
        let _ = handle_noise_changed(&mut e, |p| p.color_noise = 0.3);
        let stale = e.noise_settle_token;
        let _ = handle_noise_changed(&mut e, |p| p.color_noise = 0.4);
        let _ = handle_noise_settled(&mut e, stale);
        assert!(e.held_noise.is_some(), "an old tick's timer released a moving drag");
        let latest = e.noise_settle_token;
        let _ = handle_noise_settled(&mut e, latest);
        assert!(e.held_noise.is_none());
        assert_eq!(e.view_params().color_noise, 0.4);
    }

    /// The slider's release applies at once, without waiting for the timer.
    #[test]
    fn releasing_the_slider_applies_immediately() {
        let mut e = editor();
        let _ = handle_noise_changed(&mut e, |p| p.luma_detail = 0.9);
        let _ = crate::app::handlers::develop::handle_commit_edit(&mut e);
        assert!(e.held_noise.is_none());
        assert_eq!(e.view_params().luma_detail, 0.9);
    }

    /// A hold belongs to its image: one left over can't restyle the next.
    #[test]
    fn a_hold_never_applies_to_another_image() {
        let mut e = editor();
        e.current_edit_params.luma_noise = 0.1;
        let _ = handle_noise_changed(&mut e, |p| p.luma_noise = 0.6);
        e.selected_image_id = Some(8);
        assert_eq!(e.view_params().luma_noise, 0.6);
    }

    /// Dragging the loupe moves its content with the pointer: right shows
    /// more of the left, and it stops at the image's edge.
    #[test]
    fn panning_follows_the_pointer_and_stops_at_the_edge() {
        let mut e = editor();
        e.scale_factor = 1.0;
        e.loupe.frame = Some(LoupeView {
            image_id: 7,
            handle: iced::widget::image::Handle::from_rgba(1, 1, vec![0u8; 4]),
            window: [0.45, 0.45, 0.1, 0.1],
            size: 256,
        });
        let _ = handle_pan(&mut e, 25.6, 0.0);
        assert!((e.loupe.center[0] - 0.49).abs() < 1e-5, "{:?}", e.loupe.center);
        let _ = handle_pan(&mut e, 1e6, -1e6);
        assert_eq!(e.loupe.center, [0.05, 0.95]);
    }
}
