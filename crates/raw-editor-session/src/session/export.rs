//! Export: the batch queue, the memory gate, building each image's
//! full-resolution resources, rendering and encoding. One image at a time,
//! each chained to the next by `SessionEvent::ProcessNextExport`. (The
//! dialog and the folder picker are the app's.)

use super::{Effects, Session, SessionEvent};
use crate::gpu;
use crate::raw;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Phase 89: Export Format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Jpeg,
    Png,
    Tiff,
}

impl std::fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportFormat::Jpeg => write!(f, "JPEG"),
            ExportFormat::Png => write!(f, "PNG"),
            ExportFormat::Tiff => write!(f, "TIFF"),
        }
    }
}

/// Per-image progress within a batch, for the Export tab's queue list.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportJobState {
    Pending,
    Working,
    Done(std::path::PathBuf),
    Failed(String),
}

/// One entry in the export batch. Held in DISPLAY order, separately from
/// `export_queue` (which is the remaining work and is consumed from the back),
/// so the UI can show a stable list while the queue drains.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportJob {
    pub id: i64,
    pub filename: String,
    pub state: ExportJobState,
}

/// Phase 89: Export Settings
#[derive(Debug, Clone, PartialEq)]
pub struct ExportSettings {
    pub format: ExportFormat,
    /// Colour space the pixels are encoded in and tagged with (ICC profile).
    pub color_space: crate::core::output_space::OutputSpace,
    pub quality: u8,
    pub resize: bool,
    pub max_width: u32,
    /// The "Max long edge" field as typed. `max_width` is its value when it is
    /// a positive whole number, else 0 (see [`ExportSettings::size_invalid`]).
    pub max_width_text: String,
    pub subfolder: String,
    pub base_path: std::path::PathBuf,
    /// JPEG only: write Ultra HDR (`session::ultra_hdr`), the SDR image
    /// plus a gain map for `core::hdr::EXPORT_HEADROOM`. Viewers without
    /// HDR show the SDR image as before.
    pub ultra_hdr: bool,
}

impl ExportSettings {
    /// Resize is on but the long edge is not a positive number: exporting
    /// would silently write full-size files.
    pub fn size_invalid(&self) -> bool {
        self.resize && self.max_width == 0
    }

    /// The long-edge field as typed. `max_width` follows it; 0 means "not a
    /// usable number" (empty, a stray letter, a minus sign).
    pub fn set_max_width_text(&mut self, text: String) {
        self.max_width = text.trim().parse::<u32>().unwrap_or(0);
        self.max_width_text = text;
    }
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            format: ExportFormat::Jpeg,
            color_space: crate::core::output_space::OutputSpace::Srgb,
            quality: 80,
            resize: false,
            max_width: 2048,
            max_width_text: "2048".to_string(),
            subfolder: "Export".to_string(),
            base_path: dirs::picture_dir().unwrap_or_else(|| std::path::PathBuf::from(".")),
            ultra_hdr: false,
        }
    }
}


/// How long to wait before re-checking memory when a batch is paused.
const MEMORY_RETRY_SECS: u64 = 2;

/// Outcome of the pre-image memory check.
enum MemoryGate {
    /// Enough headroom (or the check is disabled / unavailable).
    Proceed,
    /// Below the threshold even after releasing our own caches.
    Wait { available_mb: u64, needed_mb: u64 },
}

/// Carry on with the next image (or finish the batch) on a later turn of
/// the event loop, so the UI updates between images.
fn next_export() -> Effects {
    Effects::perform("export next", async { SessionEvent::ProcessNextExport })
}

impl Session {
    /// Images the current selection would export, in display order.
    ///
    /// `multi_selection` is a `HashSet`, so iterating it directly gave an
    /// arbitrary order — and the queue is drained with `pop()`, so batches ran
    /// back-to-front in a different order every time. Ordering through
    /// `displayed_ids` makes a run reproducible and the progress list readable.
    pub fn export_target_ids(&self) -> Vec<i64> {
        if self.multi_selection.is_empty() {
            return self.selected_image_id.into_iter().collect();
        }
        let mut ids: Vec<i64> = self
            .displayed_ids()
            .into_iter()
            .filter(|id| self.multi_selection.contains(id))
            .collect();
        // Anything selected but not currently displayed still belongs in the
        // batch; append it rather than silently dropping it.
        let mut extra: Vec<i64> = self
            .multi_selection
            .iter()
            .copied()
            .filter(|id| !ids.contains(id))
            .collect();
        extra.sort_unstable();
        ids.append(&mut extra);
        ids
    }

    /// Export the action targets with the current settings. `None` when the
    /// settings are unusable (the dialog stays open, the status says why);
    /// otherwise the dialog's job is done, whether or not anything started.
    pub fn start_export(&mut self) -> Option<Effects> {
        if self.export_settings.size_invalid() {
            self.status = "Enter a long edge in pixels, or untick Resize long edge.".to_string();
            return None;
        }

        // Re-entrancy guard, matching delete_pending_from_disk's
        // `is_deleting` check. Without it, confirming again mid-run overwrote
        // export_queue and started a second interleaved ProcessNextExport chain.
        if self.is_exporting {
            self.status = "An export is already running.".to_string();
            return Some(Effects::none());
        }

        let ids = self.export_target_ids();
        if ids.is_empty() {
            self.status = "Nothing to export.".to_string();
            return Some(Effects::none());
        }

        self.export_jobs = ids
            .iter()
            .map(|id| ExportJob {
                id: *id,
                filename: self
                    .images
                    .iter()
                    .find(|i| i.id == *id)
                    .map(|i| i.filename.clone())
                    .unwrap_or_else(|| format!("image {id}")),
                state: ExportJobState::Pending,
            })
            .collect();
        // Reversed: the queue is drained with `pop()`, so this yields display order.
        self.export_queue = ids.into_iter().rev().collect();

        self.is_exporting = true;
        self.status = format!("Starting export of {} images…", self.export_jobs.len());
        Some(next_export())
    }

    /// Cancel after the in-flight image finishes. The running GPU render is not
    /// interruptible, so this drains the pending work rather than killing a task
    /// mid-readback.
    pub fn cancel_export(&mut self) {
        if !self.is_exporting {
            return;
        }
        let dropped = self.export_queue.len();
        self.export_queue.clear();
        for job in self.export_jobs.iter_mut() {
            if job.state == ExportJobState::Pending {
                job.state = ExportJobState::Failed("cancelled".to_string());
            }
        }
        self.status = format!("Export cancelled — {dropped} images skipped");
    }

    /// Update one job's progress. Silently ignores unknown ids so a stale message
    /// from a cancelled batch can't panic.
    fn set_job_state(&mut self, id: i64, state: ExportJobState) {
        if let Some(job) = self.export_jobs.iter_mut().find(|j| j.id == id) {
            job.state = state;
        }
    }

    /// Decide whether there is room to render another full-resolution frame.
    ///
    /// A single 24 MP export peaks at several hundred MB across CPU and VRAM, so
    /// starting one with almost nothing free is how the machine ends up thrashing.
    ///
    /// Note this does NOT reduce concurrency: export is already strictly
    /// sequential, one image chained to the next. The lever that actually exists is
    /// releasing the app's own regenerable caches, and failing that, waiting.
    fn check_memory_gate(&mut self) -> MemoryGate {
        let needed = self.min_free_bytes();
        if needed == 0 {
            return MemoryGate::Proceed;
        }
        // `None` means "could not determine" — proceed rather than block forever
        // on a platform with no implementation.
        let Some(available) = crate::core::memory::available_bytes() else {
            return MemoryGate::Proceed;
        };
        if available >= needed {
            return MemoryGate::Proceed;
        }

        // Give back what we can before asking the user to wait: the preview and
        // RAW caches are hundreds of MB of pure convenience.
        self.release_discretionary_memory();

        let after = crate::core::memory::available_bytes().unwrap_or(available);
        if after >= needed {
            tracing::warn!(
                "Export: freed caches to stay above the {} MB headroom ({} MB -> {} MB available)",
                crate::core::memory::as_mb(needed),
                crate::core::memory::as_mb(available),
                crate::core::memory::as_mb(after),
            );
            return MemoryGate::Proceed;
        }

        MemoryGate::Wait {
            available_mb: crate::core::memory::as_mb(after),
            needed_mb: crate::core::memory::as_mb(needed),
        }
    }

    pub fn process_next_export(&mut self) -> Effects {
        // Nothing queued means the batch is over (or was cancelled) — skip the
        // memory check so cancellation can't be held up by low memory.
        if !self.export_queue.is_empty() {
            if let MemoryGate::Wait { available_mb, needed_mb } = self.check_memory_gate() {
                // Log once per transition into the paused state. This runs once
                // per image, and release builds log at info/warn to a daily file —
                // an unguarded warn! here would flood it.
                if !self.export_paused_for_memory {
                    tracing::warn!(
                        "Export paused: {available_mb} MB available, {needed_mb} MB required"
                    );
                }
                self.export_paused_for_memory = true;
                self.status = format!(
                    "Export paused — {available_mb} MB free, waiting for {needed_mb} MB"
                );
                return Effects::perform("export memory wait", async {
                    tokio::time::sleep(std::time::Duration::from_secs(MEMORY_RETRY_SECS)).await;
                    SessionEvent::ProcessNextExport
                });
            }
            if self.export_paused_for_memory {
                self.export_paused_for_memory = false;
                tracing::warn!("Export resumed: memory headroom recovered");
            }
        }

        if let Some(image_id) = self.export_queue.pop() {
            let done = self
                .export_jobs
                .iter()
                .filter(|j| !matches!(j.state, ExportJobState::Pending))
                .count();
            self.status = format!("Exporting {} of {}…", done + 1, self.export_jobs.len());
            self.set_job_state(image_id, ExportJobState::Working);
            if let Some(img) = self.images.iter().find(|i| i.id == image_id) {
                let path = img.path.clone();
                // Each image's own saved edits. For the image currently open in
                // Develop, prefer the in-memory params: they may hold a gesture
                // that has not been flushed to SQLite yet.
                let mut params = if Some(image_id) == self.selected_image_id {
                    self.current_edit_params
                } else {
                    match self.library.as_ref().map(|lib| lib.load_saved_edit(image_id)) {
                        Some(Ok(saved)) => saved.unwrap_or_default(),
                        None => Default::default(),
                        // Exporting defaults would silently produce the unedited frame.
                        Some(Err(e)) => {
                            tracing::error!("Export: unreadable saved edit for {}: {e}", img.filename);
                            let msg = format!("saved edit for {} could not be read", img.filename);
                            self.set_job_state(image_id, ExportJobState::Failed(msg));
                            return next_export();
                        }
                    }
                };
                // Exporting while the crop tool is open must still produce the
                // cropped photo, not the tool's dimmed full-sensor preview.
                params = params.without_tool_state();
                // Reuse an already-decoded RAW when the develop/cull preload has
                // one. Skips a multi-second decode per image for the common
                // "cull, then export the selects" flow, and costs nothing: the
                // cache is already byte-budgeted and the entry is shared, not
                // copied.
                if let Some(cached) = self.raw_cache.get(&image_id).cloned() {
                    return Effects::perform("export raw", async move {
                        SessionEvent::ExportRawLoaded(image_id, Box::new(params), Ok(cached))
                    });
                }
                return Effects::perform("export raw", async move {
                    let res = raw::loader::load_raw_data(path).await;
                    SessionEvent::ExportRawLoaded(image_id, Box::new(params), res.map(std::sync::Arc::new))
                });
            }
            // Image id not found in the catalogue (removed mid-batch).
            self.set_job_state(
                image_id,
                ExportJobState::Failed("image no longer in catalogue".to_string()),
            );
            next_export()
        } else {
            self.is_exporting = false;
            self.export_paused_for_memory = false;
            let failed = self
                .export_jobs
                .iter()
                .filter(|j| matches!(j.state, ExportJobState::Failed(_)))
                .count();
            self.status = if failed > 0 {
                format!("Export finished — {failed} failed")
            } else {
                "Export Complete!".to_string()
            };
            // Release the LAST image's resources. Every other image is reclaimed by
            // the next one's save; without this the final ~336 MB sat allocated
            // until something else in the app happened to render.
            if let Some(ctx) = &self.gpu_context {
                ctx.reclaim();
            }
            Effects::none()
        }
    }

    pub fn export_raw_loaded(
        &mut self,
        image_id: i64,
        params: crate::core::types::EditParams,
        result: Result<Arc<raw::loader::RawDataResult>, String>,
    ) -> Effects {
        match result {
            Ok(raw_data) => {
                // Always build fresh full-resolution resources for export.
                // The develop pipeline uses a subsampled preview; export must use the
                // original sensor dimensions so we don't re-use those resources here.
                let ctx = self.gpu_context.clone();
                let gpu_init = self.gpu_init.clone();

                let profile_defaults = self.camera_profile_defaults.clone();

                Effects::perform("export resources", async move {
                    let res = async move {
                        let context = super::shared_context(&gpu_init, ctx).await?;
                        // CPU-heavy: on the blocking pool, not a tokio worker.
                        tokio::task::spawn_blocking(move || {
                        // The image's own camera profile (its choice, else its
                        // camera's default, else the best match) — resolved here,
                        // off the UI thread, exactly as the develop load does.
                        let profile_use = crate::core::camera_profiles::ProfileUse::resolve(
                            &raw_data,
                            params.camera_profile,
                            &profile_defaults,
                        );

                        // Must mirror the develop path (loading.rs): with a DCP the shader takes the
                        // has_dcp branch and expects the DCP ForwardMatrix (camera → XYZ D50) — passing
                        // the fallback camera → sRGB matrix there wrecks every colour (pink skin tones).
                        // Anchored WB: honour saved Temp/Tint edits exactly like develop.
                        let (kelvin, _tint, wb_override) = crate::color::solve_wb(
                            &params,
                            profile_use.as_shot,
                            profile_use.profile.as_deref(),
                            raw_data.color_matrix,
                        );

                        let (forward_matrix, interpolated_dcp) = if let Some(dcp) = &profile_use.profile {
                            let interpolated = crate::raw::dcp::interpolate_at_temperature(
                                dcp,
                                kelvin,
                                crate::core::tonemap::ToneSettings::from_params(&params),
                            );
                            (interpolated.camera_to_prophoto, Some(interpolated))
                        } else {
                            let cam_to_srgb = crate::color::calculate_cam_to_srgb(
                                raw_data.color_matrix,
                                raw_data.wb_multipliers,
                                raw_data.color_matrix_is_d65,
                            );
                            (cam_to_srgb, None)
                        };

                        // AI denoise: the cached result, regenerated if missing
                        // (an export must not silently drop an edit).
                        let mosaic = super::export_mosaic(&raw_data, image_id, params.ai_denoise);
                        gpu::shared::ImageResources::new(
                            &context,
                            image_id,
                            &mosaic,
                            raw_data.width,
                            raw_data.height,
                            &params,
                            // As-shot, exactly like the develop load: this is the
                            // space the demosaic and every analysis work in. The
                            // edited WB is applied below as the uniform override.
                            raw_data.wb_multipliers,
                            forward_matrix,
                            raw_data.cfa_pattern,
                            raw_data.black_levels,
                            raw_data.white_level,
                            interpolated_dcp.as_ref(),
                            None, // full resolution for export
                            raw_data.orientation,
                            raw_data.lens_context.clone(),
                        )
                        .map(|res| {
                            res.update_uniforms(&context, &params, interpolated_dcp.as_ref(), wb_override);
                            // Export's resources are its own: give them the AI
                            // masks' bitmaps, or those masks would export empty.
                            if params.ai_masks().next().is_some() {
                                let bitmaps = super::load_ai_bitmaps(image_id, &params);
                                res.set_ai_bitmaps(Arc::new(bitmaps));
                            }
                            (context, Arc::new(res))
                        })
                        })
                        .await
                        .map_err(|e| format!("resource build panicked: {e}"))?
                    }
                    .await;
                    SessionEvent::ExportPipelineReady(image_id, Box::new(params), res)
                })
            }
            Err(e) => {
                self.status = format!("Failed to load RAW: {}", e);
                // Without this the job stays `Working` forever and the queue view
                // shows an ever-growing pile of "rendering" rows — which is what
                // made a memory failure look like runaway concurrency.
                self.set_job_state(image_id, ExportJobState::Failed(e));
                next_export()
            }
        }
    }

    pub fn export_pipeline_ready(
        &mut self,
        image_id: i64,
        params: crate::core::types::EditParams,
        result: Result<
            (
                Arc<gpu::shared::SharedContext>,
                Arc<gpu::shared::ImageResources>,
            ),
            String,
        >,
    ) -> Effects {
        match result {
            Ok((context, _)) if context.is_lost() => {
                // A reset GPU renders nothing; forget it so the next image (or
                // the next Develop load) builds a fresh context.
                tracing::error!("Export of image {image_id}: GPU device lost");
                self.gpu_context = None;
                self.gpu_init = Arc::new(tokio::sync::OnceCell::new());
                let e = "the GPU was reset; try again".to_string();
                self.status = format!("Export failed: {e}");
                self.set_job_state(image_id, ExportJobState::Failed(e));
                next_export()
            }
            Ok((context, resources)) => {
                // THE leak fix. `gpu_context` is otherwise only ever assigned on
                // the Develop image-load path (loading.rs), so exporting without
                // having opened Develop left it None and the per-image fallback
                // below built a COMPLETE new wgpu stack — instance, adapter
                // enumeration, device, queue, 2 shader modules, 2 bind-group
                // layouts, 3 render pipelines — for every single image in the
                // batch, then dropped it with no poll. Storing it here means the
                // first export creates one context and the rest reuse it.
                if self.gpu_context.is_none() {
                    self.gpu_context = Some(context.clone());
                }

                // This image's own crop, not the one open in Develop — otherwise
                // the output is sized from the wrong image's crop rectangle.
                let crop = params.crop;
                let crop_w = crop[2].clamp(0.001, 1.0);
                let crop_h = crop[3].clamp(0.001, 1.0);
                // Use original (full-sensor) dimensions in DISPLAY orientation —
                // portrait shots must export portrait (crop is display-space too).
                let (full_w, full_h) = resources.oriented_original_dims(params.quarter_turns);
                let cropped_w = ((full_w as f32 * crop_w) as u32).max(1);
                let cropped_h = ((full_h as f32 * crop_h) as u32).max(1);

                let settings = self.export_settings.clone();
                // Honour "Resize Long Edge". This was previously computed but the
                // setting was never read, so the checkbox and the size field did
                // nothing at all.
                let (target_width, target_height) = resized_dims(cropped_w, cropped_h, &settings);
                // The colour pass reads the source with one bilinear tap, so past a
                // 2x reduction it skips pixels and fine detail aliases. Render at
                // the largest whole multiple of the target that fits in the
                // source and box-average down (`box_reduce`).
                let factor = supersample_factor((cropped_w, cropped_h), (target_width, target_height));
                let (render_w, render_h) = (target_width * factor, target_height * factor);
                if let Err(e) = check_export_size(&context, render_w, render_h, settings.format) {
                    tracing::error!("Export of image {image_id} refused: {e}");
                    self.status = format!("Export failed: {e}");
                    self.set_job_state(image_id, ExportJobState::Failed(e));
                    context.reclaim();
                    return next_export();
                }
                let source = self
                    .images
                    .iter()
                    .find(|i| i.id == image_id)
                    .map(|img| PathBuf::from(&img.path));
                let filename = source
                    .as_deref()
                    .and_then(Path::file_stem)
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("image_{}", image_id));

                // Encode into the chosen space. Per-export resources, so this
                // never reaches the display's uniform buffer.
                crate::gpu::render_functions::write_output_space(&context, &resources, settings.color_space);
                Effects::perform("export render", async move {
                    let res = async move {
                        let pixels = if settings.format == ExportFormat::Tiff {
                            let (samples, _, _) = crate::gpu::render_functions::render_to_bytes_16bit(
                                &context,
                                &resources,
                                render_w,
                                render_h,
                            )
                            .await?;
                            ExportPixels::Rgb16(box_reduce(&samples, render_w, render_h, factor))
                        } else {
                            let (bytes, _, _) = crate::gpu::render_functions::render_to_vec(
                                &context,
                                &resources,
                                render_w,
                                render_h,
                                // Export always renders the whole cropped frame,
                                // never the on-screen window.
                                crate::gpu::params::FULL_VIEW_RECT,
                            )
                            .await?;
                            let sdr = box_reduce(&bytes, render_w, render_h, factor);
                            drop(bytes);
                            if settings.ultra_hdr && settings.format == ExportFormat::Jpeg {
                                let scale = super::ultra_hdr::GAIN_MAP_SCALE;
                                let (gw, gh) = (target_width.div_ceil(scale), target_height.div_ceil(scale));
                                let gain = crate::gpu::render_functions::render_gain_map(
                                    &context,
                                    &resources,
                                    gw,
                                    gh,
                                    crate::core::hdr::EXPORT_HEADROOM,
                                )
                                .await?;
                                ExportPixels::UltraHdr { sdr, gain, gain_dims: (gw, gh) }
                            } else {
                                ExportPixels::Rgba8(sdr)
                            }
                        };
                        save_export_async(filename, source, pixels, target_width, target_height, settings)
                            .await
                    }
                    .await;
                    SessionEvent::ExportSaveComplete(image_id, res)
                })
            }
            Err(e) => {
                self.status = format!("Export failed: {}", e);
                // See handle_export_raw_loaded: an unmarked failure leaves a stuck
                // "rendering" row behind.
                self.set_job_state(image_id, ExportJobState::Failed(e));
                if let Some(ctx) = &self.gpu_context {
                    ctx.reclaim();
                }
                next_export()
            }
        }
    }

    pub fn export_save_complete(
        &mut self,
        id: i64,
        result: Result<PathBuf, String>,
    ) -> Effects {
        match result {
            Ok(path) => {
                tracing::info!("Export saved successfully: {}", path.display());
                self.status = format!("Saved: {}", path.display());
                self.set_job_state(id, ExportJobState::Done(path));
            }
            Err(e) => {
                tracing::error!("Export save failed: {}", e);
                self.status = format!("Save failed: {}", e);
                self.set_job_state(id, ExportJobState::Failed(e));
            }
        }
        // The future holding this image's ImageResources has already resolved, so
        // its textures are dropped by now — but wgpu will not hand the memory back
        // until it is polled. Without this the batch runs a full image behind on
        // reclamation.
        if let Some(ctx) = &self.gpu_context {
            ctx.reclaim();
        }
        next_export()
    }
}

// Helpers

/// Apply the "Resize Long Edge" setting, preserving aspect ratio.
///
/// The setting is a cap on the LONGER edge, so portrait and landscape frames
/// come out the same size on their long side — which is what "long edge"
/// means and what the checkbox has always said. (The width field said "Max
/// Width"; the two disagreed, and neither was implemented.)
///
/// Only ever shrinks: asking for a 4000px long edge on a 3000px crop returns
/// the crop untouched rather than upscaling it into softness.
fn resized_dims(width: u32, height: u32, settings: &ExportSettings) -> (u32, u32) {
    if !settings.resize || settings.max_width == 0 {
        return (width, height);
    }
    let long_edge = width.max(height);
    if long_edge <= settings.max_width {
        return (width, height);
    }
    let scale = settings.max_width as f64 / long_edge as f64;
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

/// The whole-number multiple of the target size to render at: the largest
/// factor whose render still fits inside the cropped source (never upscaling),
/// and 1 when there is no reduction to speak of.
fn supersample_factor(source: (u32, u32), target: (u32, u32)) -> u32 {
    (source.0 / target.0.max(1)).min(source.1 / target.1.max(1)).max(1)
}

/// Average each `factor`×`factor` block of a `width`×`height` image (any
/// channel count, inferred from the length) down to one pixel.
fn box_reduce<T: Copy + Into<u32> + TryFrom<u32>>(
    src: &[T],
    width: u32,
    height: u32,
    factor: u32,
) -> Vec<T> {
    if factor <= 1 {
        return src.to_vec();
    }
    let (w, h, f) = (width as usize, height as usize, factor as usize);
    let channels = src.len() / (w * h).max(1);
    let (ow, oh) = (w / f, h / f);
    let area = (f * f) as u32;
    let mut out = Vec::with_capacity(ow * oh * channels);
    for y in 0..oh {
        for x in 0..ow {
            for c in 0..channels {
                let mut sum = 0u32;
                for dy in 0..f {
                    let row = (y * f + dy) * w + x * f;
                    for dx in 0..f {
                        sum += src[(row + dx) * channels + c].into();
                    }
                }
                let avg = (sum + area / 2) / area;
                out.push(T::try_from(avg).unwrap_or(src[0]));
            }
        }
    }
    out
}

/// Reject an export the GPU or the encoder cannot produce, before any work.
///
/// Without this a too-large render failed wgpu validation mid-batch, and a
/// JPEG past 65535px had its dimensions silently truncated by the `as u16`
/// the encoder requires.
fn check_export_size(
    context: &gpu::shared::SharedContext,
    width: u32,
    height: u32,
    format: ExportFormat,
) -> Result<(), String> {
    context.check_texture_size(width, height)?;
    let bytes_per_pixel: u64 = if format == ExportFormat::Tiff { 8 } else { 4 };
    let row = (width as u64 * bytes_per_pixel).next_multiple_of(256);
    context.check_buffer_size(row * height as u64)?;
    if format == ExportFormat::Jpeg && (width > u16::MAX as u32 || height > u16::MAX as u32) {
        return Err(format!("{width}×{height} is larger than JPEG allows (65535px)"));
    }
    Ok(())
}

/// Pick a destination path, adding ` (2)`, ` (3)`… rather than overwriting.
///
/// Two source folders commonly hold the same stems (DSC_0001 from two cards,
/// or a re-export into the same folder). `File::create` truncates, so the
/// previous behaviour silently destroyed the earlier export.
fn unique_output_path(dir: &Path, stem: &str, extension: &str) -> Result<PathBuf, String> {
    let candidate = dir.join(format!("{stem}.{extension}"));
    if !candidate.exists() {
        return Ok(candidate);
    }
    for n in 2..10_000u32 {
        let candidate = dir.join(format!("{stem} ({n}).{extension}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    // Pathological: refuse rather than overwrite an earlier export.
    Err(format!("10,000 files named {stem} already exist in {}", dir.display()))
}

/// Carries either an 8-bit RGBA (JPEG/PNG) or 16-bit RGB (TIFF) sample
/// buffer through the async save step, so `save_export_async` can't be
/// called with a format/pixel-type mismatch.
enum ExportPixels {
    /// Owned, not `Arc`: export hands the buffer straight to an encoder and
    /// never shares it, so wrapping it would mean an extra full-frame copy
    /// (96 MB at 24 MP) alive alongside the mapped readback buffer.
    Rgba8(Vec<u8>),
    Rgb16(Vec<u16>),
    /// An Ultra HDR JPEG's two images: the SDR one (RGBA) and its gain map
    /// (one byte a pixel, `gain_dims`).
    UltraHdr { sdr: Vec<u8>, gain: Vec<u8>, gain_dims: (u32, u32) },
}

/// TIFF tag 34675, InterColorProfile.
const TIFF_ICC_PROFILE: u16 = 34675;

/// An ICC profile as a TIFF value. The tiff crate writes `[u8]` as BYTE;
/// the TIFF/EP and ICC specifications give this tag type UNDEFINED.
struct IccBytes<'a>(&'a [u8]);

impl tiff::encoder::TiffValue for IccBytes<'_> {
    const BYTE_LEN: u8 = 1;
    const FIELD_TYPE: tiff::tags::Type = tiff::tags::Type::UNDEFINED;

    fn count(&self) -> usize {
        self.0.len()
    }

    fn data(&self) -> std::borrow::Cow<'_, [u8]> {
        std::borrow::Cow::Borrowed(self.0)
    }
}

async fn save_export_async(
    filename: String,
    source: Option<PathBuf>,
    pixels: ExportPixels,
    width: u32,
    height: u32,
    settings: ExportSettings,
) -> Result<PathBuf, String> {
    tokio::task::spawn_blocking(move || {
        use std::fs::File;
        use std::io::{BufWriter, Write};

        // A failed GPU readback hands back an empty buffer. Catch it before a
        // file exists, rather than leaving an empty or truncated image on disk.
        let (got, want) = match &pixels {
            ExportPixels::Rgba8(b) | ExportPixels::UltraHdr { sdr: b, .. } => (b.len(), width as usize * height as usize * 4),
            ExportPixels::Rgb16(s) => (s.len(), width as usize * height as usize * 3),
        };
        if got != want {
            return Err(format!(
                "GPU readback returned {got} samples, expected {want} — nothing was written"
            ));
        }

        let output_dir = settings.base_path.join(&settings.subfolder);
        std::fs::create_dir_all(&output_dir).map_err(|e| e.to_string())?;

        let extension = match settings.format {
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Png => "png",
            ExportFormat::Tiff => "tiff",
        };

        let output_path = unique_output_path(&output_dir, &filename, extension)?;
        let file = File::create(&output_path).map_err(|e| e.to_string())?;
        let mut writer = BufWriter::new(file);

        // Every format carries the profile, sRGB included: untagged files
        // are only *assumed* to be sRGB, and the wide spaces are unreadable
        // without one.
        let icc = settings.color_space.icc_profile();

        let encoded = (|| -> Result<(), String> {
            match (settings.format, pixels) {
                (ExportFormat::Png, ExportPixels::Rgba8(bytes)) => {
                    let mut info = png::Info::with_size(width, height);
                    info.color_type = png::ColorType::Rgba;
                    info.bit_depth = png::BitDepth::Eight;
                    info.icc_profile = Some(std::borrow::Cow::Borrowed(&icc));
                    let encoder = png::Encoder::with_info(&mut writer, info).map_err(|e| e.to_string())?;
                    let mut png_writer = encoder.write_header().map_err(|e| e.to_string())?;
                    png_writer.write_image_data(&bytes).map_err(|e| e.to_string())?;
                    // `finish` rather than drop: dropping swallows the error.
                    png_writer.finish().map_err(|e| e.to_string())?;
                }
                (ExportFormat::Jpeg, ExportPixels::Rgba8(bytes)) => {
                    let mut encoder = jpeg_encoder::Encoder::new(&mut writer, settings.quality);
                    // Capture date, camera, lens and exposure from the RAW, so
                    // the export sorts and reads like the original. Best
                    // effort: a RAW without readable EXIF still exports.
                    let color_space = settings.color_space.exif_color_space();
                    if let Some(app1) =
                        source.as_deref().and_then(|p| crate::raw::exif::export_app1(p, color_space))
                    {
                        if let Err(e) = encoder.add_app_segment(1, &app1) {
                            tracing::warn!("Exporting without EXIF: {e}");
                        }
                    }
                    encoder.add_icc_profile(&icc).map_err(|e| e.to_string())?;
                    // Encoded straight from RGBA — the encoder ignores alpha.
                    // The RGB repack this used to do was a third full-frame
                    // buffer (~72 MB at 24 MP) alive next to the readback copy.
                    encoder
                        .encode(&bytes, width as u16, height as u16, jpeg_encoder::ColorType::Rgba)
                        .map_err(|e| e.to_string())?;
                }
                (ExportFormat::Jpeg, ExportPixels::UltraHdr { sdr, gain, gain_dims: (gw, gh) }) => {
                    if gain.len() != gw as usize * gh as usize {
                        return Err("the gain map came back the wrong size".into());
                    }
                    let mut gain_map = Vec::new();
                    let mut encoder = jpeg_encoder::Encoder::new(&mut gain_map, super::ultra_hdr::GAIN_MAP_QUALITY);
                    encoder
                        .add_app_segment(1, &super::ultra_hdr::gain_map_xmp(crate::core::hdr::EXPORT_HEADROOM))
                        .map_err(|e| e.to_string())?;
                    encoder
                        .encode(&gain, gw as u16, gh as u16, jpeg_encoder::ColorType::Luma)
                        .map_err(|e| e.to_string())?;

                    // The primary: the plain export's segments plus the
                    // container XMP; MPF is added once its size is known.
                    let mut primary = Vec::new();
                    let mut encoder = jpeg_encoder::Encoder::new(&mut primary, settings.quality);
                    let color_space = settings.color_space.exif_color_space();
                    if let Some(app1) =
                        source.as_deref().and_then(|p| crate::raw::exif::export_app1(p, color_space))
                    {
                        if let Err(e) = encoder.add_app_segment(1, &app1) {
                            tracing::warn!("Exporting without EXIF: {e}");
                        }
                    }
                    encoder
                        .add_app_segment(1, &super::ultra_hdr::primary_xmp(gain_map.len()))
                        .map_err(|e| e.to_string())?;
                    encoder.add_icc_profile(&icc).map_err(|e| e.to_string())?;
                    encoder
                        .encode(&sdr, width as u16, height as u16, jpeg_encoder::ColorType::Rgba)
                        .map_err(|e| e.to_string())?;
                    drop(sdr);
                    let file = super::ultra_hdr::assemble(&primary, &gain_map)?;
                    writer.write_all(&file).map_err(|e| e.to_string())?;
                }
                (ExportFormat::Tiff, ExportPixels::Rgb16(samples)) => {
                    let mut tiff = tiff::encoder::TiffEncoder::new(&mut writer).map_err(|e| e.to_string())?;
                    let mut image = tiff
                        .new_image::<tiff::encoder::colortype::RGB16>(width, height)
                        .map_err(|e| e.to_string())?;
                    image
                        .encoder()
                        .write_tag(tiff::tags::Tag::Unknown(TIFF_ICC_PROFILE), IccBytes(&icc))
                        .map_err(|e| e.to_string())?;
                    image.write_data(&samples).map_err(|e| e.to_string())?;
                }
                (format, _) => {
                    return Err(format!("Export pixel buffer does not match format {format}"));
                }
            }
            // Explicit: a BufWriter flushes on drop but discards the error, so
            // a full disk used to report a truncated file as a success.
            writer.flush().map_err(|e| e.to_string())
        })();

        if let Err(e) = encoded {
            drop(writer);
            let _ = std::fs::remove_file(&output_path);
            return Err(e);
        }
        Ok(output_path)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(resize: bool, max_width: u32) -> ExportSettings {
        ExportSettings { resize, max_width, ..ExportSettings::default() }
    }

    /// `0` must mean "disabled", and the byte conversion must be MB not kB —
    /// a factor-1024 slip here would either never gate or always gate.
    #[test]
    fn min_free_bytes_converts_mb_and_treats_zero_as_disabled() {
        let mut e = Session::default();
        e.min_free_ram_mb = 2048;
        assert_eq!(e.min_free_bytes(), 2048 * 1024 * 1024);
        e.min_free_ram_mb = 0;
        assert_eq!(e.min_free_bytes(), 0);
    }

    /// With the check disabled the gate must not even consult the system, and
    /// must never touch the caches.
    #[test]
    fn gate_proceeds_and_keeps_caches_when_disabled() {
        let mut e = Session::default();
        e.min_free_ram_mb = 0;
        assert!(matches!(e.check_memory_gate(), MemoryGate::Proceed));
    }

    /// A threshold no machine can satisfy must end in Wait — and, critically,
    /// only AFTER the caches have been released, since that is the app's one
    /// real lever.
    #[test]
    fn gate_waits_only_after_releasing_caches() {
        let mut e = Session::default();
        if crate::core::memory::available_bytes().is_none() {
            // Platform has no implementation; the gate is inert by design.
            return;
        }
        e.min_free_ram_mb = crate::session::MIN_FREE_RAM_MB_MAX;
        e.raw_cache_bytes = 999;
        match e.check_memory_gate() {
            MemoryGate::Wait { needed_mb, .. } => {
                assert_eq!(needed_mb, crate::session::MIN_FREE_RAM_MB_MAX as u64);
                assert_eq!(e.raw_cache_bytes, 0, "caches should have been released");
            }
            MemoryGate::Proceed => panic!("16 GB of headroom should not be satisfiable"),
        }
    }

    /// A trivially small threshold must pass on any machine that can run this.
    #[test]
    fn gate_proceeds_when_there_is_plenty_of_headroom() {
        let mut e = Session::default();
        e.min_free_ram_mb = 1;
        assert!(matches!(e.check_memory_gate(), MemoryGate::Proceed));
    }

    #[test]
    fn resize_off_leaves_dimensions_alone() {
        assert_eq!(resized_dims(6000, 4000, &settings(false, 1000)), (6000, 4000));
    }

    /// The setting caps the LONG edge, so a portrait and a landscape frame come
    /// out matched on their long side. Capping width instead would make
    /// portraits much larger, which is what the mismatched label implied.
    #[test]
    fn resize_caps_the_long_edge_in_either_orientation() {
        assert_eq!(resized_dims(6000, 4000, &settings(true, 3000)), (3000, 2000));
        assert_eq!(resized_dims(4000, 6000, &settings(true, 3000)), (2000, 3000));
    }

    #[test]
    fn resize_preserves_aspect_ratio() {
        let (w, h) = resized_dims(6000, 4000, &settings(true, 1999));
        let before = 6000.0 / 4000.0;
        let after = w as f64 / h as f64;
        assert!((before - after).abs() < 0.01, "{w}x{h}");
    }

    /// Never upscale: asking for a long edge bigger than the source would only
    /// add softness and file size.
    #[test]
    fn resize_never_enlarges() {
        assert_eq!(resized_dims(1200, 800, &settings(true, 4000)), (1200, 800));
        assert_eq!(resized_dims(1200, 800, &settings(true, 1200)), (1200, 800));
    }

    #[test]
    fn resize_ignores_a_zero_limit_rather_than_collapsing() {
        assert_eq!(resized_dims(6000, 4000, &settings(true, 0)), (6000, 4000));
    }

    #[test]
    fn resize_never_returns_a_zero_dimension() {
        let (w, h) = resized_dims(6000, 10, &settings(true, 1));
        assert!(w >= 1 && h >= 1, "{w}x{h}");
    }

    /// `File::create` truncates, so without uniquifying, two source folders
    /// holding the same stem meant the second export destroyed the first.
    /// Every format carries the chosen space's ICC profile, byte for byte —
    /// without it a wide-gamut export displays desaturated and wrong.
    #[test]
    fn every_format_embeds_the_colour_spaces_profile() {
        use crate::core::output_space::OutputSpace;
        let dir = std::env::temp_dir().join(format!("raw-editor-icc-{}", std::process::id()));
        let rt = tokio::runtime::Builder::new_current_thread().build().expect("runtime");
        let space = OutputSpace::DisplayP3;
        let want = space.icc_profile();
        for format in [ExportFormat::Jpeg, ExportFormat::Png, ExportFormat::Tiff] {
            let settings = ExportSettings {
                format,
                color_space: space,
                base_path: dir.clone(),
                subfolder: String::new(),
                ..ExportSettings::default()
            };
            let pixels = match format {
                ExportFormat::Tiff => ExportPixels::Rgb16(vec![30_000; 2 * 2 * 3]),
                _ => ExportPixels::Rgba8(vec![128; 2 * 2 * 4]),
            };
            let path = rt
                .block_on(save_export_async(format!("icc_{format}"), None, pixels, 2, 2, settings))
                .expect("saved");
            let got = match format {
                ExportFormat::Jpeg => {
                    let bytes = std::fs::read(&path).expect("read");
                    let at = bytes.windows(12).position(|w| w == b"ICC_PROFILE\0").expect("APP2 ICC segment");
                    // The segment length (counting itself) sits just before
                    // the identifier; then a sequence number and a count.
                    let len = u16::from_be_bytes([bytes[at - 2], bytes[at - 1]]) as usize;
                    assert_eq!((bytes[at + 12], bytes[at + 13]), (1, 1), "one chunk");
                    bytes[at + 14..at - 2 + len].to_vec()
                }
                ExportFormat::Png => {
                    let decoder = png::Decoder::new(std::fs::File::open(&path).expect("open"));
                    let reader = decoder.read_info().expect("png");
                    reader.info().icc_profile.as_ref().expect("iCCP chunk").to_vec()
                }
                ExportFormat::Tiff => {
                    let mut decoder = tiff::decoder::Decoder::new(std::fs::File::open(&path).expect("open")).expect("tiff");
                    decoder.get_tag_u8_vec(tiff::tags::Tag::Unknown(TIFF_ICC_PROFILE)).expect("ICC tag")
                }
            };
            assert_eq!(got, want, "{format}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_path_avoids_clobbering_an_existing_file() {
        let dir = std::env::temp_dir().join(format!("raw-editor-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");

        let first = unique_output_path(&dir, "DSC_0001", "jpg").unwrap();
        assert_eq!(first.file_name().unwrap(), "DSC_0001.jpg");
        std::fs::write(&first, b"x").expect("write");

        let second = unique_output_path(&dir, "DSC_0001", "jpg").unwrap();
        assert_eq!(second.file_name().unwrap(), "DSC_0001 (2).jpg");
        std::fs::write(&second, b"x").expect("write");

        let third = unique_output_path(&dir, "DSC_0001", "jpg").unwrap();
        assert_eq!(third.file_name().unwrap(), "DSC_0001 (3).jpg");

        // A different extension is a different file and must not be renamed.
        let png = unique_output_path(&dir, "DSC_0001", "png").unwrap();
        assert_eq!(png.file_name().unwrap(), "DSC_0001.png");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod downscale_tests {
    use super::*;

    #[test]
    fn factor_is_a_whole_multiple_that_fits() {
        assert_eq!(supersample_factor((6000, 4000), (1500, 1000)), 4);
        assert_eq!(supersample_factor((6000, 4000), (4000, 2667)), 1);
        assert_eq!(supersample_factor((6000, 4000), (6000, 4000)), 1);
    }

    /// A 1-pixel checkerboard is the worst case for a point sample: it reads
    /// all black or all white. Averaged, it must come out mid-grey.
    #[test]
    fn box_reduce_averages_a_checkerboard() {
        let (w, h) = (8u32, 8u32);
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let v = if (x + y) % 2 == 0 { 0u8 } else { 255 };
                px.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let out = box_reduce(&px, w, h, 4);
        assert_eq!(out.len(), 2 * 2 * 4);
        for p in out.chunks(4) {
            assert!((p[0] as i32 - 128).abs() <= 1, "got {}", p[0]);
            assert_eq!(p[3], 255);
        }
    }

    #[test]
    fn box_reduce_handles_sixteen_bit_rgb() {
        let src: Vec<u16> = [0u16, 65535].iter().cycle().take(4 * 3).copied().collect();
        let out = box_reduce(&src, 2, 2, 2);
        assert_eq!(out.len(), 3);
    }
}

/// The batch as a state machine: what each step asks for and how the job list
/// moves. Nothing here renders: results are delivered by hand (`testkit`).
#[cfg(test)]
mod queue_tests {
    use super::super::testkit::{deliver, labels, raw, run_ready, session_with_images};
    use super::*;

    /// Four images; the memory gate off, so a test never depends on the
    /// machine's free RAM.
    fn session() -> Session {
        let mut s = session_with_images(4);
        s.min_free_ram_mb = 0;
        s
    }

    fn states(s: &Session) -> Vec<(i64, ExportJobState)> {
        s.export_jobs.iter().map(|j| (j.id, j.state.clone())).collect()
    }

    fn start(s: &mut Session, ids: &[i64]) -> Effects {
        s.multi_selection = ids.iter().copied().collect();
        s.start_export().expect("settings are usable")
    }

    #[test]
    fn a_batch_runs_in_display_order_whatever_the_selection_order() {
        let mut s = session();
        let e = start(&mut s, &[4, 2, 3]);
        assert_eq!(labels(&e), ["export next"]);
        assert!(s.is_exporting);
        assert_eq!(
            states(&s),
            [(2, ExportJobState::Pending), (3, ExportJobState::Pending), (4, ExportJobState::Pending)]
        );
        assert_eq!(s.export_jobs[0].filename, "IMG_0002.NEF");

        let e = deliver(&mut s, SessionEvent::ProcessNextExport);
        assert_eq!(labels(&e), ["export raw"]);
        assert_eq!(s.export_jobs[0].state, ExportJobState::Working);
        assert_eq!(s.status, "Exporting 1 of 3…");
        assert_eq!(s.export_queue, [4, 3], "the rest, drained from the back");
    }

    #[test]
    fn nothing_selected_exports_the_open_image_and_nothing_open_exports_nothing() {
        let mut s = session();
        assert!(labels(&s.start_export().unwrap()).is_empty());
        assert!(!s.is_exporting);
        assert_eq!(s.status, "Nothing to export.");

        s.selected_image_id = Some(3);
        let _ = start(&mut s, &[]);
        assert_eq!(states(&s), [(3, ExportJobState::Pending)]);
    }

    #[test]
    fn an_unusable_size_keeps_the_dialog_open_and_starts_nothing() {
        let mut s = session();
        s.export_settings.resize = true;
        s.export_settings.set_max_width_text("12a".into());
        s.multi_selection = [1].into();
        assert!(s.start_export().is_none());
        assert!(!s.is_exporting);
        assert!(s.export_jobs.is_empty());
    }

    #[test]
    fn confirming_again_mid_run_does_not_start_a_second_chain() {
        let mut s = session();
        let _ = start(&mut s, &[1, 2]);
        let _ = deliver(&mut s, SessionEvent::ProcessNextExport);
        let before = (states(&s), s.export_queue.clone());

        s.multi_selection = [3, 4].into();
        let e = s.start_export().unwrap();
        assert!(labels(&e).is_empty());
        assert_eq!(s.status, "An export is already running.");
        assert_eq!((states(&s), s.export_queue.clone()), before);
    }

    /// Every way an image can fail marks its row and moves on: a row left
    /// `Working` reads as a render that never ends.
    #[test]
    fn each_failure_marks_its_job_and_the_batch_carries_on() {
        let mut s = session();
        let _ = start(&mut s, &[1, 2, 3]);

        let _ = deliver(&mut s, SessionEvent::ProcessNextExport);
        let e = deliver(&mut s, SessionEvent::ExportRawLoaded(1, Box::default(), Err("bad file".into())));
        assert_eq!(labels(&e), ["export next"]);

        let _ = deliver(&mut s, SessionEvent::ProcessNextExport);
        let e = deliver(&mut s, SessionEvent::ExportPipelineReady(2, Box::default(), Err("no GPU".into())));
        assert_eq!(labels(&e), ["export next"]);

        let _ = deliver(&mut s, SessionEvent::ProcessNextExport);
        let e = deliver(&mut s, SessionEvent::ExportSaveComplete(3, Err("disk full".into())));
        assert_eq!(labels(&e), ["export next"]);

        assert_eq!(
            states(&s),
            [
                (1, ExportJobState::Failed("bad file".into())),
                (2, ExportJobState::Failed("no GPU".into())),
                (3, ExportJobState::Failed("disk full".into())),
            ]
        );
        assert!(labels(&deliver(&mut s, SessionEvent::ProcessNextExport)).is_empty());
        assert!(!s.is_exporting);
        assert_eq!(s.status, "Export finished — 3 failed");
    }

    #[test]
    fn a_clean_batch_finishes_complete_with_each_saved_path() {
        let mut s = session();
        let _ = start(&mut s, &[1, 2]);
        for id in [1, 2] {
            let _ = deliver(&mut s, SessionEvent::ProcessNextExport);
            let _ = deliver(&mut s, SessionEvent::ExportSaveComplete(id, Ok(format!("/out/{id}.jpg").into())));
        }
        let _ = deliver(&mut s, SessionEvent::ProcessNextExport);
        assert!(!s.is_exporting);
        assert_eq!(s.status, "Export Complete!");
        assert_eq!(s.export_jobs[1].state, ExportJobState::Done("/out/2.jpg".into()));
    }

    #[test]
    fn a_decoded_raw_in_the_cache_is_reused_not_read_again() {
        let mut s = session();
        s.raw_cache.put(2, Arc::new(raw(2)));
        let _ = start(&mut s, &[2]);
        // Same label either way; the cached path is the one that does not
        // touch the (nonexistent) file, so the batch would otherwise fail.
        let e = deliver(&mut s, SessionEvent::ProcessNextExport);
        assert_eq!(labels(&e), ["export raw"]);
        assert!(
            matches!(run_ready(e)[..], [SessionEvent::ExportRawLoaded(2, _, Ok(_))]),
            "the cached RAW was handed on"
        );
    }

    #[test]
    fn cancelling_skips_the_pending_jobs_and_lets_the_running_one_finish() {
        let mut s = session();
        let _ = start(&mut s, &[1, 2, 3]);
        let _ = deliver(&mut s, SessionEvent::ProcessNextExport);

        s.cancel_export();
        assert!(s.export_queue.is_empty());
        assert_eq!(s.status, "Export cancelled — 2 images skipped");
        assert_eq!(
            states(&s),
            [
                (1, ExportJobState::Working),
                (2, ExportJobState::Failed("cancelled".into())),
                (3, ExportJobState::Failed("cancelled".into())),
            ]
        );
        assert!(s.is_exporting, "still running until the in-flight image lands");

        let _ = deliver(&mut s, SessionEvent::ExportSaveComplete(1, Ok("/out/1.jpg".into())));
        let _ = deliver(&mut s, SessionEvent::ProcessNextExport);
        assert!(!s.is_exporting);
        assert_eq!(s.export_jobs[0].state, ExportJobState::Done("/out/1.jpg".into()));
    }

    #[test]
    fn cancelling_with_nothing_running_does_nothing() {
        let mut s = session();
        s.cancel_export();
        assert_eq!(s.status, Session::default().status);
    }

    #[test]
    fn an_image_removed_mid_batch_fails_its_job_instead_of_stalling() {
        let mut s = session();
        let _ = start(&mut s, &[1, 2]);
        s.images.retain(|i| i.id != 1);
        let e = deliver(&mut s, SessionEvent::ProcessNextExport);
        assert_eq!(labels(&e), ["export next"]);
        assert_eq!(s.export_jobs[0].state, ExportJobState::Failed("image no longer in catalogue".into()));
    }

    /// A late result from a batch that has since been replaced names an id
    /// the new job list doesn't hold: it must not panic or touch a row.
    #[test]
    fn a_stale_result_for_an_unknown_image_is_ignored() {
        let mut s = session();
        let _ = start(&mut s, &[1]);
        let _ = deliver(&mut s, SessionEvent::ExportSaveComplete(99, Ok("/out/99.jpg".into())));
        assert_eq!(states(&s), [(1, ExportJobState::Pending)]);
    }
}
