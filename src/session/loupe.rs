//! The detail loupe, and holding noise reduction back on the main view.
//!
//! Every slider but three re-renders the main view in ~2–3 ms. The noise
//! sliders re-run their stage over every source pixel: ~30 ms a tick on the
//! develop preview, ~270 ms at full resolution. So, as in Lightroom's Detail
//! panel, while one of them moves the main view keeps the noise reduction it
//! had (`Session::held_noise`, applied by `view_params`), and the loupe in
//! the sidebar shows the new setting live at 1:1 (`gpu::loupe`, ~5 ms a
//! tick). The main view catches up once, when the drag pauses for
//! [`NOISE_SETTLE`] or ends.

use std::sync::Arc;
use std::time::Duration;

use super::{Effects, Session, SessionEvent};
use crate::app::message::AppTab;
use crate::core::types::EditParams;
use crate::gpu::denoise::DenoiseSettings;

/// The loupe's side, in logical pixels. It renders at the window's scale
/// factor, so it is 1:1 in physical pixels on any display.
pub const LOUPE_SIZE: f32 = 256.0;

/// How long a noise-slider drag must rest before the main view applies it.
pub const NOISE_SETTLE: Duration = Duration::from_millis(250);

/// Noise-reduction settings the main view keeps showing while a drag on one
/// of its sliders is in progress — the stage is too slow to re-run per tick.
/// Tagged with its image so it can never leak onto another one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeldNoise {
    pub image_id: Option<i64>,
    pub luma: f32,
    pub detail: f32,
    pub color: f32,
}

/// The last frame the detail loupe rendered.
#[derive(Debug, Clone)]
pub struct LoupeView {
    pub image_id: i64,
    /// `size`×`size` RGBA. A new frame is a new allocation, so a front end
    /// can tell frames apart by pointer.
    pub rgba: Arc<[u8]>,
    /// Its view-UV window — what a drag in loupe pixels moves.
    pub window: [f32; 4],
    /// Physical pixels per side.
    pub size: u32,
}

/// The Develop sidebar's 1:1 detail loupe (`gpu::loupe`).
#[derive(Debug)]
pub struct LoupeState {
    /// Where it looks, in view UV (the post-crop display image). Kept across
    /// images, like the zoom.
    pub center: [f32; 2],
    /// Its panel is open (the front end says so: a folded panel renders
    /// nothing).
    pub panel_open: bool,
    pub gpu: Option<Arc<crate::gpu::loupe::Loupe>>,
    /// The image whose loupe is being prepared (RAW read, noise model).
    pub building: Option<i64>,
    /// An image whose loupe could not be prepared, and why — not retried.
    pub failed: Option<(i64, String)>,
    pub frame: Option<LoupeView>,
    /// One render at a time; `pending` asks for another when it lands.
    pub in_flight: bool,
    pub pending: bool,
    /// The next click on the image places it.
    pub targeting: bool,
}

impl Default for LoupeState {
    fn default() -> Self {
        Self {
            center: [0.5, 0.5],
            panel_open: true,
            gpu: None,
            building: None,
            failed: None,
            frame: None,
            in_flight: false,
            pending: false,
            targeting: false,
        }
    }
}

impl Session {
    /// Physical pixels per side at the current scale factor.
    fn loupe_physical_size(&self) -> u32 {
        (LOUPE_SIZE * self.scale_factor).round().max(16.0) as u32
    }

    /// Showing, with a loaded image to show? Nothing renders otherwise.
    fn loupe_visible(&self) -> bool {
        self.current_tab == AppTab::Develop
            && self.loupe.panel_open
            && self
                .image_resources
                .as_ref()
                .is_some_and(|r| Some(r.image_id) == self.selected_image_id)
    }

    /// Open or fold the loupe's panel; opening it starts it.
    pub fn set_loupe_panel_open(&mut self, open: bool) -> Effects {
        self.loupe.panel_open = open;
        self.request_loupe()
    }

    /// Bring the loupe up to date with the current edit: prepare it for this
    /// image if needed, then render (coalesced — one render in flight).
    pub fn request_loupe(&mut self) -> Effects {
        if !self.loupe_visible() {
            return Effects::none();
        }
        let (Some(ctx), Some(main)) = (self.gpu_context.clone(), self.image_resources.clone()) else {
            return Effects::none();
        };
        let loupe = match &self.loupe.gpu {
            Some(l) if l.matches(&main) => l.clone(),
            _ => return self.build_loupe(ctx, main),
        };
        if self.loupe.in_flight {
            self.loupe.pending = true;
            return Effects::none();
        }
        self.loupe.in_flight = true;
        self.loupe.pending = false;

        // Live noise settings — the point of the loupe — except while peeking at
        // the original, when it shows what the main view shows.
        let nr_source = if self.before_peek { main.pushed_params() } else { self.current_edit_params };
        let nr = DenoiseSettings::from_params(&nr_source);
        let center = self.loupe.center;
        let size = self.loupe_physical_size();
        let image_id = main.image_id;
        Effects::perform("loupe render", async move {
            SessionEvent::LoupeRendered(image_id, loupe.render(&ctx, &main, center, size, nr, None).await)
        })
    }

    /// Prepare the loupe for `main`'s image: its full-resolution mosaic (cached,
    /// or read again) and sensor noise model. Off the UI thread.
    fn build_loupe(
        &mut self,
        ctx: Arc<crate::gpu::shared::SharedContext>,
        main: Arc<crate::gpu::shared::ImageResources>,
    ) -> Effects {
        let image_id = main.image_id;
        if self.loupe.building == Some(image_id)
            || self.loupe.failed.as_ref().is_some_and(|(id, _)| *id == image_id)
        {
            return Effects::none();
        }
        // The previous image's loupe holds its full mosaic: let it go now.
        self.loupe.gpu = None;
        self.loupe.building = Some(image_id);
        let cached = self.raw_for(image_id);
        let path = self.images.iter().find(|i| i.id == image_id).map(|i| i.path.clone());
        Effects::perform("loupe build", async move {
            let result = async move {
                let raw = match (cached, path) {
                    (Some(raw), _) => raw,
                    (None, Some(path)) => Arc::new(crate::raw::loader::load_raw_data(path).await?),
                    (None, None) => return Err("image is no longer in the catalogue".to_string()),
                };
                tokio::task::spawn_blocking(move || {
                    // The main view's mosaic carries any AI denoise; so must this.
                    let raw = match crate::ml::denoise::effective_mosaic(&raw, image_id, main.ai_denoise.1) {
                        (std::borrow::Cow::Owned(data), _) => {
                            Arc::new(crate::raw::loader::RawDataResult { data, ..(*raw).clone() })
                        }
                        (std::borrow::Cow::Borrowed(_), _) => raw,
                    };
                    crate::gpu::loupe::Loupe::new(&ctx, &main, raw).map(Arc::new)
                })
                .await
                .map_err(|e| format!("loupe preparation panicked: {e}"))?
            }
            .await;
            SessionEvent::LoupeReady(image_id, result)
        })
    }

    pub fn loupe_ready(&mut self, image_id: i64, result: Result<Arc<crate::gpu::loupe::Loupe>, String>) -> Effects {
        if self.loupe.building == Some(image_id) {
            self.loupe.building = None;
        }
        if self.selected_image_id != Some(image_id) {
            return Effects::none();
        }
        match result {
            Ok(loupe) => {
                self.loupe.gpu = Some(loupe);
                self.request_loupe()
            }
            Err(e) => {
                tracing::error!("Detail loupe unavailable for image {image_id}: {e}");
                self.loupe.failed = Some((image_id, e));
                Effects::none()
            }
        }
    }

    pub fn loupe_rendered(&mut self, image_id: i64, result: Result<crate::gpu::loupe::LoupeFrame, String>) -> Effects {
        self.loupe.in_flight = false;
        match result {
            Ok(frame) if self.selected_image_id == Some(image_id) => {
                self.loupe.frame = Some(LoupeView {
                    image_id,
                    rgba: Arc::from(frame.rgba),
                    window: frame.window,
                    size: frame.size,
                });
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("Detail loupe render failed: {e}"),
        }
        if self.loupe.pending {
            self.request_loupe()
        } else {
            Effects::none()
        }
    }

    /// Re-render only if what is shown is not this image's, at this size — for
    /// paths that change the loaded image or the display without pushing params
    /// (image load, full-resolution upgrade, DPI change).
    pub fn refresh_loupe_if_stale(&mut self) -> Effects {
        let size = self.loupe_physical_size();
        let current = self
            .loupe
            .frame
            .as_ref()
            .is_some_and(|f| Some(f.image_id) == self.selected_image_id && f.size == size);
        if current {
            Effects::none()
        } else {
            self.request_loupe()
        }
    }

    /// A drag inside the loupe, in logical pixels: move the content with the
    /// pointer, like sliding a print under a magnifier.
    pub fn pan_loupe(&mut self, dx: f32, dy: f32) -> Effects {
        let Some(frame) = &self.loupe.frame else { return Effects::none() };
        let per_px = [frame.window[2] / frame.size as f32, frame.window[3] / frame.size as f32];
        let half = [frame.window[2] / 2.0, frame.window[3] / 2.0];
        let scale = self.scale_factor;
        let c = &mut self.loupe.center;
        // Clamped to where the window can actually go, so dragging past the
        // image's edge doesn't wind up distance that must be undone first.
        c[0] = (c[0] - dx * scale * per_px[0]).clamp(half[0], 1.0 - half[0]);
        c[1] = (c[1] - dy * scale * per_px[1]).clamp(half[1], 1.0 - half[1]);
        self.request_loupe()
    }

    /// Arm or disarm placing the loupe with a click (Develop only).
    pub fn toggle_loupe_target(&mut self) {
        if self.current_tab != AppTab::Develop {
            return;
        }
        self.loupe.targeting = !self.loupe.targeting;
        if self.loupe.targeting {
            // Tools are mutually exclusive: a click can only mean one thing.
            self.is_wb_picking = false;
            self.is_leveling = false;
            self.mask_tool = crate::app::state::MaskTool::Inactive;
            self.brush_armed = false;
            self.color_picking_mask = None;
            self.status = "Click the image to place the detail loupe".to_string();
        } else {
            self.status.clear();
        }
    }

    pub fn loupe_target_picked(&mut self, u: f32, v: f32) -> Effects {
        self.loupe.targeting = false;
        self.status.clear();
        self.loupe.center = [u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)];
        self.request_loupe()
    }

    /// One tick of a noise-reduction slider. The edit changes now (and is saved
    /// at release like any other); the main view keeps its current noise
    /// reduction until the drag settles, and the loupe shows the new value.
    pub fn noise_changed(&mut self, apply: impl FnOnce(&mut EditParams)) -> Effects {
        if self.held_noise.is_none() {
            // What the main view shows right now is what it keeps.
            let shown = self.view_params();
            self.held_noise = Some(HeldNoise {
                image_id: self.selected_image_id,
                luma: shown.luma_noise,
                detail: shown.luma_detail,
                color: shown.color_noise,
            });
        }
        apply(&mut self.current_edit_params);
        self.stage_drag = true;
        self.mark_edits_dirty();
        self.sync_tool_flags();

        self.noise_settle_token = self.noise_settle_token.wrapping_add(1);
        let token = self.noise_settle_token;
        // An async block, so the timer is created on the executor's runtime
        // when polled rather than here on the calling thread.
        let settle = Effects::perform("noise settle", async move {
            tokio::time::sleep(NOISE_SETTLE).await;
            SessionEvent::NoiseSettled(token)
        });
        Effects::batch([self.request_loupe(), settle])
    }

    pub fn noise_settled(&mut self, token: u64) -> Effects {
        if token != self.noise_settle_token {
            return Effects::none();
        }
        self.release_noise()
    }

    /// Let the main view catch up with the noise sliders. `None` → no-op.
    pub fn release_noise(&mut self) -> Effects {
        if self.held_noise.take().is_none() {
            return Effects::none();
        }
        let params = self.view_params();
        self.push_uniforms_and_render(&params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let mut s = Session::default();
        s.selected_image_id = Some(7);
        s
    }

    /// The main view keeps the noise reduction it had for the whole drag;
    /// the edit itself (saved, exported, undone) moves with the slider.
    #[test]
    fn a_noise_drag_holds_the_main_view_and_not_the_edit() {
        let mut s = session();
        s.current_edit_params.luma_noise = 0.2;
        let _ = s.noise_changed(|p| p.luma_noise = 0.5);
        let _ = s.noise_changed(|p| p.luma_noise = 0.7);
        assert_eq!(s.current_edit_params.luma_noise, 0.7);
        assert!(s.edits_dirty, "the drag would not be saved");
        assert_eq!(s.view_params().luma_noise, 0.2, "held the value of a later tick");
        // Everything else renders as edited.
        s.current_edit_params.exposure = 1.0;
        assert_eq!(s.view_params().exposure, 1.0);
    }

    /// Only the latest tick's timer releases it: a drag that keeps moving
    /// keeps holding.
    #[test]
    fn only_the_latest_settle_timer_releases_the_hold() {
        let mut s = session();
        let _ = s.noise_changed(|p| p.color_noise = 0.3);
        let stale = s.noise_settle_token;
        let _ = s.noise_changed(|p| p.color_noise = 0.4);
        let _ = s.noise_settled(stale);
        assert!(s.held_noise.is_some(), "an old tick's timer released a moving drag");
        let latest = s.noise_settle_token;
        let _ = s.noise_settled(latest);
        assert!(s.held_noise.is_none());
        assert_eq!(s.view_params().color_noise, 0.4);
    }

    /// The slider's release applies at once, without waiting for the timer.
    #[test]
    fn releasing_the_slider_applies_immediately() {
        let mut s = session();
        let _ = s.noise_changed(|p| p.luma_detail = 0.9);
        let _ = s.commit_edit();
        assert!(s.held_noise.is_none());
        assert_eq!(s.view_params().luma_detail, 0.9);
    }

    /// A hold belongs to its image: one left over can't restyle the next.
    #[test]
    fn a_hold_never_applies_to_another_image() {
        let mut s = session();
        s.current_edit_params.luma_noise = 0.1;
        let _ = s.noise_changed(|p| p.luma_noise = 0.6);
        s.selected_image_id = Some(8);
        assert_eq!(s.view_params().luma_noise, 0.6);
    }

    /// Dragging the loupe moves its content with the pointer: right shows
    /// more of the left, and it stops at the image's edge.
    #[test]
    fn panning_follows_the_pointer_and_stops_at_the_edge() {
        let mut s = session();
        s.scale_factor = 1.0;
        s.loupe.frame = Some(LoupeView { image_id: 7, rgba: Arc::from(vec![0u8; 4]), window: [0.45, 0.45, 0.1, 0.1], size: 256 });
        let _ = s.pan_loupe(25.6, 0.0);
        assert!((s.loupe.center[0] - 0.49).abs() < 1e-5, "{:?}", s.loupe.center);
        let _ = s.pan_loupe(1e6, -1e6);
        assert_eq!(s.loupe.center, [0.05, 0.95]);
    }
}
