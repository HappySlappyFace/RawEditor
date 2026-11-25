//! Edited proxies: a small render of each image AS EDITED.
//!
//! Every develop frame already renders a small whole-image overview for the
//! histogram (`gpu::render_functions::render_display_and_overview`). Keeping
//! its pixels gives each image an edited thumbnail for free — no extra pass:
//!
//! - the filmstrip and library grid show it instead of the camera's own JPEG,
//!   so they look like the edit, and track it live while a slider moves;
//! - Develop shows it while an image reloads, instead of the camera JPEG. That
//!   JPEG ignores the edit, so an image whose edit differs from it (pulled-down
//!   exposure, say) visibly jumped when the real render landed ~200 ms later.
//!   Low resolution for a moment reads far better than a jump in tone.
//!
//! Lifecycle: captured in memory on each render of the real edit (`capture`),
//! written to disk as a small JPEG when the user leaves the image (`persist`),
//! and read back on demand (`load` / `schedule_preloads`). The invariant that
//! makes on-disk proxies trustworthy: a saved proxy always matches the saved
//! edits. Anything that changes an image's saved edits without rendering it —
//! paste onto a selection, reset or undo from Library — drops the proxy
//! (`crate::session::Session::invalidate_proxies`), falling back to the camera thumbnail.

use super::{Effects, Session, SessionEvent};
use crate::session::AppTab;
use crate::session::EditorReadiness;
use crate::core::types::EditParams;
use std::path::PathBuf;
use std::sync::Arc;

/// A preloaded 1280px working preview, held in the RAM look-ahead cache.
///
/// Stores the decoded pixels, not a UI image handle. The Develop viewport is a
/// wgpu shader that needs the raw bytes; caching only a `Handle` meant the
/// preloader's decode was thrown away and the JPEG was re-read from disk the
/// moment the user navigated to the image it had just preloaded.
#[derive(Debug, Clone)]
pub struct CachedPreview {
    pub width: u32,
    pub height: u32,
    pub bytes: Arc<[u8]>,
}

/// A small render of an image as edited — see `session::proxy`.
#[derive(Debug, Clone)]
pub struct EditedProxy {
    pub preview: CachedPreview,
    /// The render had edits applied. Only edited proxies replace the
    /// camera's thumbnail — but an unedited one is still the best placeholder
    /// while the image reloads, being exactly what Develop is about to show.
    pub edited: bool,
    /// The edit rendered (`proxy::edit_key`), to tell when it has fallen
    /// behind. `None` for a proxy read from disk until its image is selected.
    pub params: Option<crate::core::types::EditParams>,
    /// Orders captures, so a save that completes after a newer render cannot
    /// mark that newer render as saved. Unique per proxy (one read from disk
    /// takes a number too), so a front end can key what it builds from the
    /// pixels on it.
    pub generation: u64,
    /// The file on disk holds this capture.
    pub persisted: bool,
}

/// Edited proxies kept in memory. ~400 KB each (384px RGBA).
pub const EDITED_PROXY_CAPACITY: usize = 128;

/// `params` with UI-only state cleared, for "is this the same edit?" checks.
pub fn edit_key(params: &EditParams) -> EditParams {
    params.without_tool_state()
}

/// What leaving `image_id` should do with its proxy.
#[derive(Debug, PartialEq)]
enum Persist {
    /// Nothing new since the last save (or no proxy at all).
    Nothing,
    /// The proxy no longer shows the image's edits (they changed while no
    /// usable render was being captured, e.g. in crop mode): drop it.
    Invalidate,
    /// The image is back to its defaults: forget any saved edited thumbnail
    /// so the camera's own is shown again.
    Clear,
    /// Write this generation to disk.
    Save(u64),
}

/// `has_camera_thumb`: an image without one (its embedded preview could not
/// be read) keeps even an unedited render on disk — it is the only thumbnail
/// the image has.
fn plan(proxy: Option<&EditedProxy>, current: &EditParams, has_camera_thumb: bool) -> Persist {
    let Some(proxy) = proxy else {
        return Persist::Nothing;
    };
    match proxy.params {
        Some(rendered) if rendered != edit_key(current) => Persist::Invalidate,
        _ if proxy.persisted => Persist::Nothing,
        _ if !proxy.edited && has_camera_thumb => Persist::Clear,
        _ => Persist::Save(proxy.generation),
    }
}

/// Encode a proxy as a JPEG in the `edited` cache tier. The name is unique
/// per save: iced caches decoded images by path, so overwriting one file
/// would keep showing the old pixels.
fn write_jpeg(id: i64, generation: u64, preview: &CachedPreview) -> Result<String, String> {
    let dir = crate::raw::processor::cache_dir("edited")?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path: PathBuf = dir.join(format!("{id}-{generation}-{stamp}.jpg"));
    let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut writer = std::io::BufWriter::new(file);
    let width = u16::try_from(preview.width).map_err(|e| e.to_string())?;
    let height = u16::try_from(preview.height).map_err(|e| e.to_string())?;
    jpeg_encoder::Encoder::new(&mut writer, 85)
        .encode(&preview.bytes, width, height, jpeg_encoder::ColorType::Rgba)
        .map_err(|e| e.to_string())?;
    std::io::Write::flush(&mut writer).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

/// Read `id`'s saved proxy back into memory.
pub fn load_proxy(id: i64, path: String) -> Effects {
    Effects::perform("proxy read", async move { SessionEvent::EditedProxyLoaded(id, read_jpeg(path).await) })
}

/// Decode a saved proxy to RGBA, off the UI thread.
pub async fn read_jpeg(path: String) -> Result<(u32, u32, Arc<[u8]>), String> {
    tokio::task::spawn_blocking(move || {
        let rgba = image::open(&path).map_err(|e| e.to_string())?.to_rgba8();
        Ok((rgba.width(), rgba.height(), Arc::from(rgba.into_raw())))
    })
    .await
    .map_err(|e| e.to_string())?
}

impl Session {
    /// Keep a fresh render's overview as `image_id`'s proxy.
    pub fn capture_proxy(
        &mut self,
        image_id: i64,
        pixels: Arc<[u8]>,
        (width, height): (u32, u32),
        params: EditParams,
    ) {
        if pixels.len() != width as usize * height as usize * 4 {
            return;
        }
        self.proxy_generation += 1;
        let preview = CachedPreview { width, height, bytes: pixels };
        self.edited_proxies.put(
            image_id,
            EditedProxy {
                preview,
                edited: !edit_key(&params).is_unedited(),
                params: Some(edit_key(&params)),
                generation: self.proxy_generation,
                persisted: false,
            },
        );
    }

    /// The user is leaving the selected image (another image, another tab, or
    /// quitting): bring its proxy on disk up to date. Must run BEFORE the
    /// selection moves — it compares against `current_edit_params`.
    pub fn persist_proxy(&mut self) -> Effects {
        let Some(id) = self.selected_image_id else {
            return Effects::none();
        };
        match plan(self.edited_proxies.peek(&id), &self.current_edit_params, self.has_camera_thumb(id)) {
            Persist::Nothing => Effects::none(),
            Persist::Invalidate => {
                // Releasing a slider and moving straight on leaves the render of
                // the final value still in flight, so the proxy is one step
                // behind. When that render IS the latest edit (nothing queued
                // after it), wait for it instead of discarding the thumbnail.
                let latest_in_flight = self.is_rendering
                    && !self.pending_render
                    && self.image_resources.as_ref().map(|r| r.image_id) == Some(id);
                if latest_in_flight {
                    self.proxy_awaiting.insert(id, edit_key(&self.current_edit_params));
                } else {
                    self.invalidate_proxies(&[id]);
                }
                Effects::none()
            }
            Persist::Clear | Persist::Save(_) => self.write_proxy(id),
        }
    }

    /// A render of `id` landed while `persist` was waiting on it: save it if it
    /// shows the edit the user left the image with, otherwise drop the proxy.
    pub fn resolve_awaiting_proxy(&mut self, id: i64, rendered: Option<EditParams>) -> Effects {
        let Some(expected) = self.proxy_awaiting.remove(&id) else {
            return Effects::none();
        };
        if rendered.map(|p| edit_key(&p)) == Some(expected) {
            self.write_proxy(id)
        } else {
            self.invalidate_proxies(&[id]);
            Effects::none()
        }
    }

    /// Make `id`'s saved proxy match its in-memory one: write it out if it shows
    /// an edit, forget the saved one if the image is back to its defaults.
    fn write_proxy(&mut self, id: i64) -> Effects {
        let Some(proxy) = self.edited_proxies.peek(&id) else {
            return Effects::none();
        };
        if !proxy.edited && self.has_camera_thumb(id) {
            self.set_saved_proxy_path(id, None);
            if let Some(proxy) = self.edited_proxies.peek_mut(&id) {
                proxy.persisted = true;
            }
            return Effects::none();
        }
        let (generation, preview) = (proxy.generation, proxy.preview.clone());
        Effects::perform("proxy save", async move {
            let result = tokio::task::spawn_blocking(move || write_jpeg(id, generation, &preview))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
            SessionEvent::EditedProxySaved(id, generation, result)
        })
    }

    /// `persist`, synchronously — for window close, where neither a spawned save
    /// nor a render still in flight would ever get to finish. A 384px JPEG is a
    /// millisecond of work.
    pub fn persist_proxy_now(&mut self) {
        let Some(id) = self.selected_image_id else {
            return;
        };
        match plan(self.edited_proxies.peek(&id), &self.current_edit_params, self.has_camera_thumb(id)) {
            Persist::Nothing => {}
            Persist::Invalidate => self.invalidate_proxies(&[id]),
            Persist::Clear => {
                let _ = self.write_proxy(id);
            }
            Persist::Save(generation) => {
                if let Some(proxy) = self.edited_proxies.peek(&id) {
                    let result = write_jpeg(id, generation, &proxy.preview);
                    self.proxy_saved(id, generation, result);
                }
            }
        }
    }

    pub fn proxy_saved(&mut self, id: i64, generation: u64, result: Result<String, String>) {
        match result {
            Ok(path) => {
                self.set_saved_proxy_path(id, Some(path));
                // Only if nothing newer was rendered while the file was written.
                if let Some(proxy) = self.edited_proxies.peek_mut(&id) {
                    if proxy.generation == generation {
                        proxy.persisted = true;
                    }
                }
            }
            Err(e) => tracing::warn!("Could not save the edited thumbnail of image {id}: {e}"),
        }
    }

    fn has_camera_thumb(&self, id: i64) -> bool {
        self.images.iter().any(|img| img.id == id && img.cache_path_thumb.is_some())
    }

    /// Record (or clear) an image's saved proxy, in the catalogue and in memory.
    /// The previous file is left for `Library::sweep_edited_proxies` at the next
    /// launch, so a rapid re-save can never delete a file still in use.
    fn set_saved_proxy_path(&mut self, id: i64, path: Option<String>) {
        if let Some(lib) = &self.library {
            if let Err(e) = lib.set_edited_proxy_path(id, path.as_deref()) {
                tracing::warn!("Could not record the edited thumbnail of image {id}: {e}");
                return;
            }
        }
        self.apply_to_images(&[id], |img| img.cache_path_edited = path.clone());
    }

    pub fn proxy_loaded(&mut self, id: i64, result: Result<(u32, u32, Arc<[u8]>), String>) {
        self.pending_proxy_loads.remove(&id);
        self.queued_proxy_loads.retain(|(queued, _)| *queued != id);

        let (width, height, bytes) = match result {
            Ok(decoded) => decoded,
            Err(e) => {
                tracing::debug!("Edited thumbnail of image {id} unreadable: {e}");
                return;
            }
        };
        // A capture of a live render is always newer than what is on disk.
        if self.edited_proxies.contains(&id) {
            return;
        }
        let preview = CachedPreview { width, height, bytes };
        self.proxy_generation += 1;
        self.edited_proxies.put(
            id,
            EditedProxy {
                preview: preview.clone(),
                edited: true,
                // Unknown until the image is selected, when its saved edits load
                // — which the saved proxy matches by construction.
                params: (self.selected_image_id == Some(id))
                    .then(|| edit_key(&self.current_edit_params)),
                generation: self.proxy_generation,
                persisted: true,
            },
        );

        // Still waiting on this image's RAW in Develop: show it now.
        if self.current_tab == AppTab::Develop
            && self.selected_image_id == Some(id)
            && matches!(self.editor_readiness, EditorReadiness::Loading(l) if l == id)
            && self.rendered_preview_bytes.is_none()
        {
            self.show_as_placeholder(&preview);
        }
    }

    /// Put a proxy on screen while the image's RAW loads.
    pub fn show_as_placeholder(&mut self, preview: &CachedPreview) {
        self.working_preview_bytes = Some(preview.bytes.clone());
        self.working_preview_dims = (preview.width, preview.height);
        self.bump_preview_generation();
    }

    /// Queue saved proxies of `ids` (Develop's upcoming neighbours, nearest
    /// first) that are not in memory yet, so stepping onto them shows the edit
    /// immediately instead of a frame of nothing.
    pub fn wanted_proxy_preloads(&self, ids: &[i64]) -> Vec<(i64, String)> {
        let saved: std::collections::HashMap<i64, &String> = self
            .images
            .iter()
            .filter_map(|img| img.cache_path_edited.as_ref().map(|path| (img.id, path)))
            .collect();
        ids.iter()
            .filter(|id| !self.edited_proxies.contains(id))
            .filter_map(|id| saved.get(id).map(|path| (*id, (*path).clone())))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy(params: Option<EditParams>, edited: bool, persisted: bool) -> EditedProxy {
        let preview = CachedPreview { width: 1, height: 1, bytes: Arc::from(vec![0u8; 4]) };
        EditedProxy {
            preview,
            edited,
            params,
            generation: 7,
            persisted,
        }
    }

    fn edited() -> EditParams {
        EditParams { exposure: -1.0, ..EditParams::default() }
    }

    #[test]
    fn a_fresh_unsaved_edit_is_saved() {
        let p = proxy(Some(edited()), true, false);
        assert_eq!(plan(Some(&p), &edited(), true), Persist::Save(7));
    }

    #[test]
    fn an_already_saved_proxy_is_left_alone() {
        let p = proxy(Some(edited()), true, true);
        assert_eq!(plan(Some(&p), &edited(), true), Persist::Nothing);
        assert_eq!(plan(None, &edited(), true), Persist::Nothing);
    }

    /// Edits made while no usable render was captured (crop mode) leave the
    /// proxy showing the old edit — it must not be saved as the new one.
    #[test]
    fn a_proxy_behind_the_edit_is_invalidated() {
        let p = proxy(Some(edited()), true, false);
        let moved = EditParams { exposure: 0.5, ..EditParams::default() };
        assert_eq!(plan(Some(&p), &moved, true), Persist::Invalidate);
    }

    /// The crop tool's flag is UI state: entering crop mode must not make an
    /// otherwise current proxy look stale.
    #[test]
    fn the_crop_tool_flag_does_not_count_as_an_edit() {
        let p = proxy(Some(edited()), true, false);
        let mut cropping = edited();
        cropping.is_cropping = 1;
        assert_eq!(plan(Some(&p), &cropping, true), Persist::Save(7));
    }

    #[test]
    fn a_reset_image_clears_its_saved_thumbnail() {
        let p = proxy(Some(EditParams::default()), false, false);
        assert_eq!(plan(Some(&p), &EditParams::default(), true), Persist::Clear);
    }

    /// Without a camera thumbnail the unedited render is the image's only
    /// one, so it is saved rather than cleared.
    #[test]
    fn an_image_without_a_camera_thumbnail_keeps_its_unedited_render() {
        let p = proxy(Some(EditParams::default()), false, false);
        assert_eq!(plan(Some(&p), &EditParams::default(), false), Persist::Save(7));
    }

    #[test]
    fn capture_marks_unedited_renders_as_such() {
        let mut e = Session::default();
        let px: Arc<[u8]> = Arc::from(vec![0u8; 8]);
        e.capture_proxy(1, px.clone(), (2, 1), EditParams::default());
        assert!(!e.edited_proxies.peek(&1).unwrap().edited);
        e.capture_proxy(2, px.clone(), (2, 1), edited());
        assert!(e.edited_proxies.peek(&2).unwrap().edited);
        // A buffer that doesn't match its dims (failed readback) is ignored.
        e.capture_proxy(3, px, (4, 4), edited());
        assert!(e.edited_proxies.peek(&3).is_none());
    }

    // ── the session's side: capture, save, read back ─────────────────────────
    //
    // A save writes a JPEG to the user's cache folder, so these tests never
    // run the "proxy save" effect: they deliver its result by hand.

    use crate::session::testkit::{labels, open_in_develop, session_with_images};

    fn pixels() -> Arc<[u8]> {
        Arc::from(vec![128u8; 2 * 2 * 4])
    }

    /// Image 1 open in Develop with an edit, its render captured.
    fn captured() -> Session {
        let mut s = session_with_images(3);
        let _ = open_in_develop(&mut s, 1);
        s.current_edit_params = edited();
        s.capture_proxy(1, pixels(), (2, 2), edited());
        s
    }

    fn saved_path(s: &Session, id: i64) -> Option<String> {
        let rows = s.library.as_ref().unwrap().get_all_images().unwrap();
        rows.into_iter().find(|i| i.id == id).and_then(|i| i.cache_path_edited)
    }

    #[test]
    fn a_capture_with_the_wrong_size_is_ignored() {
        let mut s = session_with_images(1);
        s.capture_proxy(1, Arc::from(vec![0u8; 5]), (2, 2), edited());
        assert!(s.edited_proxies.is_empty());
    }

    /// Leaving the image saves its proxy once, and the catalogue records the
    /// file; leaving again with nothing new saves nothing.
    #[test]
    fn leaving_an_edited_image_saves_its_proxy_once() {
        let mut s = captured();
        assert_eq!(labels(&s.persist_proxy()), ["proxy save"]);
        let generation = s.edited_proxies.peek(&1).unwrap().generation;
        s.proxy_saved(1, generation, Ok("/cache/edited/1.jpg".into()));
        assert_eq!(saved_path(&s, 1).as_deref(), Some("/cache/edited/1.jpg"));
        assert_eq!(s.images[0].cache_path_edited.as_deref(), Some("/cache/edited/1.jpg"));
        assert!(s.persist_proxy().is_empty(), "saved again with nothing new");
    }

    /// A save that lands after a newer render records its file, but must not
    /// mark the newer render as saved: leaving again saves that one.
    #[test]
    fn a_save_overtaken_by_a_newer_render_does_not_count_for_it() {
        let mut s = captured();
        let old = s.edited_proxies.peek(&1).unwrap().generation;
        let _ = s.persist_proxy();
        s.capture_proxy(1, pixels(), (2, 2), edited());
        s.proxy_saved(1, old, Ok("/cache/edited/old.jpg".into()));
        assert!(!s.edited_proxies.peek(&1).unwrap().persisted);
        assert_eq!(labels(&s.persist_proxy()), ["proxy save"]);
    }

    #[test]
    fn a_failed_save_leaves_the_catalogue_alone() {
        let mut s = captured();
        let generation = s.edited_proxies.peek(&1).unwrap().generation;
        s.proxy_saved(1, generation, Err("disk full".into()));
        assert_eq!(saved_path(&s, 1), None);
        assert!(!s.edited_proxies.peek(&1).unwrap().persisted);
    }

    /// The edit moved on with no render to show it (crop mode, say): the
    /// proxy, and the saved file it would replace, are dropped. The camera's
    /// own thumbnail is wrong too, but never shows an edit that isn't there.
    #[test]
    fn a_proxy_behind_the_edit_is_dropped_with_its_saved_file() {
        let mut s = captured();
        s.proxy_saved(1, s.edited_proxies.peek(&1).unwrap().generation, Ok("/cache/edited/1.jpg".into()));
        s.current_edit_params.exposure = 0.7;
        assert!(s.persist_proxy().is_empty());
        assert!(!s.edited_proxies.contains(&1));
        assert_eq!(saved_path(&s, 1), None, "the stale file is still the catalogue's thumbnail");
    }

    /// Releasing a slider and moving straight on: the render of the final
    /// value is still in flight. Wait for it rather than drop the thumbnail,
    /// then save it if it shows that edit, or drop it if not.
    #[test]
    fn leaving_mid_render_waits_for_the_render_of_the_final_edit() {
        let mut s = captured();
        s.current_edit_params.exposure = 0.7;
        s.is_rendering = true;
        // The wait needs the in-flight render to be this image's, which
        // needs its GPU resources: without them the proxy is dropped.
        assert!(s.persist_proxy().is_empty());
        assert!(s.proxy_awaiting.is_empty());

        let mut s = captured();
        let final_edit = EditParams { exposure: 0.7, ..EditParams::default() };
        s.proxy_awaiting.insert(1, edit_key(&final_edit));
        s.capture_proxy(1, pixels(), (2, 2), final_edit);
        assert_eq!(labels(&s.resolve_awaiting_proxy(1, Some(final_edit))), ["proxy save"]);
        assert!(s.resolve_awaiting_proxy(1, Some(final_edit)).is_empty(), "resolved twice");

        let mut s = captured();
        s.proxy_awaiting.insert(1, edit_key(&final_edit));
        assert!(s.resolve_awaiting_proxy(1, Some(edited())).is_empty());
        assert!(!s.edited_proxies.contains(&1), "a render of an older edit was kept");
    }

    /// An image reset to its defaults goes back to the camera's thumbnail:
    /// the saved edited one is forgotten, with nothing written.
    #[test]
    fn a_reset_image_forgets_its_saved_proxy_without_writing() {
        let mut s = captured();
        s.proxy_saved(1, s.edited_proxies.peek(&1).unwrap().generation, Ok("/cache/edited/1.jpg".into()));
        s.images[0].cache_path_thumb = Some("/cache/thumb/1.jpg".into());
        s.current_edit_params = EditParams::default();
        s.capture_proxy(1, pixels(), (2, 2), EditParams::default());
        assert!(s.persist_proxy().is_empty());
        assert_eq!(saved_path(&s, 1), None);
        assert!(s.edited_proxies.peek(&1).unwrap().persisted);
    }

    /// Without a camera thumbnail, even an unedited render is saved: it is the
    /// only thumbnail the image has.
    #[test]
    fn an_image_without_a_camera_thumbnail_keeps_an_unedited_proxy() {
        let mut s = session_with_images(1);
        let _ = open_in_develop(&mut s, 1);
        s.capture_proxy(1, pixels(), (2, 2), EditParams::default());
        assert_eq!(labels(&s.persist_proxy()), ["proxy save"]);
    }

    /// A proxy read from disk fills in what memory lacks, never replaces a
    /// live capture, and while its image's RAW loads in Develop it is shown.
    #[test]
    fn a_proxy_read_back_fills_in_and_shows_while_loading() {
        let mut s = session_with_images(3);
        let _ = open_in_develop(&mut s, 1);
        s.pending_proxy_loads.insert(1);
        s.queued_proxy_loads.push((1, "/p/1.jpg".into()));
        s.proxy_loaded(1, Ok((2, 2, pixels())));
        assert!(s.pending_proxy_loads.is_empty() && s.queued_proxy_loads.is_empty());
        let p = s.edited_proxies.peek(&1).unwrap();
        assert!(p.persisted && p.edited);
        assert_eq!(p.params, Some(edit_key(&s.current_edit_params)), "the open image's edit is known");
        assert_eq!(s.working_preview_dims, (2, 2), "not shown while loading");

        s.proxy_loaded(2, Ok((2, 2, pixels())));
        assert_eq!(s.edited_proxies.peek(&2).unwrap().params, None, "another image's edit isn't known yet");

        s.capture_proxy(3, pixels(), (2, 2), edited());
        let live = s.edited_proxies.peek(&3).unwrap().generation;
        s.proxy_loaded(3, Ok((4, 4, Arc::from(vec![0u8; 64]))));
        assert_eq!(s.edited_proxies.peek(&3).unwrap().generation, live, "the disk copy replaced a live capture");
    }

    #[test]
    fn an_unreadable_proxy_is_skipped() {
        let mut s = session_with_images(1);
        s.pending_proxy_loads.insert(1);
        s.proxy_loaded(1, Err("truncated".into()));
        assert!(s.pending_proxy_loads.is_empty());
        assert!(s.edited_proxies.is_empty());
    }

    #[test]
    fn preloads_are_the_saved_proxies_not_in_memory_in_the_order_asked() {
        let mut s = session_with_images(4);
        for id in [2, 3, 4] {
            s.images[id as usize - 1].cache_path_edited = Some(format!("/p/{id}.jpg"));
        }
        s.capture_proxy(3, pixels(), (2, 2), edited());
        assert_eq!(s.wanted_proxy_preloads(&[4, 1, 3, 2]), [(4, "/p/4.jpg".into()), (2, "/p/2.jpg".into())]);
    }

    /// On window close a stale proxy is dropped there and then (nothing will
    /// render again to save it).
    #[test]
    fn closing_the_window_drops_a_stale_proxy() {
        let mut s = captured();
        s.current_edit_params.exposure = 0.7;
        s.persist_proxy_now();
        assert!(!s.edited_proxies.contains(&1));
    }
}
