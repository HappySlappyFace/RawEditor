//! Selecting and opening images, switching tabs, and keeping the load
//! queues full of what the user will reach next. (Modifier keys, the range
//! anchor, scrolling the selection into view and the pointer are the app's.)

use std::sync::Arc;

use super::{load_proxy, CachedPreview, Effects, Session, SessionEvent, MAX_CONCURRENT_PREVIEW_LOADS, MAX_CONCURRENT_RAW_LOADS};
use crate::session::AppTab;
use crate::session::EditorReadiness;
use crate::database::models::Image as ImageData;
use crate::raw;
use super::edit_key;

/// Where the selection went when the session moved it.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moved {
    /// It stayed where it was.
    Stay,
    /// This image should open next (as the app's own move, not a click).
    To(i64),
    /// Nothing is left to select.
    Cleared,
}

/// How a click changes the multi-selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionChange {
    /// Just this image.
    Only,
    /// Add it, or take it out if it was in.
    Toggle,
    /// These images (a range, in display order); `add` keeps what was
    /// selected before.
    Range { ids: Vec<i64>, add: bool },
}

/// What the camera-preview image (Cull, and Develop before its first
/// render) should show (`Session::camera_preview`). The front end holds
/// that picture in its own form.
#[derive(Debug, Clone)]
pub enum PreviewHandle {
    /// Nothing yet.
    Clear,
    /// These pixels, from the RAM cache.
    Cached(CachedPreview),
    /// The working-preview file at this path.
    Path(String),
}

/// What switching to a tab calls for.
#[must_use]
pub enum TabLoad {
    Nothing,
    /// The Develop selection is marked for removal: it moved.
    Moved(Moved),
    Load(Effects),
}

/// Decode `image_id`'s RAW for Develop.
fn decode_raw(image_id: i64, path: String) -> Effects {
    Effects::perform("raw decode", async move {
        SessionEvent::RawDataLoaded(image_id, raw::loader::load_raw_data(path).await)
    })
}

/// Read `image_id`'s working preview (the camera JPEG) for display.
fn load_working_preview(image_id: i64, path: String) -> Effects {
    Effects::perform("working preview", async move {
        match load_preview_pixels(path.clone()).await {
            Ok((w, h, pixels)) => SessionEvent::WorkingPreviewReady(image_id, Some(Arc::from(pixels)), (w, h), path),
            Err(_) => SessionEvent::WorkingPreviewReady(image_id, None, crate::session::PLACEHOLDER_DIMS, path),
        }
    })
}

/// Take `id` out of the not-yet-started part of a load queue.
///
/// Returns true if it was waiting there (so the caller should load it itself).
/// The first `in_flight` entries are the running decodes: those are left alone
/// and report false, since their result will arrive on its own.
fn claim_queued(
    queue: &mut Vec<(i64, String)>,
    pending: &mut std::collections::HashSet<i64>,
    id: i64,
    in_flight: usize,
) -> bool {
    let Some(pos) = queue.iter().position(|(queued, _)| *queued == id) else {
        return false;
    };
    if pos < in_flight {
        return false; // already running
    }
    queue.remove(pos);
    pending.remove(&id);
    true
}

/// Replace a load queue's not-yet-started tail with `wanted`, in order.
///
/// The loader subscription runs the first `in_flight` entries, so those stay
/// put — dropping one would throw away a decode already under way. The rest
/// is rebuilt from scratch: fast arrow-key navigation used to append a fresh
/// window per keypress and then grind through every stale one in turn.
fn requeue(
    queue: &mut Vec<(i64, String)>,
    pending: &mut std::collections::HashSet<i64>,
    wanted: Vec<(i64, String)>,
    in_flight: usize,
) {
    let keep = in_flight.min(queue.len());
    for (id, _) in queue.drain(keep..) {
        pending.remove(&id);
    }
    for (id, path) in wanted {
        if pending.insert(id) {
            queue.push((id, path));
        }
    }
}

async fn load_preview_pixels(path: String) -> Result<(u32, u32, Vec<u8>), String> {
    tokio::task::spawn_blocking(move || {
        let img = image::open(&path).map_err(|e| e.to_string())?;
        let rgba = img.to_rgba8();
        Ok((rgba.width(), rgba.height(), rgba.into_raw()))
    }).await.map_err(|e| e.to_string())?
}

impl Session {
    /// The user is leaving the selected image for `image_id` (or re-selecting
    /// it): save what it has unsaved and bring its edited thumbnail up to date.
    /// Must run BEFORE the selection moves: `flush_edits` writes
    /// `current_edit_params` against the currently-selected id, so flushing after
    /// the switch would write this image's edits onto the next one.
    pub fn leave_image(&mut self, image_id: i64) -> Effects {
        self.flush_edits();
        // Leaving an image: bring its edited thumbnail on disk up to date. Also
        // before the selection moves — it checks against the outgoing edits.
        let persist = if self.selected_image_id != Some(image_id) {
            self.persist_proxy()
        } else {
            Effects::none()
        };
        // The peek belongs to the outgoing image; carrying it across would show
        // the new image unedited with no way to notice why.
        self.before_peek = false;
        persist
    }

    /// Select `image_id` (`change` says how it joins the selection) and open
    /// it: the outgoing image is saved first, and an AI-select session on it
    /// is ended while the params are still its own.
    pub fn select_image(&mut self, image_id: i64, change: SelectionChange) -> Effects {
        let ai_done = if self.selected_image_id != Some(image_id) {
            self.ai_select_done()
        } else {
            Effects::none()
        };
        // Saves the outgoing image's edits and thumbnail: before anything moves.
        let persist = self.leave_image(image_id);
        self.change_selection(image_id, change);
        let load = self.open_image(image_id);
        Effects::batch([ai_done, persist, load])
    }

    pub fn change_selection(&mut self, image_id: i64, change: SelectionChange) {
        match change {
            SelectionChange::Only => {
                self.multi_selection.clear();
                self.multi_selection.insert(image_id);
            }
            SelectionChange::Toggle => {
                if !self.multi_selection.remove(&image_id) {
                    self.multi_selection.insert(image_id);
                }
            }
            SelectionChange::Range { ids, add } => {
                if !add {
                    self.multi_selection.clear();
                }
                self.multi_selection.extend(ids);
            }
        }
    }

    /// Make `image_id` the open image: its saved edit, its history, and — in
    /// Develop or Cull, unless it is already loaded or loading — its load.
    pub fn open_image(&mut self, image_id: i64) -> Effects {
        // `Loading(id)` promises a load of `id` that will finish it, and only a
        // Develop selection starts one. Selecting another image anywhere else
        // (Library, Cull) loads nothing, and the old image's result then lands
        // unwanted — so coming back to it later found "already loading", started
        // nothing, and Develop sat on "Loading" forever. Drop the promise with
        // the selection; entering Develop on an image loads it afresh (a decode
        // still running is reused, not repeated: `direct_raw_loads`).
        if self.current_tab != AppTab::Develop
            && matches!(self.editor_readiness, EditorReadiness::Loading(id) if id != image_id)
        {
            self.editor_readiness = EditorReadiness::NoSelection;
        }
        self.selected_image_id = Some(image_id);
        // Mask selection is an index into the previous image's mask list — clear
        // both it and any pending placement mode when the image changes.
        self.selected_mask = None;
        self.mask_tool = crate::session::MaskTool::Inactive;
        // The overlay was drawn for the previous image.
        self.changed.insert(super::Changed::OVERLAY);

        if let Some(library) = &self.library {
            self.current_edit_params = match library.load_saved_edit(image_id) {
                Ok(saved) => saved.unwrap_or_default(),
                Err(e) => {
                    // The library backs the old JSON up before the next flush
                    // replaces it with these defaults.
                    tracing::error!("Could not read the saved edit for image {image_id}: {e}");
                    self.status =
                        "This image's saved edit could not be read, so it opens unedited. The old edit is kept as a backup.".to_string();
                    Default::default()
                }
            };
            // Crop mode survives the switch, so the freshly loaded params (which
            // never carry the flag) must pick it up before anything renders them.
            self.sync_tool_flags();
            self.history_map.entry(image_id).or_insert_with(|| (vec![self.current_edit_params], 0));
            // A proxy read from disk matches the saved edits just loaded — record
            // which edit it shows, so leaving can tell if it has fallen behind.
            let key = edit_key(&self.current_edit_params);
            if let Some(proxy) = self.edited_proxies.peek_mut(&image_id) {
                proxy.params.get_or_insert(key);
            }
        }


        if self.current_tab == AppTab::Develop || self.current_tab == AppTab::Cull {
            let needs_load = match &self.editor_readiness {
                EditorReadiness::Ready(id) => *id != image_id,
                EditorReadiness::Loading(id) => *id != image_id,
                _ => true,
            };
            if needs_load {
                return self.load_image(image_id);
            }
        }
        Effects::none()
    }

    /// Switch to `tab`: save what is unsaved, and (leaving Develop) the
    /// image's thumbnail. The load the tab calls for is [`Self::load_for_tab`].
    pub fn set_tab(&mut self, tab: AppTab) -> Effects {
        // Leaving Develop mid-gesture must not strand unsaved slider edits.
        self.flush_edits();
        // Leaving Develop is leaving the image, as far as its thumbnail goes.
        let persist = if self.current_tab == AppTab::Develop && tab != AppTab::Develop {
            self.persist_proxy()
        } else {
            Effects::none()
        };
        // Space only peeks in Develop; leaving with it held would strand the flag.
        self.before_peek = false;
        self.current_tab = tab;
        persist
    }

    /// The load (if any) that switching to `tab` calls for.
    pub fn load_for_tab(&mut self, tab: AppTab) -> TabLoad {
        if tab == AppTab::Develop {
            // An image could have been marked for removal while a different tab
            // was active — the tab switch itself has no rating-awareness
            // otherwise, so a stale marked selection would silently load and
            // display in Develop without this check.
            if self.selection_is_marked() {
                return TabLoad::Moved(self.ensure_selection_not_marked());
            }
            if let Some(image_id) = self.selected_image_id {
                let needs_load = match &self.editor_readiness {
                    EditorReadiness::Ready(id) => *id != image_id,
                    EditorReadiness::Loading(id) => *id != image_id,
                    _ => true,
                };
                if needs_load {
                    return TabLoad::Load(self.load_image(image_id));
                }
            }
        } else if tab == AppTab::Cull {
            // Library selection doesn't load previews, so the camera preview can
            // still hold the image Cull last showed — and Cull displayed exactly
            // that, under the new selection's name. Reloading is cheap: a RAM
            // cache hit in the common case, and it queues the neighbours too.
            if let Some(image_id) = self.selected_image_id {
                return TabLoad::Load(self.load_image(image_id));
            }
        }
        TabLoad::Nothing
    }

    /// Ctrl+A — select every image the current tab is displaying.
    pub fn select_all(&mut self) {
        let ids = self.displayed_ids();
        if ids.is_empty() {
            return;
        }
        self.status = format!("{} images selected", ids.len());
        self.multi_selection = ids.into_iter().collect();
    }

    /// Cyclic search starting just after `from_id` for the next image that is
    /// NOT marked for removal. Bounded to `images.len()` steps, so it can never
    /// infinite-loop even if every image (including `from_id`) is marked.
    pub fn find_next_unmarked_image_id(&self, from_id: i64) -> Option<i64> {
        let idx = self.images.iter().position(|i| i.id == from_id)?;
        let len = self.images.len();
        // Prefer an image the active filter shows, so the filmstrip has a
        // selection to draw; fall back to any unmarked one rather than none.
        let shown: std::collections::HashSet<i64> = self.displayed_ids().into_iter().collect();
        let mut any = None;
        for step in 1..=len {
            let candidate = &self.images[(idx + step) % len];
            if crate::database::models::is_marked(candidate.rating) {
                continue;
            }
            if shown.contains(&candidate.id) {
                return Some(candidate.id);
            }
            any.get_or_insert(candidate.id);
        }
        any
    }

    /// True when Develop is active and its current selection is marked for
    /// removal — the condition `ensure_develop_selection_not_marked` acts on.
    pub fn selection_is_marked(&self) -> bool {
        if self.current_tab != AppTab::Develop {
            return false;
        }
        let Some(sel) = self.selected_image_id else {
            return false;
        };
        self
            .images
            .iter()
            .find(|i| i.id == sel)
            .map(|i| crate::database::models::is_marked(i.rating))
            .unwrap_or(false)
    }

    /// If Develop is the active tab and its selection is marked for removal,
    /// navigate to the next unmarked image, or clear the selection if none
    /// exists. Called both right after marking an image (delete) and
    /// when switching into the Develop tab (above), since marking most commonly
    /// happens from Library/Cull, not Develop itself.
    pub fn ensure_selection_not_marked(&mut self) -> Moved {
        if !self.selection_is_marked() {
            return Moved::Stay;
        }
        let Some(sel) = self.selected_image_id else {
            return Moved::Stay;
        };

        if let Some(next_id) = self.find_next_unmarked_image_id(sel) {
            Moved::To(next_id)
        } else {
            self.selected_image_id = None;
            self.editor_readiness = EditorReadiness::NoSelection;
            self.camera_preview = Some(PreviewHandle::Clear);
            Moved::Cleared
        }
    }

    /// Step through the images the current tab is DISPLAYING.
    ///
    /// Previously these indexed the unfiltered `self.images`, so arrow keys
    /// happily navigated to images the active filter was hiding — and in Develop,
    /// to images marked for removal that the tab refuses to show.
    pub fn step_selection(&self, forward: bool) -> Option<i64> {
        let ids = self.displayed_ids();
        if ids.is_empty() {
            return None;
        }
        let current = self.selected_image_id?;
        let idx = ids.iter().position(|id| *id == current)?;
        let next = if forward {
            (idx + 1) % ids.len()
        } else if idx == 0 {
            ids.len() - 1
        } else {
            idx - 1
        };
        Some(ids[next])
    }

    /// A working preview (the camera JPEG) was read: pixels (`None` when they
    /// could not be decoded) of `dims`, from the file at `path`. The selected
    /// image's goes on screen.
    pub fn working_preview_ready(&mut self, id: i64, bytes: Option<std::sync::Arc<[u8]>>, dims: (u32, u32), path: String) {
        // Populate the look-ahead cache even when this is no longer the selected
        // image: the decode already happened, and caching it makes navigating
        // *back* free too, not just forward into preloaded territory.
        if let Some(bytes) = &bytes {
            self.preview_cache.put(
                id,
                CachedPreview {
                    width: dims.0,
                    height: dims.1,
                    bytes: bytes.clone(),
                },
            );
        }
        if Some(id) == self.selected_image_id {
            self.camera_preview = Some(match &bytes {
                Some(bytes) => PreviewHandle::Cached(CachedPreview { width: dims.0, height: dims.1, bytes: bytes.clone() }),
                None => PreviewHandle::Path(path),
            });
            self.working_preview_bytes = bytes;
            self.bump_preview_generation();
            self.working_preview_dims = dims;
        }
    }

    pub fn preview_cached(&mut self, id: i64, result: Result<(u32, u32, std::sync::Arc<[u8]>), String>) {
        // Phase 78: Cleanup pending load
        self.pending_loads.remove(&id);
        // Phase 81: Cleanup queued load
        self.queued_loads.retain(|(i, _)| *i != id);

        if let Ok((width, height, bytes)) = result {
            // Store the decoded pixels themselves. Previously this built a Handle
            // from `pixels.to_vec()` — copying the buffer back out of the Arc — and
            // dropped the Arc, so the Develop viewport had nothing to use and
            // re-read the JPEG from disk on navigation.
            self.preview_cache.put(
                id,
                CachedPreview { width, height, bytes },
            );
        }
    }

    /// Decode the selected image again. Its lens profile is looked up when the
    /// RAW is read, so after the lens database changes the loaded one is stale.
    /// The edits are untouched — only the decode is redone.
    pub fn reload_selected_image(&mut self) -> Effects {
        let Some(image_id) = self.selected_image_id else {
            return Effects::none();
        };
        if self.current_tab == AppTab::Develop {
            self.load_image(image_id)
        } else {
            // Not on screen: entering Develop decodes it afresh.
            self.image_resources = None;
            self.preview_resources = None;
            self.editor_readiness = EditorReadiness::NoSelection;
            Effects::none()
        }
    }

    /// Start loading `image_id` for display; what the camera-preview image
    /// shows meanwhile is set on `camera_preview`.
    pub fn load_image(&mut self, image_id: i64) -> Effects {
        let mut preview = PreviewHandle::Clear;
        // `full_res_upgrading` is deliberately NOT reset: an upgrade of the
        // previous image may still be building, and starting this one's
        // alongside it would hold two full-size working sets. Its result
        // releases the guard and re-checks this image (`Session::full_res_ready`).
        // The pin belongs to the image being left.
        if self.current_raw.as_ref().is_some_and(|(id, _)| *id != image_id) {
            self.current_raw = None;
        }
        let develop = self.current_tab == AppTab::Develop;
        if develop {
            // The outgoing image's GPU resources and render must not stay on
            // screen: `rendered_preview_bytes` outranks the working preview in the
            // viewport, so the PREVIOUS photo used to sit there under the new
            // filename until the new RAW finished decoding. Dropping both lets the
            // instant JPEG show immediately, frees the old textures now, and keeps
            // any slider touched mid-load from rendering the old image.
            self.image_resources = None;
            self.rendered_preview_bytes = None;
            self.preview_resources = None;
            self.rendered_view_rect = crate::gpu::params::FULL_VIEW_RECT;
            self.requested_view_rect = crate::gpu::params::FULL_VIEW_RECT;
            self.current_metadata = None;
            self.bump_preview_generation();
        }
        if let Some((working_cache_path, raw_path, saved_proxy)) = self
            .images
            .iter()
            .find(|i| i.id == image_id)
            .map(|img| (img.cache_path_working.clone(), img.path.clone(), img.cache_path_edited.clone()))
        {
            // Develop shows the image's edited proxy while the RAW loads, not the
            // camera JPEG: that ignores the edit, so the image visibly jumped (in
            // exposure, colour, crop) when the real render landed. A proxy in
            // memory goes up at once; a saved one is a ~1 ms decode away.
            let proxy = develop
                .then(|| self.edited_proxies.get(&image_id).map(|p| p.preview.clone()))
                .flatten();
            let saved_proxy = saved_proxy.filter(|_| develop && proxy.is_none());
            let camera_preview_shown = proxy.is_none() && saved_proxy.is_none();
            if let Some(preview) = &proxy {
                self.show_as_placeholder(preview);
            }

            // Phase 73: Check RAM cache first. A hit is the whole point of the
            // preloader — it supplies the decoded pixels directly, so the disk
            // read + JPEG decode below is skipped entirely.
            let cached_preview = self.preview_cache.get(&image_id).cloned();
            if let Some(cached) = &cached_preview {
                preview = PreviewHandle::Cached(cached.clone());
                if camera_preview_shown {
                    self.working_preview_bytes = Some(cached.bytes.clone());
                    self.working_preview_dims = (cached.width, cached.height);
                    self.bump_preview_generation();
                }
            } else {
                if proxy.is_none() {
                    self.working_preview_bytes = None;
                }
                if let Some(path) = &working_cache_path {
                    preview = PreviewHandle::Path(path.clone());
                }
            }

            // `Loading` promises a RAW load is in flight for this image, and only
            // Develop starts one. Setting it from Cull too left the promise
            // unfulfilled: switching Cull → Develop then saw "already loading",
            // started nothing, and sat on "Loading RAW…" forever.
            if develop {
                self.editor_readiness = EditorReadiness::Loading(image_id);
            }
            let mut effects = Vec::new();
            if let Some(path) = saved_proxy {
                effects.push(load_proxy(image_id, path));
            }
            // Cull displays the camera JPEG (and wants it cached either way);
            // Develop only needs it when there is no proxy to show instead.
            if cached_preview.is_none() && (!develop || camera_preview_shown) {
                if let Some(path) = &working_cache_path {
                    effects.push(load_working_preview(image_id, path.clone()));
                }
            }

            // Only load full RAW data if we are in Develop mode
            if develop {
                let cached_raw = self.raw_cache.get(&image_id).cloned();
                if let Some(cached_raw) = cached_raw {
                    effects.push(self.prepare_image_resources_from_raw(image_id, cached_raw.clone()));
                } else if !self.pending_raw_loads.contains(&image_id) {
                    // One direct decode per image: stepping A -> B -> A used to
                    // decode A twice, and a held arrow key one image per step.
                    if self.direct_raw_loads.insert(image_id) {
                        effects.push(decode_raw(image_id, raw_path));
                    }
                } else if claim_queued(
                    &mut self.queued_raw_loads,
                    &mut self.pending_raw_loads,
                    image_id,
                    MAX_CONCURRENT_RAW_LOADS,
                ) {
                    // Waiting in the preload queue's tail: decode it now. Moving it
                    // within the tail is not enough, because `schedule_raw_preloads`
                    // runs right after and rebuilds the tail without the selected
                    // image, which would drop it and leave Develop on "Loading".
                    if self.direct_raw_loads.insert(image_id) {
                        effects.push(decode_raw(image_id, raw_path));
                    }
                }
                // Otherwise the preload for it is already running (the first
                // `MAX_CONCURRENT_RAW_LOADS` queue entries): its result completes
                // the load.
            }

            // Schedule preloads for adjacent images
            self.schedule_preloads();
            if develop {
                self.schedule_raw_preloads();
                self.schedule_proxy_preloads();
            }

            self.camera_preview = Some(preview);
            return Effects::batch(effects);
        }
        self.camera_preview = Some(preview);
        Effects::none()
    }

    fn schedule_preloads(&mut self) {
        let wanted: Vec<(i64, String)> =
            self.neighbours(self.preview_preload_behind, self.preview_preload_ahead)
                .into_iter()
                .filter(|img| !self.preview_cache.contains(&img.id))
                .filter_map(|img| img.cache_path_working.clone().map(|path| (img.id, path)))
                .collect();
        requeue(
            &mut self.queued_loads,
            &mut self.pending_loads,
            wanted,
            MAX_CONCURRENT_PREVIEW_LOADS,
        );
    }

    fn schedule_raw_preloads(&mut self) {
        if self.raw_preload_budget_mb == 0 {
            return;
        }

        let wanted: Vec<(i64, String)> =
            self.neighbours(self.raw_preload_behind, self.raw_preload_ahead)
                .into_iter()
                .filter(|img| !self.raw_cache.contains(&img.id))
                // A direct decode of it is already running (an image stepped
                // past): queueing it as well would decode it twice.
                .filter(|img| !self.direct_raw_loads.contains(&img.id))
                .map(|img| (img.id, img.path.clone()))
                .collect();
        requeue(
            &mut self.queued_raw_loads,
            &mut self.pending_raw_loads,
            wanted,
            MAX_CONCURRENT_RAW_LOADS,
        );
    }

    /// Read Develop's upcoming neighbours' saved proxies into memory, so the
    /// placeholder for the next image is already decoded when the user gets there.
    fn schedule_proxy_preloads(&mut self) {
        let ids: Vec<i64> =
            self.neighbours(self.preview_preload_behind, self.preview_preload_ahead)
                .iter()
                .map(|img| img.id)
                .collect();
        let wanted = self.wanted_proxy_preloads(&ids);
        requeue(
            &mut self.queued_proxy_loads,
            &mut self.pending_proxy_loads,
            wanted,
            MAX_CONCURRENT_PREVIEW_LOADS,
        );
    }

    /// The images around the selection, in the order the user will reach them.
    ///
    /// Display order (the same list the arrow keys step through), nearest first
    /// — +1, −1, +2, −2, … — wrapping at the ends and listing each image once.
    /// This used to walk the unfiltered catalogue from the farthest image behind
    /// to the farthest ahead: preloads were spent on images a filter hides, and
    /// with only the queue's head running, the image you would open NEXT was
    /// among the last to decode.
    fn neighbours(&self, behind: usize, ahead: usize) -> Vec<&ImageData> {
        let shown = self.displayed_images();
        let Some(current) = self
            .selected_image_id
            .and_then(|selected| shown.iter().position(|img| img.id == selected))
        else {
            return Vec::new();
        };
        let n = shown.len();
        let mut seen: std::collections::HashSet<i64> = [shown[current].id].into();
        let mut out = Vec::new();
        for step in 1..=ahead.max(behind) {
            if step <= ahead {
                let img = shown[(current + step) % n];
                if seen.insert(img.id) {
                    out.push(img);
                }
            }
            if step <= behind {
                let img = shown[(current + n - step % n) % n];
                if seen.insert(img.id) {
                    out.push(img);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn session_with(ids: &[i64]) -> Session {
        let mut e = Session::default();
        e.current_tab = AppTab::Library;
        e.images = ids.iter().map(|i| img(*i, "/p/x.NEF")).collect();
        e
    }

    #[test]
    fn neighbours_come_nearest_first_and_wrap() {
        let mut e = session_with(&[1, 2, 3, 4, 5, 6]);
        e.selected_image_id = Some(1);
        let ids: Vec<i64> = e.neighbours(2, 3).iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![2, 6, 3, 5, 4]);
    }

    /// A window wider than the catalogue lists every other image exactly once.
    #[test]
    fn neighbours_never_repeat_or_include_the_selection() {
        let mut e = session_with(&[1, 2, 3]);
        e.selected_image_id = Some(2);
        let ids: Vec<i64> = e.neighbours(10, 50).iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![3, 1]);
    }

    #[test]
    fn requeue_keeps_in_flight_loads_and_replaces_the_rest() {
        let mut queue = vec![(1, "a".into()), (2, "b".into()), (3, "c".into())];
        let mut pending: std::collections::HashSet<i64> = [1, 2, 3].into();
        requeue(&mut queue, &mut pending, vec![(2, "b".into()), (9, "z".into())], 2);
        let ids: Vec<i64> = queue.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![1, 2, 9]);
        assert_eq!(pending, [1, 2, 9].into());
    }

    /// Arrow keys used to index the unfiltered catalogue and would happily
    /// land on images the current tab refuses to show.
    #[test]
    fn stepping_stays_inside_the_displayed_set() {
        let mut e = session_with(&[1, 2, 3]);
        e.images[1].rating = crate::database::models::with_mark(0, true);
        e.current_tab = AppTab::Develop; // excludes marked
        e.selected_image_id = Some(1);
        assert_eq!(e.step_selection(true), Some(3));
        assert_eq!(e.step_selection(false), Some(3)); // wraps backwards
    }
}

#[cfg(test)]
mod claim_tests {
    use super::{claim_queued, requeue};
    use std::collections::HashSet;

    fn q(ids: &[i64]) -> Vec<(i64, String)> {
        ids.iter().map(|i| (*i, String::new())).collect()
    }
    fn ids(q: &[(i64, String)]) -> Vec<i64> {
        q.iter().map(|(i, _)| *i).collect()
    }

    #[test]
    fn a_waiting_image_is_taken_out_of_the_queue_and_pending() {
        let mut queue = q(&[1, 2, 3, 4]);
        let mut pending: HashSet<i64> = [1, 2, 3, 4].into();
        assert!(claim_queued(&mut queue, &mut pending, 4, 2));
        assert_eq!(ids(&queue), vec![1, 2, 3]);
        assert!(!pending.contains(&4));
    }

    #[test]
    fn a_running_head_is_left_alone() {
        let mut queue = q(&[1, 2, 3]);
        let mut pending: HashSet<i64> = [1, 2, 3].into();
        assert!(!claim_queued(&mut queue, &mut pending, 2, 2));
        assert_eq!(ids(&queue), vec![1, 2, 3]);
        assert!(pending.contains(&2));
    }

    #[test]
    fn an_absent_image_reports_false() {
        let mut queue = q(&[1, 2]);
        let mut pending: HashSet<i64> = [1, 2].into();
        assert!(!claim_queued(&mut queue, &mut pending, 9, 2));
    }

    /// The bug: the selected image moved within the tail was then dropped by
    /// the requeue that follows every selection, so nothing ever decoded it.
    #[test]
    fn a_claimed_image_is_not_lost_to_the_requeue_that_follows() {
        let mut queue = q(&[1, 2, 7, 3]);
        let mut pending: HashSet<i64> = [1, 2, 7, 3].into();
        assert!(claim_queued(&mut queue, &mut pending, 7, 2));
        // the selected image is never in the wanted list
        requeue(&mut queue, &mut pending, vec![(3, String::new()), (4, String::new())], 2);
        assert_eq!(ids(&queue), vec![1, 2, 3, 4]);
        assert!(!pending.contains(&7), "7 is the caller's to decode, not a preload");
    }
}
