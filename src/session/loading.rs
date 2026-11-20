//! Loading an image for Develop: RAW decodes (direct and preloaded), the
//! GPU resources built from them, and the full-resolution upgrade.

use std::sync::Arc;

use super::{Effects, Session, SessionEvent};
use crate::app::state::EditorReadiness;
use crate::gpu;
use crate::raw;

/// A finished resources build: the shared context and the image's resources.
pub type BuiltResources = Result<(Arc<gpu::shared::SharedContext>, Arc<gpu::shared::ImageResources>), String>;

/// Whether a build's resources were installed as the open image's.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    /// Stale or failed (the status says why): nothing changed on screen.
    No,
    /// New resources are in place: the open image wants rendering (and the
    /// placeholder can go).
    Yes,
}

/// What a finished full-resolution rebuild means.
#[must_use]
pub enum FullRes {
    /// It was this image's: as for any build.
    Installed(Installed),
    /// Not installed; this is what follows (perhaps an upgrade of the image
    /// now open).
    Effects(Effects),
}

/// Run a resources build and report it as an event.
fn perform_build(
    label: &'static str,
    build: impl std::future::Future<Output = BuiltResources> + Send + 'static,
    done: impl FnOnce(BuiltResources) -> SessionEvent + Send + 'static,
) -> Effects {
    Effects::perform(label, async move { done(build.await) })
}

/// The one `SharedContext`. Every path that needs a context goes through
/// here: tasks started before the first finishes (select 1 → 2 → 1 during the
/// first load, or an export beside a load) all get the same device, so
/// resources built by one can be installed beside another's context.
pub async fn shared_context(
    cell: &tokio::sync::OnceCell<Arc<gpu::shared::SharedContext>>,
    existing: Option<Arc<gpu::shared::SharedContext>>,
) -> Result<Arc<gpu::shared::SharedContext>, String> {
    cell.get_or_try_init(|| async {
        match existing {
            Some(c) => Ok(c),
            None => gpu::shared::SharedContext::new().await.map(Arc::new),
        }
    })
    .await
    .cloned()
}

fn metadata_snapshot(raw: &raw::loader::RawDataResult) -> raw::loader::RawDataResult {
    raw::loader::RawDataResult {
        data: Vec::new(),
        dcp_profile: raw.dcp_profile.clone(),
        width: raw.width,
        height: raw.height,
        wb_multipliers: raw.wb_multipliers,
        color_matrix: raw.color_matrix,
        color_matrix_is_d65: raw.color_matrix_is_d65,
        cfa_pattern: raw.cfa_pattern,
        black_levels: raw.black_levels,
        white_level: raw.white_level,
        crops: raw.crops,
        cfa_name: raw.cfa_name.clone(),
        measured_black_levels: raw.measured_black_levels,
        make: raw.make.clone(),
        model: raw.model.clone(),
        iso: raw.iso.clone(),
        shutter_speed: raw.shutter_speed.clone(),
        aperture: raw.aperture.clone(),
        lens: raw.lens.clone(),
        lens_context: raw.lens_context.clone(),
        orientation: raw.orientation,
    }
}

impl Session {
    pub fn raw_data_loaded(&mut self, image_id: i64, result: Result<raw::loader::RawDataResult, String>) -> Effects {
        self.direct_raw_loads.remove(&image_id);
        match result {
            Ok(raw) => {
                let raw = Arc::new(raw);
                self.insert_raw_cache(image_id, raw.clone());
                self.apply_raw_preload_cleanup(image_id);

                if self.awaiting_raw(image_id) {
                    self.prepare_image_resources_from_raw(image_id, raw)
                } else {
                    Effects::none()
                }
            }
            Err(e) => { 
                if self.selected_image_id == Some(image_id) {
                    self.status = format!("Failed to load RAW: {}", e);
                    self.editor_readiness = EditorReadiness::Failed(image_id, e);
                }
                Effects::none()
            }
        }
    }

    pub fn raw_preloaded(&mut self, image_id: i64, result: Result<Arc<raw::loader::RawDataResult>, String>) -> Effects {
        self.apply_raw_preload_cleanup(image_id);

        match result {
            Ok(raw) => {
                self.insert_raw_cache(image_id, raw.clone());
                if self.awaiting_raw(image_id) {
                    return self.prepare_image_resources_from_raw(image_id, raw);
                }
                Effects::none()
            }
            Err(e) => {
                tracing::warn!("RAW preload failed for image {}: {}", image_id, e);
                // Develop left the selection to this preload (it was already
                // running when selected): its failure is the load's failure, as
                // for a direct decode. Only logging it left `Loading` with nothing
                // in flight. `Failed` makes a re-select decode it again.
                if self.awaiting_raw(image_id) {
                    self.status = format!("Failed to load RAW: {}", e);
                    self.editor_readiness = EditorReadiness::Failed(image_id, e);
                }
                Effects::none()
            }
        }
    }

    /// True when Develop is waiting on exactly this RAW to finish loading.
    ///
    /// Deliberately NOT gated on the current tab. `Loading(id)` is a promise that
    /// a load is in flight; if the user stepped out to Library while it decoded
    /// and the result were dropped, coming back would find "still loading", start
    /// nothing, and wait forever.
    fn awaiting_raw(&self, image_id: i64) -> bool {
        self.selected_image_id == Some(image_id)
            && matches!(self.editor_readiness, EditorReadiness::Loading(id) if id == image_id)
    }

    pub fn prepare_image_resources_from_raw(&mut self, image_id: i64, raw: Arc<raw::loader::RawDataResult>) -> Effects {
        self.current_raw = Some((image_id, raw.clone()));
        self.current_metadata = Some(metadata_snapshot(&raw));
        self.current_metadata_id = Some(image_id);

        // Phase 140: Store current DCP profile
        self.current_dcp_profile = raw.dcp_profile.clone();
        // The memo is keyed only on (kelvin, profile_curve), so a new profile at
        // the same slider values would otherwise return the previous image's LUT.
        self.dcp_memo.replace(None);

        // As-shot WB anchor: solve the camera multipliers to (Kelvin, tint) so the
        // Temp/Tint sliders can display and pivot around the real camera values.
        let (as_kelvin, as_tint) = crate::color::as_shot_kelvin_tint(&raw);
        self.as_shot_wb = Some((as_kelvin, as_tint));
        tracing::info!("As-shot WB: {:.0}K, tint {:+.1}", as_kelvin, as_tint);

        let params = self.current_edit_params;
        let profile_defaults = self.camera_profile_defaults.clone();
        let context = self.gpu_context.clone();
        let gpu_init = self.gpu_init.clone();

        perform_build("develop resources", async move {
                let ctx = shared_context(&gpu_init, context).await?;
                // CPU-heavy (profile scan, mosaic blend, noise/haze analysis,
                // texture uploads): on the blocking pool, not a tokio worker, so
                // renders' `block_in_place` always finds a free worker.
                tokio::task::spawn_blocking(move || {

                // The image's camera profile: its own choice, else its camera's
                // default, else the best installed match. Resolved here, off the
                // UI thread (it may scan and parse profile files).
                let profile_use = crate::core::camera_profiles::ProfileUse::resolve(
                    &raw,
                    params.camera_profile,
                    &profile_defaults,
                );

                // Anchored kelvin for the DCP interpolation. ImageResources::new
                // receives the AS-SHOT multipliers (the "None → as-shot" baseline
                // for update_uniforms); handle_image_resources_ready immediately
                // applies any saved Temp/Tint edits via the WB override.
                let as_shot = profile_use.as_shot;
                let kelvin = params.kelvin_from_anchor(as_shot.0);

                let (forward_matrix, interpolated_dcp) = if let Some(dcp) = &profile_use.profile {
                    let interpolated = crate::raw::dcp::interpolate_at_temperature(dcp, kelvin, crate::core::tonemap::ToneSettings::from_params(&params));
                    (interpolated.camera_to_prophoto, Some(interpolated))
                } else {
                    let xyz_to_cam = raw.color_matrix;
                    let cam_to_srgb = crate::color::calculate_cam_to_srgb(xyz_to_cam, raw.wb_multipliers, raw.color_matrix_is_d65);
                    (cam_to_srgb, None)
                };

                // AI denoise rides in on the mosaic itself (`ml::denoise`).
                let (mosaic, ai_applied) = crate::ml::denoise::effective_mosaic(&raw, image_id, params.ai_denoise);
                match gpu::shared::ImageResources::new(
                    &ctx,
                    image_id,
                    &mosaic,
                    raw.width,
                    raw.height,
                    &params,
                    raw.wb_multipliers,
                    forward_matrix,
                    raw.cfa_pattern,
                    raw.black_levels,
                    raw.white_level,
                    interpolated_dcp.as_ref(),
                    Some(1280), // develop preview: subsample for fast load
                    raw.orientation,
                    raw.lens_context.clone(),
                ) {
                    Ok(mut resources) => {
                        resources.profile_use = Some(profile_use);
                        resources.ai_denoise = (params.ai_denoise, ai_applied);
                        Ok((ctx, std::sync::Arc::new(resources)))
                    }
                    Err(e) => Err(e),
                }
                })
                .await
                .map_err(|e| format!("resource build panicked: {e}"))?
            },
            move |res| SessionEvent::ImageResourcesReady(image_id, res),
        )
    }

    pub fn image_resources_ready(&mut self, image_id: i64, result: BuiltResources) -> Installed {
        // Ignore stale results from a previous image selection
        if self.selected_image_id != Some(image_id) {
            return Installed::No;
        }

        match result {
            Ok((context, resources)) => {
                if self.gpu_context.is_none() {
                    self.gpu_context = Some(context.clone());
                }
                // The profile these resources were built with is the image's
                // profile from here on — the WB solve below already reads it.
                if let Some(used) = &resources.profile_use {
                    if self.current_dcp_profile.as_ref().map(Arc::as_ptr)
                        != used.profile.as_ref().map(Arc::as_ptr)
                    {
                        self.dcp_memo.replace(None);
                    }
                    self.current_dcp_profile = used.profile.clone();
                    self.as_shot_wb = Some(used.as_shot);
                    if let Some(meta) = self.current_metadata.as_mut() {
                        meta.dcp_profile = used.profile.clone();
                    }
                }

                self.sync_tool_flags();
                if let Some(ctx) = &self.gpu_context {
                    let params = self.view_params();
                    let (interpolated, wb_override) = self.resolve_wb_and_dcp(&params);
                    resources.update_uniforms(ctx, &params, interpolated.as_deref(), wb_override);
                }

                // A full-resolution upgrade keeps the preview it replaces: renders
                // that full resolution would not sharpen use it instead
                // (`RawEditor::preview_resources`). Anything else drops it.
                // AI masks: the new resources start with no bitmaps.
                if self.ai_bitmaps.image_id == Some(image_id) {
                    resources.set_ai_bitmaps(self.ai_bitmaps.bitmaps.clone());
                }
                let upgrade = resources.width == resources.original_width;
                self.preview_resources = self.image_resources.take().filter(|old| {
                    upgrade && old.image_id == image_id && old.width < old.original_width
                });
                self.image_resources = Some(resources);
                self.editor_readiness = EditorReadiness::Ready(image_id);
                // The render replaces the placeholder; overlays and histogram
                // were drawn for the old resources.
                self.camera_preview = Some(super::PreviewHandle::Clear);
                self.changed.insert(super::Changed::OVERLAY | super::Changed::HISTOGRAM);
                Installed::Yes
            }
            Err(e) => {
                self.status = format!("GPU Init Failed: {}", e);
                self.editor_readiness = EditorReadiness::Failed(image_id, e);
                Installed::No
            }
        }
    }

    /// The full-resolution rebuild finished: release the one-at-a-time guard,
    /// then install it — or, if the user has moved on, see whether the image
    /// they are on now wants one (its own render may have asked while this was
    /// building and been turned away by the guard).
    pub fn full_res_ready(&mut self, image_id: i64, result: BuiltResources) -> FullRes {
        self.full_res_upgrading = false;
        if self.selected_image_id != Some(image_id) {
            return FullRes::Effects(self.upgrade_if_soft());
        }
        match result {
            Ok(_) => FullRes::Installed(self.image_resources_ready(image_id, result)),
            Err(e) => {
                // The preview resources still work: keep editing on them.
                tracing::warn!("Full-resolution rebuild of image {image_id} failed, keeping the preview: {e}");
                self.full_res_failed = Some(image_id);
                FullRes::Effects(Effects::none())
            }
        }
    }

    /// Start the full-resolution rebuild if the current view shows the
    /// subsampled develop texture magnified (`develop::render_target`).
    fn upgrade_if_soft(&mut self) -> Effects {
        if self.current_tab != crate::app::message::AppTab::Develop
            || !matches!(self.editor_readiness, EditorReadiness::Ready(_))
            || self.render_target().magnification
                <= crate::core::viewport::FULL_RES_MAGNIFICATION
        {
            return Effects::none();
        }
        self.trigger_full_res_upgrade()
    }

    /// Rebuild the current image's ImageResources at full resolution, replacing
    /// the subsampled develop preview. Requested by any render whose view shows
    /// the preview magnified (`Session::render`). The RAW comes from
    /// `raw_cache`, or from disk if the cache no longer holds it — skipping the
    /// upgrade there left a zoomed-in view soft with no way to sharpen it.
    pub fn trigger_full_res_upgrade(&mut self) -> Effects {
        let Some(image_id) = self.selected_image_id else { return Effects::none(); };

        // Only upgrade if currently subsampled
        if let Some(res) = &self.image_resources {
            if res.width == res.original_width { return Effects::none(); } // already full-res
            if res.image_id != image_id { return Effects::none(); } // still loading this one
        } else {
            return Effects::none();
        }

        if self.full_res_upgrading || self.full_res_failed == Some(image_id) {
            return Effects::none();
        }

        let cached = self.raw_for(image_id);
        let path = match &cached {
            Some(_) => None,
            None => match self.images.iter().find(|i| i.id == image_id) {
                Some(img) => Some(img.path.clone()),
                None => return Effects::none(),
            },
        };

        self.full_res_upgrading = true;
        let params = self.current_edit_params;
        let profile_use = self.image_resources.as_ref().and_then(|r| r.profile_use.clone());
        let profile_defaults = self.camera_profile_defaults.clone();
        let context = self.gpu_context.clone();
        let gpu_init = self.gpu_init.clone();

        tracing::info!(
            "Upgrading to full-res resources for image {} ({})",
            image_id,
            if cached.is_some() { "cached RAW" } else { "re-reading RAW" }
        );

        perform_build("full-resolution resources", async move {
                let raw = match (cached, path) {
                    (Some(raw), _) => raw,
                    (None, Some(path)) => Arc::new(raw::loader::load_raw_data(path).await?),
                    (None, None) => return Err("no RAW to upgrade from".to_string()),
                };
                let ctx = shared_context(&gpu_init, context).await?;
                // CPU-heavy (profile scan, mosaic blend, noise/haze analysis,
                // texture uploads): on the blocking pool, not a tokio worker, so
                // renders' `block_in_place` always finds a free worker.
                tokio::task::spawn_blocking(move || {

                // The profile the image already uses (resolved for its preview),
                // or — if the preview recorded none — resolved now.
                let profile_use = profile_use.unwrap_or_else(|| {
                    crate::core::camera_profiles::ProfileUse::resolve(&raw, params.camera_profile, &profile_defaults)
                });

                // Anchored kelvin; as-shot multipliers to ::new (same reasoning as
                // the preview path above).
                let as_shot = profile_use.as_shot;
                let kelvin = params.kelvin_from_anchor(as_shot.0);

                let (forward_matrix, interpolated_dcp) = if let Some(dcp) = &profile_use.profile {
                    let interpolated = crate::raw::dcp::interpolate_at_temperature(dcp, kelvin, crate::core::tonemap::ToneSettings::from_params(&params));
                    (interpolated.camera_to_prophoto, Some(interpolated))
                } else {
                    let xyz_to_cam = raw.color_matrix;
                    let cam_to_srgb = crate::color::calculate_cam_to_srgb(xyz_to_cam, raw.wb_multipliers, raw.color_matrix_is_d65);
                    (cam_to_srgb, None)
                };

                // AI denoise rides in on the mosaic itself (`ml::denoise`).
                let (mosaic, ai_applied) = crate::ml::denoise::effective_mosaic(&raw, image_id, params.ai_denoise);
                match gpu::shared::ImageResources::new(
                    &ctx,
                    image_id,
                    &mosaic,
                    raw.width,
                    raw.height,
                    &params,
                    raw.wb_multipliers,
                    forward_matrix,
                    raw.cfa_pattern,
                    raw.black_levels,
                    raw.white_level,
                    interpolated_dcp.as_ref(),
                    None, // full resolution
                    raw.orientation,
                    raw.lens_context.clone(),
                ) {
                    Ok(mut resources) => {
                        resources.profile_use = Some(profile_use);
                        resources.ai_denoise = (params.ai_denoise, ai_applied);
                        Ok((ctx, Arc::new(resources)))
                    }
                    Err(e) => Err(e),
                }
                })
                .await
                .map_err(|e| format!("resource build panicked: {e}"))?
            },
            move |res| SessionEvent::FullResReady(image_id, res),
        )
    }

    fn apply_raw_preload_cleanup(&mut self, image_id: i64) {
        self.pending_raw_loads.remove(&image_id);
        self.queued_raw_loads.retain(|(id, _)| *id != image_id);
    }
}
