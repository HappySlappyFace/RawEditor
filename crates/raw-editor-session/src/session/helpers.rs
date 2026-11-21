//! The editor's state helpers: history, edit flushing, the catalogue view,
//! display geometry and the RAW cache. They read and write only session
//! state (they used to be `RawEditor` methods).

use super::*;

impl Session {
    // Phase 65: Undo/Redo Helper
    // Returns a mutable reference to the (Stack, Index) tuple for the current image.
    // Initializes it if missing.
    pub fn get_current_history(
        &mut self,
    ) -> Option<&mut (Vec<crate::core::types::EditParams>, usize)> {
        if let Some(image_id) = self.selected_image_id {
            // Ensure entry exists
            self.history_map.entry(image_id).or_insert_with(|| {
                // Initial state is the current params
                (vec![self.current_edit_params], 0)
            });

            self.history_map.get_mut(&image_id)
        } else {
            None
        }
    }

    /// The params the main view renders: the user's edit, except that noise
    /// reduction stays where it was while a drag on its sliders is held back
    /// (`held_noise`). Everything that pushes the edit to the GPU uses this;
    /// what is saved, exported and undone is `current_edit_params`.
    pub fn view_params(&self) -> crate::core::types::EditParams {
        let mut p = self.current_edit_params;
        if let Some(h) = self.held_noise.filter(|h| h.image_id == self.selected_image_id) {
            p.luma_noise = h.luma;
            p.luma_detail = h.detail;
            p.color_noise = h.color;
        }
        p
    }

    /// Helper to push current state to history.
    ///
    /// Also flushes pending edits to SQLite: a history commit marks the end of
    /// a gesture, which is exactly when the params are worth persisting. Tying
    /// the two together means any future commit site gets persistence for free
    /// and the two can't drift apart.
    pub fn commit_current_state(&mut self) {
        self.stage_drag = false;
        self.flush_edits();
        let params_to_push = self.current_edit_params;
        let mut pushed = false;
        if let Some((stack, index)) = self.get_current_history() {
            // A release that changed nothing — clicking a slider without
            // dragging, grabbing a crop handle and letting go — must not add a
            // step, or the next Undo visibly does nothing. `is_cropping` is
            // tool state, so entering crop mode alone isn't an edit either.
            let unchanged = stack
                .get(*index)
                .is_some_and(|top| top.without_tool_state() == params_to_push.without_tool_state());
            if unchanged {
                return;
            }

            // Truncate any redo history
            stack.truncate(*index + 1);

            // Push new state
            stack.push(params_to_push);
            *index += 1;
            pushed = true;
            // Each step is ~21 KB: keep the newest HISTORY_LIMIT per image
            // (the oldest undo steps go first).
            if stack.len() > HISTORY_LIMIT {
                let excess = stack.len() - HISTORY_LIMIT;
                stack.drain(..excess);
                *index -= excess;
            }

            tracing::debug!(
                "History Commit: Stack size {}, Index {}",
                stack.len(),
                *index
            );
        }
        if pushed {
            self.last_commit = Some(std::time::Instant::now());
        }
    }

    /// A double-click reset's commit. The first press of the double click
    /// already pushed an undo step (the slider jumped to the click); if that
    /// was moments ago, fold the reset into it, so one Undo returns to the
    /// value the user had. Otherwise it is an ordinary commit.
    pub fn commit_amending_last(&mut self) {
        let recent = self.last_commit.is_some_and(|t| t.elapsed() < SLIDER_AMEND_WINDOW);
        if !recent {
            return self.commit_current_state();
        }
        self.last_commit = None;
        self.stage_drag = false;
        self.flush_edits();
        let params = self.current_edit_params;
        if let Some((stack, index)) = self.get_current_history() {
            if *index == 0 {
                return;
            }
            stack[*index] = params;
            // The reset landed back on where the user started: no step at all.
            if stack[*index - 1].without_tool_state() == params.without_tool_state() {
                stack.remove(*index);
                *index -= 1;
            }
        }
    }

    /// Mirror the crop tool's on/off state into the params the shader reads.
    ///
    /// `is_cropping` rides in `EditParams` only because that is what reaches
    /// the GPU. Anything that swaps the params wholesale — undo/redo, reset,
    /// loading another image's edits — would otherwise leave the shader and
    /// the crop overlay disagreeing about whether crop mode is on.
    pub fn sync_tool_flags(&mut self) {
        self.current_edit_params.is_cropping = u32::from(self.is_cropping);
        // The mask overlay shows while the user asks for it, or while a
        // stroke is being painted (you need to see what you are painting).
        self.current_edit_params.mask_overlay = match self.selected_mask {
            Some(i) if (self.show_mask_overlay || self.is_painting || self.ai_select.is_some())
                && i < self.current_edit_params.mask_count as usize =>
            {
                i as u32 + 1
            }
            _ => 0,
        };
    }

    /// True while the mask tool wants the preview canvas (placement mode or a
    /// selected mask with visible handles) — suppresses panning, like crop.
    pub fn mask_overlay_active(&self) -> bool {
        self.mask_tool != MaskTool::Inactive || self.selected_mask.is_some()
    }

    /// Size of the pixel buffer the develop viewport currently holds.
    ///
    /// This is the RENDER TARGET size, which since the windowed render is no
    /// longer the image's shape: at zoom it covers only the visible slice.
    /// Use it to size textures and nothing else — for anything that reasons
    /// about where the image sits on screen, use `image_display_dims`.
    /// MUST mirror the fallback priority in `views/develop.rs`
    /// (rendered > working > placeholder).
    pub fn buffer_dims(&self) -> (u32, u32) {
        if self.rendered_preview_bytes.is_some() {
            self.rendered_preview_dims
        } else if self.working_preview_bytes.is_some() {
            self.working_preview_dims
        } else {
            PLACEHOLDER_DIMS
        }
    }

    /// The images the CURRENT TAB is showing, in display order.
    ///
    /// The one place the per-tab filter predicates live. They differ, and the
    /// differences are deliberate:
    ///
    /// | Tab | rating filter | folder filter | marked for removal |
    /// |-----|---------------|---------------|--------------------|
    /// | Library | yes | yes | visible |
    /// | Cull | yes | no | visible |
    /// | Develop | yes | no | **excluded** |
    ///
    /// A mark is stored as 200 + stars (`models::is_marked`), which satisfies `>=` against any
    /// real 0-5 threshold, so marked images survive the rating filter — that
    /// is intentional, and why Develop needs its own explicit exclusion.
    ///
    /// Anything that reasons about "the images on screen" must come through
    /// here: the three views that render lists, range selection, select-all,
    /// and arrow-key navigation. Re-inlining the predicate is how the four
    /// drift apart.
    pub fn displayed_images(&self) -> Vec<&ImageData> {
        self.images
            .iter()
            .filter(|img| self.min_filter_rating == 0 || img.rating >= self.min_filter_rating)
            .filter(|img| {
                if self.current_tab != AppTab::Library {
                    return true;
                }
                self.library_folder_filter
                    .as_ref()
                    .map(|folder| path_is_within(&img.path, folder))
                    .unwrap_or(true)
            })
            // Marked-for-removal is hidden where acting on it would be wrong:
            // Develop refuses to edit them, and Export refuses to write out
            // images the user has flagged for deletion.
            .filter(|img| {
                !matches!(self.current_tab, AppTab::Develop | AppTab::Export)
                    || !crate::database::models::is_marked(img.rating)
            })
            .collect()
    }

    /// RAW metadata for the selected image, or `None` when what is held
    /// belongs to some other image (or nothing has been loaded yet).
    pub fn selected_metadata(&self) -> Option<&raw::loader::RawDataResult> {
        if self.current_metadata_id.is_some() && self.current_metadata_id == self.selected_image_id
        {
            self.current_metadata.as_ref()
        } else {
            None
        }
    }

    /// Ids of [`Self::displayed_images`], in the same order.
    pub fn displayed_ids(&self) -> Vec<i64> {
        self.displayed_images().iter().map(|i| i.id).collect()
    }

    /// The images a rate / flag / delete action applies to: the whole
    /// multi-selection when there is one, else the current image.
    pub fn action_target_ids(&self) -> Vec<i64> {
        if !self.multi_selection.is_empty() {
            self.multi_selection.iter().copied().collect()
        } else {
            self.selected_image_id.into_iter().collect()
        }
    }

    /// Apply `f` to the in-memory catalogue rows for `ids`.
    ///
    /// One pass over the catalogue with a set lookup — the per-id
    /// `iter().find()` this replaces was O(selection × catalogue) on every
    /// select-all rating.
    pub fn apply_to_images(&mut self, ids: &[i64], mut f: impl FnMut(&mut ImageData)) {
        let ids: HashSet<i64> = ids.iter().copied().collect();
        for img in self.images.iter_mut().filter(|img| ids.contains(&img.id)) {
            f(img);
        }
    }

    /// Forget the edited proxies of `ids`, whose saved edits just changed
    /// without being rendered — see `handlers::proxy`. Their thumbnails fall
    /// back to the camera's until they are next developed. The stale files
    /// are swept at the next launch (`Library::sweep_edited_proxies`).
    pub fn invalidate_proxies(&mut self, ids: &[i64]) {
        for id in ids {
            self.edited_proxies.pop(id);
        }
        let wanted: HashSet<i64> = ids.iter().copied().collect();
        let had_saved: Vec<i64> = self
            .images
            .iter()
            .filter(|img| img.cache_path_edited.is_some() && wanted.contains(&img.id))
            .map(|img| img.id)
            .collect();
        if had_saved.is_empty() {
            return;
        }
        if let Some(lib) = &self.library {
            if let Err(e) = lib.clear_edited_proxies(&had_saved) {
                tracing::warn!("Could not clear stale edited thumbnails: {e}");
            }
        }
        self.apply_to_images(&had_saved, |img| img.cache_path_edited = None);
    }

    /// True when the selected image's edits are being rendered, so its proxy
    /// follows them on its own.
    fn renders_selected_image(&self) -> bool {
        self.current_tab == AppTab::Develop
            && self.selected_image_id.is_some()
            && self.image_resources.as_ref().map(|r| r.image_id) == self.selected_image_id
    }

    /// True when this image should render as selected.
    ///
    /// The single predicate for both the library grid and the filmstrip. They
    /// used to disagree — the grid ORed `selected_image_id` with
    /// `multi_selection`, the filmstrip read only `multi_selection` — which
    /// happened to look consistent only because a plain click kept the two in
    /// sync. Range selection breaks that assumption.
    pub fn is_selected(&self, image_id: i64) -> bool {
        self.selected_image_id == Some(image_id) || self.multi_selection.contains(&image_id)
    }

    /// Display-space (EXIF-oriented) dimensions of what is actually ON SCREEN.
    ///
    /// The single dims source for all on-screen geometry: letterbox fit,
    /// zoom-to-cursor, crop and mask drag math, and the canvas overlay. Only
    /// the ASPECT actually matters to `core::viewport::image_rect`, which is
    /// why taking it from the GPU resources is safe even while a preview of a
    /// different resolution is on screen — and why it must not come from the
    /// render buffer, whose aspect now tracks the viewport rather than the
    /// image.
    ///
    /// **Crop-aware.** Outside crop mode the viewport shows the crop sub-rect,
    /// so these are the CROPPED dimensions. Returning the full frame's here
    /// made the letterbox reserve the uncropped aspect and the shader stretch
    /// the crop to fill it — a 16:9 crop of a 3:2 frame rendered vertically
    /// squashed. (Export was unaffected: it renders the crop to its own
    /// correctly-sized target.) In crop mode the shader deliberately reveals
    /// the whole sensor so the handles can be dragged over it, so the full
    /// frame genuinely is what's displayed and the crop must NOT be applied.
    ///
    /// **Before the GPU resources exist** (the RAW is still loading) what is on
    /// screen is the placeholder, shown in its own shape: an edited proxy is
    /// already cropped, and the camera JPEG never is. Applying the crop here
    /// squeezed the full camera frame into the crop's aspect for as long as
    /// the load took — and would crop an already-cropped proxy a second time.
    pub fn image_display_dims(&self) -> (u32, u32) {
        let Some(res) = &self.image_resources else {
            return if self.working_preview_bytes.is_some() {
                self.working_preview_dims
            } else {
                PLACEHOLDER_DIMS
            };
        };
        let (w, h) = res.oriented_dims(self.current_edit_params.quarter_turns);
        if self.is_cropping {
            return (w, h);
        }
        crop_dims(w, h, self.current_edit_params.crop)
    }

    /// Magnification as a photographer reads it: 100% means one full-resolution
    /// sensor pixel per physical screen pixel.
    ///
    /// Distinct from `self.zoom`, which is relative to fit — at fit a 6000px
    /// image on a 1900px viewport is `zoom == 1.0` but only ~32%.
    pub fn zoom_percent(&self) -> f32 {
        let (dw, dh) = self.image_display_dims();
        let (vw, vh) = self.viewport_size;
        let (fw, _) = crate::core::viewport::fitted_size(dw, dh, vw, vh);
        // Against the FULL-resolution width: the develop texture may be a
        // subsampled preview, and reporting against that would claim 100%
        // while showing half the detail.
        let full_w = self
            .image_resources
            .as_ref()
            .map(|r| r.oriented_original_dims(self.current_edit_params.quarter_turns).0)
            .unwrap_or(dw)
            .max(1) as f32;
        // `fw` spans the CROPPED region (image_display_dims is crop-aware), so
        // the sensor pixels it covers are only that fraction of the full width.
        let visible_w = if self.is_cropping {
            full_w
        } else {
            (full_w * self.current_edit_params.crop[2]).max(1.0)
        };
        fw * self.zoom * self.scale_factor / visible_w * 100.0
    }

    /// Helper to save current edit parameters to database
    /// Signal that the preview pixel buffer's contents changed, so the viewport
    /// shader re-uploads its texture on the next redraw. Must be called
    /// wherever `working_preview_bytes` or `rendered_preview_bytes` is
    /// replaced — a missed call shows a stale image.
    pub fn bump_preview_generation(&mut self) {
        self.preview_generation = self.preview_generation.wrapping_add(1);
        // 0 is reserved for the shader's 1x1 placeholder.
        if self.preview_generation == 0 {
            self.preview_generation = 1;
        }
    }

    /// Mark the current image's edits as needing persistence.
    ///
    /// Called from the per-slider-tick path (`update_pipeline`). Writing to
    /// SQLite there directly meant a JSON serialize + SELECT + UPDATE + fsync
    /// per mouse-move event, on the UI thread; the flag defers that to the
    /// commit points below.
    pub fn mark_edits_dirty(&mut self) {
        self.edits_dirty = true;
    }

    /// Persist the current image's edits if anything changed since the last
    /// flush. Cheap and safe to call unconditionally at any commit point.
    ///
    /// Flush points must cover every way the current params can stop being
    /// current: slider release (`CommitEdit`), drag release, image switch, tab
    /// switch, and window close. Missing one loses the last gesture's edits.
    pub fn flush_edits(&mut self) {
        if !self.edits_dirty {
            return;
        }
        // Clear first: a failed write should not spin on every later flush.
        self.edits_dirty = false;
        self.save_current_edits();
        // Edits saved with no render behind them (undo, reset or paste in
        // Library or Cull): the image's proxy now shows something else.
        if !self.renders_selected_image() {
            if let Some(id) = self.selected_image_id {
                self.invalidate_proxies(&[id]);
            }
        }
    }

    /// Unconditional write, bypassing the dirty flag. Prefer `flush_edits`;
    /// this exists for callers that write params for an image other than the
    /// selected one, or that must persist regardless of flag state.
    pub fn save_current_edits(&self) {
        // Phase 23: Only save if database is loaded
        if let Some(library) = &self.library {
            if let Some(image_id) = self.selected_image_id {
                if let Err(e) = library.save_edit_params(image_id, &self.current_edit_params) {
                    tracing::error!("Failed to save edits for image {}: {:?}", image_id, e);
                }
            }
        }
    }

    /// Phase 66: Helper to calculate a center crop for a target aspect ratio
    /// Returns [x, y, w, h] in normalized coordinates (0.0 to 1.0)
    pub fn calculate_center_crop(target_ratio: f32, image_w: u32, image_h: u32) -> [f32; 4] {
        let image_ratio = image_w as f32 / image_h as f32;

        if image_ratio > target_ratio {
            // Image is wider than target: Crop width (sides)
            // h = 1.0, w = target / image
            let w = target_ratio / image_ratio;
            let x = (1.0 - w) / 2.0;
            [x, 0.0, w, 1.0]
        } else {
            // Image is taller than target: Crop height (top/bottom)
            // w = 1.0, h = image / target
            let h = image_ratio / target_ratio;
            let y = (1.0 - h) / 2.0;
            [0.0, y, 1.0, h]
        }
    }

    fn raw_cache_entry_bytes(raw: &raw::loader::RawDataResult) -> usize {
        raw.data.len()
            .saturating_mul(std::mem::size_of::<u16>())
            .saturating_add(1024)
    }

    pub fn raw_cache_budget_bytes(&self) -> usize {
        (self.raw_preload_budget_mb as usize).saturating_mul(1024 * 1024)
    }

    /// Headroom to keep free system-wide, in bytes. `0` = check disabled.
    /// Mirrors `raw_cache_budget_bytes`'s shape deliberately.
    pub fn min_free_bytes(&self) -> u64 {
        (self.min_free_ram_mb as u64).saturating_mul(1024 * 1024)
    }

    /// Drop everything regenerable the app is holding, and let wgpu hand back
    /// what has already been released.
    ///
    /// Called under memory pressure. These caches exist purely to make
    /// navigation fast; rebuilding them costs time, whereas running the machine
    /// out of RAM costs the user their session.
    pub fn release_discretionary_memory(&mut self) {
        self.preview_cache.clear();
        self.raw_cache.clear();
        self.raw_cache_bytes = 0;
        if let Some(ctx) = &self.gpu_context {
            ctx.reclaim();
        }
    }

    pub fn evict_raw_cache_to_budget(&mut self) {
        let budget = self.raw_cache_budget_bytes();
        while self.raw_cache_bytes > budget {
            if let Some((_id, raw)) = self.raw_cache.pop_lru() {
                self.raw_cache_bytes = self
                    .raw_cache_bytes
                    .saturating_sub(Self::raw_cache_entry_bytes(&raw));
            } else {
                self.raw_cache_bytes = 0;
                break;
            }
        }
    }

    /// The decoded RAW of `image_id` if memory holds it: the open image's pin
    /// first, then the preload cache. Does not touch LRU order.
    pub fn raw_for(&self, image_id: i64) -> Option<Arc<raw::loader::RawDataResult>> {
        match &self.current_raw {
            Some((id, raw)) if *id == image_id => Some(raw.clone()),
            _ => self.raw_cache.peek(&image_id).cloned(),
        }
    }

    pub fn insert_raw_cache(&mut self, image_id: i64, raw: Arc<raw::loader::RawDataResult>) {
        if self.raw_preload_budget_mb == 0 {
            return;
        }

        if let Some(old) = self.raw_cache.get(&image_id) {
            self.raw_cache_bytes = self
                .raw_cache_bytes
                .saturating_sub(Self::raw_cache_entry_bytes(old));
        }

        self.raw_cache_bytes = self
            .raw_cache_bytes
            .saturating_add(Self::raw_cache_entry_bytes(&raw));
        self.raw_cache.put(image_id, raw);
        self.evict_raw_cache_to_budget();
    }

    /// Drop a deleted image's entry from the raw cache, keeping
    /// `raw_cache_bytes` accounting correct.
    pub fn remove_from_raw_cache(&mut self, image_id: i64) {
        if self.current_raw.as_ref().is_some_and(|(id, _)| *id == image_id) {
            self.current_raw = None;
        }
        if let Some(old) = self.raw_cache.pop(&image_id) {
            self.raw_cache_bytes = self
                .raw_cache_bytes
                .saturating_sub(Self::raw_cache_entry_bytes(&old));
        }
    }
}
