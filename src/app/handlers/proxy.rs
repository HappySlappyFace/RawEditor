// Edited proxies: a small render of each image AS EDITED.
//
// Every develop frame already renders a small whole-image overview for the
// histogram (`gpu::render_functions::render_display_and_overview`). Keeping
// its pixels gives each image an edited thumbnail for free — no extra pass:
//
// - the filmstrip and library grid show it instead of the camera's own JPEG,
//   so they look like the edit, and track it live while a slider moves;
// - Develop shows it while an image reloads, instead of the camera JPEG. That
//   JPEG ignores the edit, so an image whose edit differs from it (pulled-down
//   exposure, say) visibly jumped when the real render landed ~200 ms later.
//   Low resolution for a moment reads far better than a jump in tone.
//
// Lifecycle: captured in memory on each render of the real edit (`capture`),
// written to disk as a small JPEG when the user leaves the image (`persist`),
// and read back on demand (`load` / `schedule_preloads`). The invariant that
// makes on-disk proxies trustworthy: a saved proxy always matches the saved
// edits. Anything that changes an image's saved edits without rendering it —
// paste onto a selection, reset or undo from Library — drops the proxy
// (`RawEditor::invalidate_proxies`), falling back to the camera thumbnail.

use crate::app::message::{AppTab, Message};
use crate::app::state::{CachedPreview, EditedProxy, EditorReadiness, RawEditor};
use crate::core::types::EditParams;
use iced::Task;
use std::path::PathBuf;
use std::sync::Arc;

/// `params` with UI-only state cleared, for "is this the same edit?" checks.
pub fn edit_key(params: &EditParams) -> EditParams {
    params.without_tool_state()
}

/// Keep a fresh render's overview as `image_id`'s proxy.
pub fn capture(
    editor: &mut RawEditor,
    image_id: i64,
    pixels: Arc<[u8]>,
    (width, height): (u32, u32),
    params: EditParams,
) {
    if pixels.len() != width as usize * height as usize * 4 {
        return;
    }
    editor.proxy_generation += 1;
    let preview = CachedPreview { width, height, bytes: pixels };
    editor.edited_proxies.put(
        image_id,
        EditedProxy {
            handle: preview.to_handle(),
            preview,
            edited: !edit_key(&params).is_unedited(),
            params: Some(edit_key(&params)),
            generation: editor.proxy_generation,
            persisted: false,
        },
    );
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

/// The user is leaving the selected image (another image, another tab, or
/// quitting): bring its proxy on disk up to date. Must run BEFORE the
/// selection moves — it compares against `current_edit_params`.
pub fn persist(editor: &mut RawEditor) -> Task<Message> {
    let Some(id) = editor.selected_image_id else {
        return Task::none();
    };
    match plan(editor.edited_proxies.peek(&id), &editor.current_edit_params, has_camera_thumb(editor, id)) {
        Persist::Nothing => Task::none(),
        Persist::Invalidate => {
            // Releasing a slider and moving straight on leaves the render of
            // the final value still in flight, so the proxy is one step
            // behind. When that render IS the latest edit (nothing queued
            // after it), wait for it instead of discarding the thumbnail.
            let latest_in_flight = editor.is_rendering
                && !editor.pending_render
                && editor.image_resources.as_ref().map(|r| r.image_id) == Some(id);
            if latest_in_flight {
                editor.proxy_awaiting.insert(id, edit_key(&editor.current_edit_params));
            } else {
                editor.invalidate_proxies(&[id]);
            }
            Task::none()
        }
        Persist::Clear | Persist::Save(_) => write(editor, id),
    }
}

/// A render of `id` landed while `persist` was waiting on it: save it if it
/// shows the edit the user left the image with, otherwise drop the proxy.
pub fn resolve_awaiting(editor: &mut RawEditor, id: i64, rendered: Option<EditParams>) -> Task<Message> {
    let Some(expected) = editor.proxy_awaiting.remove(&id) else {
        return Task::none();
    };
    if rendered.map(|p| edit_key(&p)) == Some(expected) {
        write(editor, id)
    } else {
        editor.invalidate_proxies(&[id]);
        Task::none()
    }
}

/// Make `id`'s saved proxy match its in-memory one: write it out if it shows
/// an edit, forget the saved one if the image is back to its defaults.
fn write(editor: &mut RawEditor, id: i64) -> Task<Message> {
    let Some(proxy) = editor.edited_proxies.peek(&id) else {
        return Task::none();
    };
    if !proxy.edited && has_camera_thumb(editor, id) {
        set_saved_path(editor, id, None);
        if let Some(proxy) = editor.edited_proxies.peek_mut(&id) {
            proxy.persisted = true;
        }
        return Task::none();
    }
    let (generation, preview) = (proxy.generation, proxy.preview.clone());
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || write_jpeg(id, generation, &preview))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r)
        },
        move |result| Message::EditedProxySaved(id, generation, result),
    )
}

/// `persist`, synchronously — for window close, where neither a spawned save
/// nor a render still in flight would ever get to finish. A 384px JPEG is a
/// millisecond of work.
pub fn persist_now(editor: &mut RawEditor) {
    let Some(id) = editor.selected_image_id else {
        return;
    };
    match plan(editor.edited_proxies.peek(&id), &editor.current_edit_params, has_camera_thumb(editor, id)) {
        Persist::Nothing => {}
        Persist::Invalidate => editor.invalidate_proxies(&[id]),
        Persist::Clear => {
            let _ = write(editor, id);
        }
        Persist::Save(generation) => {
            if let Some(proxy) = editor.edited_proxies.peek(&id) {
                let result = write_jpeg(id, generation, &proxy.preview);
                handle_saved(editor, id, generation, result);
            }
        }
    }
}

pub fn handle_saved(editor: &mut RawEditor, id: i64, generation: u64, result: Result<String, String>) {
    match result {
        Ok(path) => {
            set_saved_path(editor, id, Some(path));
            // Only if nothing newer was rendered while the file was written.
            if let Some(proxy) = editor.edited_proxies.peek_mut(&id) {
                if proxy.generation == generation {
                    proxy.persisted = true;
                }
            }
        }
        Err(e) => tracing::warn!("Could not save the edited thumbnail of image {id}: {e}"),
    }
}

fn has_camera_thumb(editor: &RawEditor, id: i64) -> bool {
    editor.images.iter().any(|img| img.id == id && img.cache_path_thumb.is_some())
}

/// Record (or clear) an image's saved proxy, in the catalogue and in memory.
/// The previous file is left for `Library::sweep_edited_proxies` at the next
/// launch, so a rapid re-save can never delete a file still in use.
fn set_saved_path(editor: &mut RawEditor, id: i64, path: Option<String>) {
    if let Some(lib) = &editor.library {
        if let Err(e) = lib.set_edited_proxy_path(id, path.as_deref()) {
            tracing::warn!("Could not record the edited thumbnail of image {id}: {e}");
            return;
        }
    }
    editor.apply_to_images(&[id], |img| img.cache_path_edited = path.clone());
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
pub fn load(id: i64, path: String) -> Task<Message> {
    Task::perform(read_jpeg(path), move |result| Message::EditedProxyLoaded(id, result))
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

pub fn handle_loaded(
    editor: &mut RawEditor,
    id: i64,
    result: Result<(u32, u32, Arc<[u8]>), String>,
) -> Task<Message> {
    editor.pending_proxy_loads.remove(&id);
    editor.queued_proxy_loads.retain(|(queued, _)| *queued != id);

    let (width, height, bytes) = match result {
        Ok(decoded) => decoded,
        Err(e) => {
            tracing::debug!("Edited thumbnail of image {id} unreadable: {e}");
            return Task::none();
        }
    };
    // A capture of a live render is always newer than what is on disk.
    if editor.edited_proxies.contains(&id) {
        return Task::none();
    }
    let preview = CachedPreview { width, height, bytes };
    editor.edited_proxies.put(
        id,
        EditedProxy {
            handle: preview.to_handle(),
            preview: preview.clone(),
            edited: true,
            // Unknown until the image is selected, when its saved edits load
            // — which the saved proxy matches by construction.
            params: (editor.selected_image_id == Some(id))
                .then(|| edit_key(&editor.current_edit_params)),
            generation: 0,
            persisted: true,
        },
    );

    // Still waiting on this image's RAW in Develop: show it now.
    if editor.current_tab == AppTab::Develop
        && editor.selected_image_id == Some(id)
        && matches!(editor.editor_readiness, EditorReadiness::Loading(l) if l == id)
        && editor.rendered_preview_bytes.is_none()
    {
        show_as_placeholder(editor, &preview);
    }
    Task::none()
}

/// Put a proxy on screen while the image's RAW loads.
pub fn show_as_placeholder(editor: &mut RawEditor, preview: &CachedPreview) {
    editor.working_preview_bytes = Some(preview.bytes.clone());
    editor.working_preview_dims = (preview.width, preview.height);
    editor.bump_preview_generation();
}

/// Queue saved proxies of `ids` (Develop's upcoming neighbours, nearest
/// first) that are not in memory yet, so stepping onto them shows the edit
/// immediately instead of a frame of nothing.
pub fn wanted_preloads(editor: &RawEditor, ids: &[i64]) -> Vec<(i64, String)> {
    let saved: std::collections::HashMap<i64, &String> = editor
        .images
        .iter()
        .filter_map(|img| img.cache_path_edited.as_ref().map(|path| (img.id, path)))
        .collect();
    ids.iter()
        .filter(|id| !editor.edited_proxies.contains(id))
        .filter_map(|id| saved.get(id).map(|path| (*id, (*path).clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy(params: Option<EditParams>, edited: bool, persisted: bool) -> EditedProxy {
        let preview = CachedPreview { width: 1, height: 1, bytes: Arc::from(vec![0u8; 4]) };
        EditedProxy {
            handle: preview.to_handle(),
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
        let mut e = RawEditor::new().0;
        let px: Arc<[u8]> = Arc::from(vec![0u8; 8]);
        capture(&mut e, 1, px.clone(), (2, 1), EditParams::default());
        assert!(!e.edited_proxies.peek(&1).unwrap().edited);
        capture(&mut e, 2, px.clone(), (2, 1), edited());
        assert!(e.edited_proxies.peek(&2).unwrap().edited);
        // A buffer that doesn't match its dims (failed readback) is ignored.
        capture(&mut e, 3, px, (4, 4), edited());
        assert!(e.edited_proxies.peek(&3).is_none());
    }

    fn late_render(id: i64, edit: EditParams) -> crate::app::message::RenderOutput {
        crate::app::message::RenderOutput {
            image_id: id,
            display: Arc::from(Vec::new()),
            dims: (1, 1),
            view_rect: crate::gpu::params::FULL_VIEW_RECT,
            overview: Arc::from(vec![9u8; 8]),
            overview_dims: (2, 1),
            histogram: [[0; 256]; 3],
            edit: Some(edit),
            upload_ms: 0.0,
            render_ms: 0.0,
            update_ms: 0.0,
        }
    }

    /// Released a slider and moved straight on: the final render lands after
    /// the user left, and must still become the image's thumbnail.
    #[test]
    fn the_awaited_final_render_becomes_the_proxy() {
        let mut e = RawEditor::new().0;
        e.proxy_awaiting.insert(5, edit_key(&edited()));
        let _ = crate::app::handlers::develop::handle_render_finished(&mut e, late_render(5, edited()));
        assert!(e.edited_proxies.peek(&5).is_some_and(|p| p.edited));
        assert!(e.proxy_awaiting.is_empty());
    }

    /// ...but a render of an older step of the edit must not.
    #[test]
    fn an_awaited_render_of_an_older_edit_is_dropped() {
        let mut e = RawEditor::new().0;
        e.proxy_awaiting.insert(5, edit_key(&edited()));
        let older = EditParams { exposure: -0.5, ..EditParams::default() };
        let _ = crate::app::handlers::develop::handle_render_finished(&mut e, late_render(5, older));
        assert!(e.edited_proxies.peek(&5).is_none());
        assert!(e.proxy_awaiting.is_empty());
    }

    /// Without anyone waiting on it, a render of an image no longer loaded
    /// is ignored entirely.
    #[test]
    fn a_stray_render_of_another_image_is_ignored() {
        let mut e = RawEditor::new().0;
        let _ = crate::app::handlers::develop::handle_render_finished(&mut e, late_render(5, edited()));
        assert!(e.edited_proxies.peek(&5).is_none());
    }
}
