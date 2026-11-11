# RawEditor code review notes

Started 2026-11-10 at commit fd91513 (`perceptual-color`). Read-only review: nothing in `src/` was changed. (One temporary `#[cfg(test)]` probe was added to `src/app/message.rs` to print `size_of` figures and reverted straight after; `git status` shows only this file.)

Each finding has a severity:

- **BUG**: behaviour I believe is wrong, with the path that triggers it.
- **RISK**: correct today, but fragile or wrong under conditions named in the note.
- **PERF**: measurable or likely cost.
- **CLEANUP**: simpler or clearer code, no behaviour change.
- **IDEA**: a behaviour worth adding or changing; nothing is broken.

"Checked" means I traced the code path; "suspected" means I read it but did not confirm by running anything.


## 1. Update loop, Develop handlers, AI select

Files read: `app/update.rs`, `app/handlers/{develop,ai_mask,masks,navigation}.rs`, parts of `app/state.rs`, `app/handlers/loading.rs`, and the geometry/mask blocks of `gpu/shaders.rs`.

### BUG 1.1: Leaving an image mid AI-select can delete a mask on the next image (checked by reading, not reproduced)

- `handle_image_selected` (`handlers/navigation.rs:57`) replaces `current_edit_params` with the NEW image's edits.
- `update()` then runs `ai_mask::track` (`handlers/ai_mask.rs:382`), which sees the image changed and calls `handle_done`.
- `handle_done` (`ai_mask.rs:294`) tests `s.created && current_edit_params.masks[s.slot].ai_id == 0`. Those are now the new image's masks. Any non-AI mask has `ai_id == 0`, so the test passes and `handle_delete_mask(editor, s.slot)` deletes the new image's mask at that index and commits it.
- Trigger: on image A press "+ AI Select" (new slot N), then click another image in the filmstrip without pressing Done. If image B has a linear/radial/brush/range mask at index N, it is deleted. Undo on B brings it back, but nothing tells the user it happened.
- Second effect: if the session never produced a selection, A is saved (by the `flush_edits` at the top of `handle_image_selected`) with an empty `MASK_AI` slot, because the clean-up ran against B.
- Same pattern for a tab switch is fine (params unchanged); the image switch is the broken case.
- Fix: end the session where the image is left, before the params are swapped. Call `ai_mask::handle_done` at the top of `handle_image_selected` (next to the existing `flush_edits`), and make `track`'s `selected_image_id != s.image_id` branch only drop the session without touching params.

### BUG 1.2: Eyedroppers ignore Straighten and lens correction (checked by reading)

- `sample_raw_rgb` (`handlers/develop.rs:946`) maps the click through the crop and `display_to_sensor_uv` only.
- The colour pass also rotates by `params.rotation` and applies the lens remap (Step 0) before it samples the input.
- So with a non-zero Straighten or lens distortion correction, the WB eyedropper and the mask colour-range eyedropper (`handlers/masks.rs:357`) average a different patch from the one under the cursor. At 5° the offset near the frame edge is roughly 4% of the frame.
- Fix: one function, `core::viewport::display_to_source_uv(params, lens, uv)`, mirroring the shader's chain (rotation → orientation → lens), unit-tested against the same numbers `straighten_from_line`'s test uses. Use it in every click-to-sensor mapping.

### BUG 1.3: AI masks are prompted and drawn in the wrong frame on straightened or lens-corrected images (checked by reading)

- `handle_prompt` / `handle_rect` (`ai_mask.rs:224`, `232–233`) convert the click with `display_to_sensor_uv` only, so SAM is prompted at a point that is not the content clicked (same cause as 1.2).
- The mask bitmap is then sampled at display UV before rotation and lens remap (the mask stage uses `input.tex_coords`), while the picture is sampled after them. The mask sits offset from the subject.
- For the same reason, changing Straighten or toggling lens correction after making an AI mask or a brush stroke slides the picture under the mask. Linear and radial masks have the same property, but they are quick to re-place; a painted or AI mask is not.
- Fix options: (a) evaluate bitmap and brush layers at the post-rotation/post-lens source UV in the shader, so pixel-anchored masks follow the picture; or (b) carry them along in `EditParams` when rotation changes, as `turn_quarter` does for quarter turns. (a) is less code and exact.

### RISK 1.4: `is_slider_change` is a hand-kept list and is already incomplete

- `update.rs:92`. Missing: `BlackOffsetChanged`, `BlackPhaseChanged`, `BrushSettingChanged`, `AiDenoiseAmount`.
- Effect: a Ctrl-click reset on those sliders does not commit, and moving them does not end a typed slider entry.
- Fix: have the forked slider emit its release message on a Ctrl-click reset. Then `update` needs no list at all.

### RISK 1.5: `MenuItem` and `SliderSet` bypass the wrapper logic in `update`

- `update.rs:284` and `:367` call `dispatch` on the inner message. The cross-cutting steps in `update` (`ends_slider_entry`, the reset commit) only see the outer message.
- Today the inner messages happen not to need those steps. A future one will silently skip them.
- Fix: recurse into `update` for the inner message, or move the cross-cutting steps into a function both paths call.

### RISK 1.6: `dcp_memo` is not keyed on the profile

- `handlers/develop.rs:1097–1135`. The memo key is `(kelvin, ToneSettings)`. It is only correct because `handlers/loading.rs:80` and `:176` clear it by hand whenever resources are rebuilt.
- A new path that swaps the profile without going through those two lines returns the previous profile's interpolation.
- Fix: add the profile's identity to the key (`Arc::as_ptr` of the DCP, or the image id + profile name).

### RISK 1.7: `handle_reset_edits` leaves tools armed and saves twice

- `handlers/develop.rs:474`. It clears `selected_mask` and `mask_tool`, but not `brush_armed`, `color_picking_mask`, or an open `ai_select` session, all of which index masks that no longer exist.
- It calls `lib.delete_edits` and then `commit_current_state`, which flushes the default params straight back into the table.

### RISK 1.8: `sample_raw_rgb` black level handling

- `handlers/develop.rs:978`: it subtracts the mean of the four black levels, not each site's own, ignores the user's `black_offsets`, and divides by `black_levels.len()` (NaN when empty).
- `handlers/masks.rs:371` repeats the mean with a literal `/ 4.0`.
- It reads `raw.data`, so with AI denoise on it samples the un-denoised mosaic while the shader sees the blended one. Small effect on a 16×16 mean; worth a comment at least.
- The CFA-channel `match` is inline; see 1.14.

### RISK 1.9: WB eyedropper reports a temperature it did not set

- `handle_wb_picked` (`develop.rs:1007`) clamps temperature and tint to ±1, but the status line prints the unclamped Kelvin.
- It also re-derives the inverse of `kelvin_from_anchor` inline with the literals 120.0 and 50.0. Put the inverse next to the forward function and test the round trip.

### RISK 1.10: `handle_turn_image` is a silent no-op before resources load

- `develop.rs:181`. Pressing ⟲/⟳ while the image is still loading does nothing and says nothing. Either queue the turn or set a status line.

### PERF 1.11: Every `Message` is 21.8 KB, and undo history is unbounded

Measured with a temporary test: `size_of::<EditParams>()` = 21,728 bytes (`BrushData` is 19,976 of it), `size_of::<Message>()` = 21,768, `size_of::<RawEditor>()` = 72,032.

- `Message` carries `EditParams` by value in `ExportRawLoaded` / `ExportPipelineReady` (`message.rs:327–334`) and in `RenderOutput::edit`, with `#[allow(clippy::large_enum_variant)]` at `message.rs:201`. So every message, including each mouse-move and frame tick, is a 21 KB value that iced moves through its queue. `Box` those payloads and `Message` drops to tens of bytes.
- `history_map` (`state.rs:669`) keeps one `Vec<EditParams>` per image visited and never trims it (only deletion removes entries). Each undo step is 21.7 KB, so 50 steps on each of 200 images is about 217 MB that the memory budget does not count.
- Fixes, smallest first: cap steps per image (say 100) and drop history for images not visited recently; or store brush data behind an `Arc` so that steps which did not touch the brush share it (this also ends the "fixed capacity so `EditParams` stays `Copy`" constraint, which is what forces the 20 KB).
- `EditParams` being `Copy` at 21.7 KB also means each `let p = editor.current_edit_params` (there are many, e.g. `view_params`, every handler that compares before/after) is a 21 KB memcpy. Cheap individually, but `view_params()` is called several times per render.

### PERF 1.12: Paste runs SQLite on the UI thread per image

- `handle_paste_settings` (`develop.rs:613`) loads and saves each selected image's edits synchronously. On a selection of hundreds it blocks the window (constraint 2).
- Suspected, not traced: the other images' in-memory undo stacks are not updated, so an undo on one of them after a paste may step back to a state from before the paste without the pasted state being on the stack.
- Fix: one transaction in `spawn_blocking`, then a message that invalidates proxies and history for the ids.

### PERF 1.13: Small per-call costs that scale with the catalogue

- `invalidate_proxies` (`state.rs:1267`) does `ids.contains` inside a loop over images: O(images × ids). `apply_to_images` already builds a `HashSet`.
- `displayed_images()` / `displayed_ids()` (`state.rs:1180`, `:1216`) allocate a `Vec` over the whole catalogue on every call, and are called from 18 places (see section on views for which are per-frame).
- `save_preferences` (`state.rs:1526`) writes the settings file synchronously on the UI thread for every section fold and copy-category tick.

### CLEANUP 1.14

- About 24 `XChanged(f32)` messages each have a one-line handler plus entries in `is_slider_change`, `rail_gradient`, `slider_key`, `reset_section` and `apply_categories`. One `Message::Slider(SliderId, f32)` and a table of `SliderId → (field accessor, section, copy category, range, rail)` would replace all five lists, and would make 1.4 impossible.
- "Turn every other tool off" is written out by hand in many handlers (`leave_other_tools` in `masks.rs:42` is one copy; `develop.rs`, `loupe.rs`, `window.rs`, `ai_mask.rs` have their own, each resetting a slightly different subset). The tools are mutually exclusive, so an `enum ActiveTool { None, Crop, Level, WbPicker, MaskPlacement(..), Brush, ColourPicker(usize), AiSelect(..) }` would make the exclusivity structural, and 1.7 would not be possible.
- The Bayer site → channel `match` in `sample_raw_rgb` should be one `fn cfa_channel(pattern, x, y)` in `raw` shared with the other CPU code that needs it.
- The placeholder size `(1280, 853)` appears in both `buffer_dims` (`state.rs:1157`) and `image_display_dims` (`state.rs:1336`); make it one constant.


## 2. Navigation, loading, export

Files read: `app/handlers/{navigation,loading,export}.rs`, `app/loader.rs`, `gpu/render_functions.rs` (entry points), `core/brush.rs` (data layout). Everything here is by reading; nothing was run.

### BUG 2.1: The selected image's RAW decode is neither bounded nor deduplicated (checked by reading)

- `trigger_image_load` (`handlers/navigation.rs:902–906`) starts `raw::loader::load_raw_data` directly whenever the image is not in `raw_cache` and not in `pending_raw_loads`. A direct load is recorded nowhere: `pending_raw_loads` only tracks the preload queue.
- Holding an arrow key in Develop across uncached images therefore starts one full decode per step. None is cancelled, each holds about 100 MB while it runs, and `MAX_CONCURRENT_RAW_LOADS` (2) does not apply to them. Stepping A → B → A before A finishes decodes A twice.
- The results are harmless (`handle_raw_data_loaded` drops stale ones), so the symptom is memory and CPU spikes and a slow first render after a fast scroll, not a wrong image.
- Fix: route the selected image through the same queue with a reserved slot (the queue already has the cancellation semantics, since dropping an entry drops its subscription), or keep a `direct_raw_loads: HashSet<i64>` and skip a second start. A short debounce (decode only once the selection has rested ~80 ms) would also stop the per-step decodes during a held key.

### BUG 2.2: Prioritising a queued RAW can cancel a running decode (checked by reading)

- `navigation.rs:907–914`: when the selected image is already queued, it is removed and re-inserted at index 0.
- The first `MAX_CONCURRENT_RAW_LOADS` entries ARE the running decodes (`app/loader.rs`, and CLAUDE.md §C: "never reorder or drop those"). If the selected image was at index ≥ 2, the insert pushes the entry at index 1 to index 2. iced then drops that subscription, its half-finished decode is thrown away, and it is decoded again from scratch later.
- Fix: insert at index `min(MAX_CONCURRENT_RAW_LOADS, len)` when the image is not already one of the heads, i.e. "next to start", which is what `requeue` already does for the tail.

### RISK 2.3: `SharedContext` creation can race, and the loser's resources may be installed

- Three places build a context when `editor.gpu_context` is `None`: `loading.rs:97`, `loading.rs:313` (full-res upgrade) and `export.rs:384`. Each does it inside its own async task, so two tasks started before the first finishes both build one.
- `handle_image_resources_ready` returns early for a stale image without storing its context. Select 1 → 2 → 1 during the very first load: task 1 and task 2 each build a context, the result for 2 is dropped as stale (its context with it), and whichever context is stored first wins. A later result whose resources were built on the OTHER device is then installed next to it. Using a texture from one `wgpu::Device` with another's queue is a validation error (logged by the uncaptured-error handler, renders nothing).
- Suspected: I traced the control flow but did not confirm that a second result can pass the staleness check with a different context. The window is the first image load only.
- Fix: create the context exactly once. Either at startup (it is needed for anything useful anyway), or through one shared `tokio::sync::OnceCell<Arc<SharedContext>>` on the editor that all three paths `get_or_try_init`. That also turns constraint 5 ("whoever first creates a context must store it back") from a rule into a structure.

### BUG 2.4: Paste leaves the other images' undo history stale (checked by reading)

- `history_map` is written in three places only: `delete.rs:171`, `navigation.rs:63` (seed on first visit) and `library.rs:457`.
- `handle_paste_settings` writes new edits to every selected image's row but never touches their stacks. For an image visited earlier in the session, the stack top is still the pre-paste state.
- On returning to it: the first Undo goes to `stack[index − 1]`, so it skips back past the state before the paste. Redo then returns to the pre-paste state, and the pasted state is not on the stack at all, so it is lost.
- Fix: in the paste handler, drop `history_map` entries for the pasted ids (simple; loses their history), or push the pasted params onto each existing stack (keeps undo, and makes paste itself undoable per image). This replaces the "suspected" line in 1.12.

### RISK 2.5: Heavy CPU work inside `async` blocks, not `spawn_blocking`

- `prepare_image_resources_from_raw` (`loading.rs`), `trigger_full_res_upgrade` and `handle_export_raw_loaded` do all of this directly in an `async move` block: `ProfileUse::resolve` (scans the profiles folder and parses DCP files), `ml::denoise::effective_mosaic` (a full-mosaic blend), `ImageResources::new` (noise estimate, haze analysis, highlight learning, texture uploads), `ai_mask::load_all` (PNG decodes), and in export `export_mosaic`, which can run PMRID over the whole image.
- They run on tokio worker threads, so the UI thread is not blocked, but a worker is. The render functions use `block_in_place`, which needs a free worker to hand its queue to. With two RAW preloads decoding, an export running PMRID and a full-res upgrade, a small machine can have every worker busy and renders queue behind them.
- Fix: wrap the CPU parts in `spawn_blocking` (constraint 2 says so for decodes and EXIF already) and keep only the GPU submit/poll on the async side.

### RISK 2.6: Export downscale is a plain bilinear tap

- "Resize Long Edge" renders the colour pass straight at the target size (`export.rs:475`, `render_to_vec`). The pass reads `input_texture` with one bilinear `textureSample` (`shaders.rs:1167`; `textureSampleLevel(.., 0.0)` with lens correction on), and the texture has one mip level (`shared.rs`, every `mip_level_count: 1`).
- At a reduction of more than 2× that skips source pixels. A 6000 px frame exported at 1500 px reads 4 of every 16 pixels: fine detail (fabric, foliage, hair, sensor noise) aliases into moiré and sparkle, and the result is noisier than a proper downscale because nothing is averaged. The same applies to the 384 px overview, where it only affects thumbnails and the histogram.
- Not confirmed by rendering. `eval_tests` has no test that compares a resized export with a box-filtered full-resolution one; that would be the check.
- Fix, cheapest first: render at full size and downscale on the CPU with an area filter (`fast_image_resize` or the `image` crate's `Triangle`/`Lanczos3` in linear light) before encoding; or render at the smallest integer multiple ≥ target and box-reduce. A mip chain on the demosaic output is the GPU answer but it interacts with every `textureLoad` neighbourhood read in Pass 2.

### RISK 2.7: Smaller correctness points

- `find_next_unmarked_image_id` (`navigation.rs`) walks the unfiltered `editor.images`, while arrow-key stepping uses the displayed (filtered, sorted) ids. With a filter active, auto-advance after a cull mark can select an image the filter hides, so the filmstrip shows no selection.
- `handle_image_selected` and `handle_process_next_export` call `library.load_edit_params` synchronously on the UI thread. One indexed row, so small, but it is on the arrow-key path and is exactly what constraint 2 forbids. A per-session `HashMap<i64, EditParams>` filled lazily would remove the read from navigation entirely.
- Crop-handle and mask drags (`apply_crop_drag`, `apply_mask_drag`, `navigation.rs:680`) add per-event cursor deltas and clamp. Once a handle hits an edge, further movement is discarded, so on the way back the handle moves immediately and no longer sits under the cursor. Compute from the press position plus the total delta instead (store the rect at press).
- `handle_mouse_pressed` treats any two presses within 300 ms as a double click, with no distance check, and resets the view. Two quick clicks at different places (starting a pan twice) count.
- `unique_output_path` (`export.rs:618`) tests `exists()` and then creates: two exports finishing together cannot collide today because export is serial, but `File::create_new` in a loop would make it correct by construction. After 10,000 taken names it falls back to OVERWRITING the first file; returning an error is safer than destroying an export.
- `handle_cancel_export` marks pending jobs `Failed("cancelled")`. A `Cancelled` state would let the list show them differently from real failures (the count in the header currently adds them to "failed").

### PERF 2.8: Per-step and per-rebuild costs that scale with the catalogue

- One arrow-key step allocates the whole displayed list about five times: `neighbours` three times (preview, RAW and proxy preload scheduling, via `displayed_images()`), then `reveal` and `step_selection` via `displayed_ids()`. Compute it once per step and pass it down, or cache the displayed id list on the editor and invalidate it where the filter, sort or catalogue changes.
- There are 24 linear `images.iter().find(..)` / `.position(..)` lookups by id across `src/`. Some are inside views and so run on every rebuild: `views/cull.rs:56` and `:99`, `views/export.rs:266`, `views/develop.rs:767`. An `id → index` `HashMap` on the editor, rebuilt where `images` is replaced, removes all of them.
- The Export tab's queue (`views/export.rs:182`) calls `export_target_ids` on every rebuild. That is `displayed_ids()` (an allocation over the catalogue), then `ids.contains` per selected id (O(n²)), then `images.iter().find` per id to get the file name (O(selected × catalogue)), then one widget per id with no virtualisation. With 2,000 of 10,000 images selected that is about 24 million comparisons and 2,000 widgets per message while the tab is open. §E's rule applies here: build the list once when the selection changes, and virtualise or cap the rows ("and 1,950 more").
- `load_image_handle` / `load_preview_pixels` (`navigation.rs:1040`) decode the SELECTED image's working preview with `image::open`, while preloads (`loader.rs::process_load_request`) take the zune path under `fast-jpeg`. The image the user is waiting on gets the slower decoder. One shared `decode_preview(path)` fixes it.

### CLEANUP 2.9

- The block "resolve profile → matrix / DCP interpolation → effective mosaic → `ImageResources::new`" exists three times (`loading.rs:130`, `loading.rs:341`, `export.rs:393`) and `ImageResources::new` takes 15 positional arguments, several of the same type in a row. One `build_resources(ctx, raw, image_id, params, subsample)` and a `ResourceSource` struct would make a fourth caller (and the next new field) a one-line change, and would be the natural place for the `spawn_blocking` of 2.5.
- `metadata_snapshot` (`loading.rs`) copies every `RawDataResult` field by hand to get "everything but the mosaic". Split the type into `RawMeta` (small, `Clone`) and the mosaic `Arc`, and the function and its "remember to add the new field here" hazard go away.
- `render_to_vec`, `render_to_bytes` and `render_with_bindings` (`render_functions.rs`) repeat the same submit → `map_async` → `block_in_place(poll)` → `unpad` sequence, and `render_to_bytes_16bit` repeats `encode_color_pass_with` with a different format and pipeline. One `async fn read_back(context, Readback)` and a `format` parameter on the encode function would leave a single copy of the padding and mapping logic.
- `render_functions.rs:35–37` has three stacked doc comments left from earlier versions above `struct Readback`, describing a function that is no longer there.


## 3. GPU resources and render entry points

Files read: `gpu/shared.rs` (all), `gpu/render_functions.rs` (non-test part), `gpu/demosaic.rs` (parameter construction), and the call sites in `handlers/develop.rs`.

### RISK 3.1: The UI thread rewrites the uniform buffer while a render is in flight (suspected, by reading)

- `push_uniforms_and_render` (`handlers/develop.rs:1167`) calls `resources.update_uniforms` FIRST and only then checks `is_rendering` (`:1176`). So during a slider drag the UI thread writes the whole 784-byte uniform block (and the mask block, and `stage_params`) while the previous frame's task may still be on a worker thread between its own writes and submits.
- `update_uniforms` builds the block with `GpuEditParams::from(params)`, whose `view_rect` is `FULL_VIEW_RECT` (`gpu/params.rs:267`). `render_display_and_overview` does: write overview rect → encode → submit → **write main rect → encode (incl. `prepare_stages`) → submit**. `queue.write_buffer` calls are applied in call order at the next submit, whichever thread made them.
- If the UI write lands in the bold window, the main pass renders the WHOLE image (and the newer params) into a target sized for the on-screen window, and `handle_render_finished` publishes it as covering `requested_view_rect`. When zoomed in, that is one frame of the full image squeezed into the visible region. At fit there is nothing to see, which may be why it has gone unnoticed.
- Two quieter effects of the same race: `prepare_stages` reads `stage_params` at render time, so a frame can run noise reduction for the new settings and colour for the old ones (or the reverse); and `RenderOutput::edit` is captured when the render is triggered, so an overview stored as the edited proxy can have been rendered with later params than the edit it is filed under. A pending render always follows, so the final state is right; the transient frames are not.
- I did not reproduce this. A way to check: drag a slider at 400% with a 1000 Hz mouse and `RAW_EDITOR_SLOW_MOTION` off, recording the screen; or assert in `handle_render_finished` (debug builds) that the readback's params generation matches the one captured at trigger.
- Fix: make the render task the only writer. `update_uniforms` would store the CPU-side state (`current_params`, `stage_params`, LUT dirty flags) and the render function would upload it at its start, so every write and submit of one frame happens in sequence on one thread. A smaller patch is a `Mutex<()>` per `ImageResources` held by `update_uniforms` and across each write → submit pair, but that can stall the UI thread for the length of an encode.

### RISK 3.2: Export demosaics and analyses in the edited white balance; the screen uses as-shot

- Develop builds resources with the as-shot multipliers (`loading.rs:130`, comment at `:113`) and applies Temp/Tint through `update_uniforms`. Export passes `wb_final`, the slider-derived override, straight into `ImageResources::new` (`export.rs:362`, `:393`).
- Inside `new`, that argument is not only the uniform's WB. It is the WB the RCD demosaic works in (`GpuDemosaicParams::new`, `gpu/demosaic.rs:89`), the `analysis_wb` the noise model, the denoise plan, the haze analysis and the highlight correction are computed in, and `self.wb_multipliers`, the "as-shot" baseline.
- So for any image with a white-balance edit, the exported file's demosaic direction decisions, NR weights and learned highlight colour come from a different working space than the Develop view. Each stage is internally consistent, so nothing is broken, but "export matches the screen" (which the scale-count and UV-grid choices elsewhere work hard to keep) does not hold exactly for those images. I expect the difference to be small for mild edits and visible in NR chroma behaviour for large ones (tungsten shot corrected by 2× on blue). Not measured.
- Fix: pass `raw_data.wb_multipliers` to `new` and call `update_uniforms(.., wb_override)` afterwards, exactly as `handle_image_resources_ready` does. The shared `build_resources` of 2.9 would do this by construction. `eval_tests` could then assert export == preview for a WB-edited frame.

### RISK 3.3: No handling of a lost device

- `SharedContext::new` installs `on_uncaptured_error` (logging only) but no `set_device_lost_callback`. After a driver reset, a suspend/resume on some laptops, or an eGPU unplug, every readback fails to map; the render functions then return an empty `Vec` (`render_functions.rs`, the `else` arms), the viewport shows nothing, and export writes... see the next point. The only recovery is restarting the app.
- Fix: on device loss, set `editor.gpu_context = None` and drop `image_resources`, so the next load builds a fresh context (the lazy creation in `loading.rs` already exists), and show a status line.

### RISK 3.4: A failed readback is an empty buffer, not an error

- `render_to_vec`, `render_to_bytes_16bit` and `render_display_and_overview` return `Vec::new()` / an empty `Arc` when `map_async` fails. The type gives the caller no way to tell "failed" from "rendered".
- Export: checked `handle_export_pipeline_ready` → `save_export_async`; an empty buffer reaches the encoder, which fails on a length mismatch (PNG, TIFF) and the partial file is removed, so no bad file is written. The user sees an encoder error ("wrong data size") rather than "the GPU readback failed". JPEG: `jpeg_encoder::encode` with a short buffer also returns an error, as far as I can tell from its API; not run.
- Fix: return `Result<_, String>` from all three, as `render_with_bindings` already half-does with `Option`.

### PERF 3.5: Load-time CPU work is repeated and serial

- `ImageResources::new` runs three full passes over the mosaic one after another: `noise::estimate`, `haze::analyse`, `highlights::analyse` (`shared.rs:1060–1098`), each building its own black-subtracted, scaled view of the data. For a full-resolution build (upgrade or export) that is three walks over 24–45 MP on one thread. `std::thread::scope` with three threads would cut the wall time to the slowest of the three with no new dependency.
- A full-resolution upgrade repeats all three for the same image the preview already analysed. Haze and highlights are defined on a UV grid / per image and are meant to agree between preview and export; passing the preview's results into the full-res build would save the work and remove a source of small preview/full-res differences. (The noise model genuinely differs with the stride and must be re-measured.)
- `downsample_bayer` with stride 1 copies the whole mosaic (`shared.rs:598`) just to return an owned `Vec`; `new` already avoids calling it for stride 1, so the branch is dead in the app and only reached by the unit test. Remove it or return a `Cow`.

### CLEANUP 3.6

- `ImageResources` has eight `Mutex`/`RwLock` fields (`stages`, `stage_params`, `ai_bitmaps`, `wb_current`, `forward_matrix`, `current_params`, `last_uploaded_hsm_temp`, `last_uploaded_tone`) that are always updated together in `update_uniforms`. One `Mutex<RenderState>` holding all of them would make the "these change as a unit" rule visible, remove the `if let Ok(..)` on each (a poisoned lock is currently skipped silently for some fields and recovered with `into_inner` for others), and is the natural home for the fix to 3.1.
- `shared.rs:1404`: the doc comment for `pushed_params` ("The params most recently pushed by `update_uniforms`…") sits above `color_bind_group_with`, so rustdoc attaches both paragraphs to the wrong function.
- `ImageResources::new` takes `wb_multipliers`, `black_levels`, `white_level`, `cfa_pattern`, `orientation`, `lens` separately; every caller reads them off the same `RawDataResult`. Passing `&RawMeta` (see 2.9) leaves five arguments.
- Several `pub` fields are only read inside `gpu::` (`texture_view`, `debayer_texture_view`, `hsv_lut_texture`, `tone_curve_texture`, `last_uploaded_*`, `current_params`). `pub(super)` would document that the app layer must not touch wgpu types (constraint 6).


## 4. Remaining handlers: library, delete, proxy, window, loupe, camera profiles, AI denoise

Files read: `handlers/{library,delete,proxy,window,loupe,camera_profiles,ai_denoise}.rs` in full, `handlers/scroll.rs` to line 330, `main.rs`, and the raw-cache functions in `state.rs`.

### BUG 4.1: The selected image's RAW is not pinned, so the eyedroppers can fail for good (checked by reading)

- `sample_raw_rgb` (`handlers/develop.rs:952`) reads the mosaic from `editor.raw_cache` and gives up with "raw data not in memory (reopen the image)" when it is not there. It serves the WB eyedropper (`develop.rs:1014`) and the mask colour picker (`masks.rs:357`).
- `insert_raw_cache` (`state.rs:1498`) returns without storing anything when the RAW preload budget is 0, a value Preferences offers and `handle_set_raw_preload_budget` handles on purpose. With that setting both pickers fail on every image, and reopening, as the message advises, cannot help.
- With a non-zero budget the same thing happens by eviction. The cache is a plain LRU, the selected image is inserted first and its neighbours after it, so the selected image is the OLDEST entry and the first one `evict_raw_cache_to_budget` drops. Defaults (1024 MB, 1 behind + 4 ahead) hold six 24 MP frames comfortably; a 256 MB budget with 45 MP files (about 90 MB each) evicts the open image as soon as two neighbours finish.
- The same missing pin costs time elsewhere: the full-resolution upgrade (`loading.rs:307`), the loupe (`loupe.rs:100`), AI select (`ai_mask.rs:182`) and AI denoise (`ai_denoise.rs:158`, `:440`) each fall back to decoding the file again, and none of them puts the result back. With budget 0, opening one image and using those tools decodes it up to five times.
- Fix: keep the open image's `Arc<RawDataResult>` in its own field (`editor.current_raw: Option<(i64, Arc<..>)>`), outside the budget, set where the develop resources are installed and cleared on image change. Route all six readers through one `RawEditor::raw_for(image_id)` that checks it, then the LRU. That also removes the five copies of the "cached, else re-read" `match`.

### BUG 4.2: A hard delete can write the deleted image's edits onto its neighbour (checked by reading; narrow)

- `handle_hard_delete_complete` (`delete.rs:186`) sets `editor.selected_image_id = fallback` and then emits `Message::ImageSelected(next_id)`. `handle_image_selected` begins with `flush_edits()`, which saves `current_edit_params` under `selected_image_id` (`state.rs:1425`) — now the fallback image, while the params are still the deleted image's.
- It needs `edits_dirty` to be set at that moment. The delete runs in the background (the trash can take seconds for a large selection) and the modal is already closed, so the user can keep working on the doomed image; a slider held, or an arrow-key nudge without a commit, when `HardDeleteComplete` lands is enough.
- Fix: in `handle_hard_delete_complete`, clear `edits_dirty` (the image is gone, there is nothing to save) before moving the selection, or leave `selected_image_id` alone and let `handle_image_selected` move it. Better: disable editing while `is_deleting` is set for the selected image.
- Related: the same handler does not drop `image_resources`, `preview_resources`, `working_preview_bytes` or `rendered_preview_bytes` when the last image is deleted (the folder-removal path at `library.rs:473` does), so the GPU working set of a deleted image stays allocated and its last render stays in state.

### BUG 4.3: Closing the window from the OS skips the save points (checked by reading)

- Flushing edits and saving the open image's edited thumbnail happen in `handle_close_window` (`window.rs:142`), reached only from the title bar's own close button (`Message::CloseWindow`). `main.rs` does not call `exit_on_close_request(false)` and there is no `window::close_requests()` subscription, so Alt+F4, a tiling WM's close binding, the taskbar's Close and a session logout all exit without it.
- What is lost: edits from a gesture that has not reached a commit point (rare), and, every time, the proxy of the open image. The image's edits were saved at the last commit, the proxy on disk is from the last time the user LEFT the image, so the catalogue restarts with a thumbnail that shows an older edit. `handle_image_selected` then stamps that proxy as matching the saved edits (`navigation.rs:66`). That is the §F invariant broken on a common exit path.
- Fix: `.exit_on_close_request(false)` plus `window::close_requests().map(|_| Message::CloseWindow)`. For exits that cannot be intercepted (SIGKILL, power loss), store a hash of the edit a proxy shows next to `cache_path_edited` and drop any proxy whose hash differs from the saved edits when it is loaded; the invariant then checks itself instead of depending on every exit path.

### BUG 4.4: AI denoise is not regenerated for an image opened while another regeneration runs (suspected)

- `after_resources` (`ai_denoise.rs:419`) returns early while `regen.is_some()`. `handle_regenerated` reloads only if the finished image is still selected. If the user stepped to a second image that also lacks its cache, nothing ever starts its regeneration: it stays rendered without AI denoise, with no status line, until it is reopened.
- Fix: at the end of `handle_regenerated`, call `after_resources(editor)` for whatever is now loaded.

### RISK 4.5: Smaller state leaks between asynchronous steps

- `camera_profiles::handle_downloaded` (`:104`) names the camera from `camera(editor)` when the download FINISHES. If the user changed image meanwhile, "RawTherapee has no profile for X" names the wrong camera and `unavailable` is recorded against it. Carry `(make, model)` through the task.
- Changing a per-camera default profile, or installing profiles, changes how every `Auto` image of that camera renders, but their saved edited thumbnails are left alone (`handle_set_default`, `catalog_changed`). They show the old rendering until each image is opened again. Recording the resolved profile's name with the proxy (see 4.3's hash) would catch this too.
- Programmatic selection goes through the click path. `Task::done(Message::ImageSelected(..))` from delete, auto-advance and `ensure_develop_selection_not_marked` reaches `handle_image_selected`, which reads `last_modifiers`: with Shift or Ctrl still held (Ctrl+Backspace, a Shift+digit rating binding) the "move on" becomes a range select or toggles the image OUT of the selection. Split the handler into `select(id, SelectMode)` and have only real clicks derive the mode from modifiers.
- `is_rendering`, `loupe.in_flight` and `render_in_flight` are cleared only by their completion message. A panic inside a render task (they run wgpu and `block_in_place`) never delivers one, and that view stops updating until restart with nothing in the UI. A drop guard that sends the completion, or a timeout check on the next request, would bound it.
- `open_url` (`window.rs:15`) drops the `Child` without waiting, which leaves a zombie process per click on Unix until the app exits. On Windows it goes through `cmd /c start`, which the neighbouring `reveal_folder` comment already notes mangles `&`; the URLs passed today are constants without one.

### RISK 4.6: UI-thread work in folder removal and cache bookkeeping

- `handle_remove_folder_confirmed` (`library.rs:398`) runs `forget_images` and then deletes up to four cache files per image synchronously in `update`. For a 5,000-image folder that is one transaction plus 20,000 `unlink` calls with the window frozen. The hard-delete path next to it already does the same work in `spawn_blocking`.
- The two removal paths repeat the in-memory clean-up (`library.rs:455–486`, `delete.rs:163–176`) and have already diverged (4.2). One `RawEditor::forget_images_in_memory(&HashSet<i64>)` used by both would keep them equal. Neither removes the ids from the export queue.
- `spawn_cache_workers` starts four workers at startup AND after every import (`library.rs:240`), and each completed job starts its own replacement, so two imports in a row run twelve decodes at once against the "capped at 4 to keep memory predictable" comment. Track the number of live workers in state and top up to four.
- The cache claim is `SELECT id … WHERE cache_status = 'pending' LIMIT 1` with no `ORDER BY`. The column is indexed (`idx_images_cache_status`, `database/library.rs:160`), so the claim itself is cheap, but thumbnails are generated in whatever order the index gives, not for the folder on screen. Ordering the claim by "visible folder first" would make a large import feel much faster.
- Every cache completion rewrites `editor.status` ("Caching: 12/500…"), overwriting whatever was there, including "Export failed: …" and "Could not save rating: …" messages the user has not read. Progress belongs in its own field.

### PERF 4.7

- `handle_noise_changed` (`loupe.rs:233`) arms a new 250 ms timer on every slider tick, and every one of them later delivers `NoiseSettled(token)`, which is discarded unless it is the latest. Each delivery still costs an `update` and a full `view` rebuild (§E), so a noise drag pays for twice the messages. Keep one timer: record `Instant::now()` per tick, and when the timer fires either release or re-arm for the remainder.
- `loupe::build` (`loupe.rs:104`) makes the AI-denoised copy with `RawDataResult { data, ..(*raw).clone() }`. The struct-update syntax clones the whole original first, mosaic included (48 MB at 24 MP), only to drop that `data` at once. Clone the metadata fields only, or split `RawDataResult` into `Arc<[u16]>` + `RawMeta` (see 2.9), which also lets the loupe and the modal's engine share one buffer.
- `handle_open` in `ai_denoise.rs:164` measures the noise model over the full mosaic again (`ml::denoise::noise_model`); `ImageResources::new` measured it already (3.5). Keep the unit-range model on `ImageResources` or the cached RAW.
- Filmstrip momentum (`scroll.rs:150`) keeps ticking `scroll_by` every frame until the velocity decays below `STOP_SPEED`, even after the strip has hit either end; a hard flick at the end of the strip costs a second or two of frames that move nothing. Zero the velocity when the offset stops changing.
- `handle_remove_folder_confirmed` filters with `ids.contains(&img.id)` (a `Vec`) inside a pass over all images: O(N·M). The `gone` set built a few lines later should be built first and used there.

### CLEANUP 4.8

- `handle_reset_preferences` (`window.rs:340`) and `save_preferences` (`state.rs:1526`) each list every setting by hand, as does start-up. A new setting has to be added in three places and a miss is silent (reset does not touch it, or it is not saved). `AppSettings` could be a field of `RawEditor` (`editor.settings.thumbnail_size`), which removes all three lists.
- `handle_escape` repeats `handle_close_modal`'s body (`window.rs:216`, `:226`).
- `proxy::persist_now` calls `write()` for the `Clear` case and drops the returned `Task`; it is correct only because that branch of `write` happens to be synchronous. Split `write` into `clear_saved(editor, id)` and `save(editor, id) -> Task`.
- `handle_import_from_camera` is a stub that sets "Camera import is not implemented yet.", and the Import screen still shows its button (`views/library.rs:613`). Hide the button until the feature exists.


## 5. Database, and one AI denoise interaction

Files read: `database/library.rs` (all non-test code), `core/types.rs` lines 471–662, `gpu/denoise.rs` `DenoiseSettings::from_params`. Everything here is from reading; nothing was run.

### BUG 5.1: The before/after peek applies classic NR on top of AI denoise (read, not run)

- `EditParams::before_reference` (`core/types.rs:637`) keeps the three NR sliders but not `ai_denoise`, which falls back to 0.
- `DenoiseSettings::from_params` (`gpu/denoise.rs:109`) zeroes the NR sliders only when `ai_denoise > 0`. With the peek's params it sees 0 and uses the sliders.
- The mosaic on the GPU is still the AI-denoised one: `ai_denoise::resources_stale` compares against `current_edit_params`, so the peek does not rebuild.
- Result on an image with AI denoise applied and non-zero NR sliders (the default has Colour 0.3): "Before" shows the AI-denoised image with classic NR added, which is neither the edit nor the original, and the NR stage re-runs on press and on release. The comment on `before_reference` says keeping NR exists to avoid exactly that re-run.
- Fix: add `ai_denoise: self.ai_denoise` to the preserved list, with a line in the existing `before_reference` tests.

### RISK 5.2: An edit that fails to load is replaced by defaults, then saved over

- `load_edit_params` returns `Err` both for "no row" and for "the JSON did not parse" (`library.rs:459`). The callers do not tell them apart: `navigation.rs:59` and `develop.rs:654` use `unwrap_or_default()`, `export.rs:279` uses `.ok()`.
- So if a saved edit ever fails to parse (a field changes type, an enum variant is renamed, a row is damaged), the image opens unedited with no message, and the first flush writes the defaults over the saved JSON. Export of that image silently produces the unedited frame.
- Every field added since the first release carries `#[serde(default)]`, so this is not happening today; it is one careless type change, one forgotten attribute, or one NaN (RISK 12.1) away, and the loss is silent and permanent.
- Fix: return `Ok(None)` for no row and `Err` for the rest. On `Err`, show it in the status line, do not mark the image as saveable (or copy the old JSON to a `settings_json_backup` column first), and make export fail that image by name.

### RISK 5.3: `edits` allows several rows per image

- `save_edit_params` (`library.rs:405`) selects the newest row, then updates or inserts. There is no `UNIQUE(image_id)`, and the select's error is turned into "no row" with `.ok()`, so any failure there (a busy database, say) inserts a second row.
- Readers take `ORDER BY id DESC LIMIT 1`, so a duplicate is harmless to the user, but `ai_mask_references` and `delete_edits` have to cope with it and the table can only grow.
- Fix: a unique index on `image_id` and one `INSERT … ON CONFLICT(image_id) DO UPDATE` statement. That is also one statement instead of two on the UI thread at every flush.
- Foreign keys are off (documented in `apply_pragmas`), so nothing stops an `edits` row for an image that no longer exists; BUG 4.2's late flush creates one.

### RISK 5.4: Schema migration ignores every error

- Columns are added with `let _ = conn.execute("ALTER TABLE … ADD COLUMN …")` (`library.rs:125–157`). "Duplicate column" is the expected error, but a real one (read-only file, full disk, locked database) is dropped too, and the app then fails later on the first query that names the column, with an error that points nowhere near the cause.
- `PRAGMA user_version` is already used, but only for the cache-tier version. Fix: check `PRAGMA table_info(images)` for the column before adding it, or give the schema its own version number (a one-row `meta` table) and run numbered migrations, which constraint 8 in CLAUDE.md says does not exist yet.

### PERF 5.5: Start-up reconciliation runs before the catalogue is shown

- `load_database_blocking` (`library.rs:654`) does, in order and before returning any image: `verify_cache_paths` (up to three `exists()` per cached image), `verify_files` (one per image), `ai_mask_references` (reads every `edits` row's JSON), the AI-mask sweep and the edited-proxy sweep.
- For 50,000 images that is about 200,000 `stat` calls before the Library appears. On an internal SSD it is a second or two; on a spun-down external drive or a network share it is the whole start-up time, and the window shows nothing meanwhile.
- Fix: return `get_all_images()` first, show it, then run the reconciliation as a second background task that sends back only the rows that changed. The requeue of `processing`/`failed` rows must still run before the cache workers start.

### CLEANUP 5.6

- `get_all_images` reads `flag` with `row.get(8).unwrap_or(0)` although the query already wraps it in `COALESCE(flag, 0)`; the `unwrap_or` only hides a type error.
- `Library::conn()` hands out the raw connection (`library.rs:176`); callers outside the module can then write SQL that the "update every query with the schema" rule cannot find by reading `library.rs`. Worth checking who uses it and moving those queries in.
- Not checked: whether a UI-thread write (`flush_edits`, ratings) can wait on a cache worker's write. Each connection waits for the lock up to its busy timeout, so a slow worker transaction would freeze the window for that long. The worker transactions I read are single statements, so this should be short in practice.


## 6. RAW loading, cache tiers, DCP profiles, export EXIF

Files read: `raw/{loader,processor,jpeg,dcp,exif}.rs`, `core/camera_profiles.rs`. Everything here is from reading; nothing was run.

### BUG 6.1: A DCP without ForwardMatrix tags renders with the wrong matrix (suspected)

- `interpolate_at_temperature` (`raw/dcp.rs:366–367`) does `forward_matrix_1.unwrap_or(color_matrix_1)` and the same for 2. A ColorMatrix maps XYZ → camera; a ForwardMatrix maps white-balanced camera → XYZ D50. They are not interchangeable, they are roughly each other's inverse.
- Nothing rejects such a profile earlier. `camera_profiles::validate` (`core/camera_profiles.rs:543`) only requires a non-identity ColorMatrix1, and the only other reads of `forward_matrix_1` are log lines.
- Two ways to reach it:
  - A profile with no ForwardMatrix at all (allowed by the DNG spec; common in older and hand-made profiles). The whole image goes through XYZ → camera as if it were camera → XYZ.
  - A single-illuminant profile with ForwardMatrix1 only. `illuminant2` defaults to 21 (D65) when its tag is absent (`dcp.rs`, `parse_dcp`), so the weight is not 0, and the blend is ForwardMatrix1 against ColorMatrix1. The colour error then changes with the Temp slider.
- Fix: treat a missing second illuminant as single-illuminant (copy matrix 1 into 2, or force the weight to 0). For a profile with no ForwardMatrix, either build camera → XYZ D50 from the ColorMatrix the way the DNG spec describes (invert `ColorMatrix`, normalise by the camera neutral, Bradford-adapt to D50), or use the built-in matrix path for the matrix and keep only the profile's tables and curve. Reject in `validate` until one of those exists.

### RISK 6.2: Loader edge cases that develop wrongly instead of failing (`raw/loader.rs`)

- **Float data** (`:102`): samples are scaled by 65535, but black and white levels are taken from metadata in the file's own scale. Suspected wrong for float DNGs. Reject float input with a reason, or rescale the levels with the data.
- **Unknown CFA name** (`:143`): `_ => 0` treats anything unrecognised as RGGB. A 2×2 sensor that is not Bayer (RGBE, CYGM) passes the layout check and develops with wrong colours. It should be an `Err` like the other unsupported layouts.
- **Truncated file**: `crop_in_place` (`:374`) stops at the end of a short buffer, and its test shows it can return fewer rows than asked. With zero complete rows the loader returns height 0 and empty data with no error, and the zero-sized texture fails later, far from the cause. Return `Err` when `rows == 0`, and consider treating any shortfall as an error since the image is cut off.
- **Black levels** (`:172–175`): metadata levels are truncated with `as u32`, so a fractional level (rawler reports rationals) loses up to 1 DN, and `measured_black_levels` is then built from the truncated values. Keep them as `f32` to the shader, which already takes floats.
- **White level** (`:285`): only the first channel's value is used. Per-channel white levels exist (some Canon and Fuji files); the demosaic pass already caps each colour separately, so the per-channel values could be passed through.
- **Non-UTF-8 path** (`:314`): `path.to_str().unwrap_or_default()` hands kamadak an empty path, so EXIF is silently missing for that file. Pass the `Path`.
- **Two parses** (`:75–77`): `rawler::decode`, then `get_decoder` + `raw_metadata` on the same source. The decoder object can be created once and asked for both.

### RISK 6.3: The thumbnail and Develop can disagree about rotation (suspected)

- `processor::exif_orientation` (`raw/processor.rs:110`) asks rawler only when kamadak cannot open the container (`Err`).
- The loader's `ExifInfo::fill_from` fills every field kamadak left empty, including the case where kamadak opened the file but found no Orientation tag.
- For such a file the thumbnail is built with orientation 1 and Develop uses rawler's value. Fix: share one function, "kamadak's value, else rawler's", between both.

### RISK 6.4: DCP parsing trusts the file (`raw/dcp.rs`)

- `read_ifd` allocates `vec![0u8; count × type_size]` (`:100`) before reading, with `count` straight from the file: a damaged or hostile profile can ask for up to 4 GB. Downloads are capped at 16 MB but the user's own folder is not. Check `offset + total` against the file length first.
- `tag_floats` for FLOAT (`chunks(4)`) indexes `c[3]` without the length filter the two rational branches have. It is safe today only because `total` is a multiple of 4; add the same filter.
- `hue_sat_dims.0 * .1 * .2` is a `u32` multiply on file values: it panics in debug builds and wraps in release for absurd dimensions.
- `eval_curve_points` (`:449`) divides by `w[1].0 - w[0].0`; two points with the same x give NaN in the baked LUT. Skip zero-width segments.
- It interpolates the ProfileToneCurve linearly. The DNG spec asks for a spline; with Adobe's 1025-point curves the difference is nil, with a hand-made 5-point curve it is visible as slope breaks.
- `illuminant_to_kelvin` (`:329`) knows only codes 17–23; anything else is 5000 K. Codes 1 (daylight), 3 (tungsten), 10 (cloudy), 11 (shade) and 24 (studio tungsten) do appear in profiles and would shift the dual-illuminant blend.
- ProfileHueSatMapEncoding (51107), ProfileLookTableEncoding (51108) and DefaultBlackRender (51110) are not read. A table marked sRGB-encoded on its value axis would be sampled as linear. Not checked how many real profiles set them.
- `parse_dcp` opens the file twice (once for the byte order, once in `read_ifd`); `read_ifd` could return `le`.

### RISK 6.5: Profile download and matching (`core/camera_profiles.rs`)

- The listing carries each file's git blob SHA and size, and the manifest stores the SHA, but the downloaded bytes are never checked against either. The check is one SHA-1 over `"blob {len}\0" + bytes`.
- GitHub's contents API returns at most 1,000 entries per directory, without an error past that. RawTherapee's folder is large and growing; when it passes the limit, some cameras silently stop being offered. The git trees API has a higher limit and reports truncation.
- `update_downloaded` uses `?` inside its loop: one failed download discards the report of the ones already updated.
- `validate` writes the bytes to a probe file just to parse them, and the probe is named by process id only, so an import and a download running together share it. Let `parse_dcp` take a `Read + Seek`.
- `Catalog::default_for` returns the first match while iterating a `HashMap`. If two saved defaults match one camera (the name and an alias), which wins changes between runs.
- An untagged profile matches by `file.contains(model)`: model "D5" matches "Nikon D500.dcp" and "D5000". Match whole tokens, as tagged profiles do.
- A profile dropped into the folder by hand with a name over 120 bytes is listed but cannot be chosen (`ProfileChoice::named` returns `None`). Skip it in `scan` with a log line, or show why.

### RISK 6.6: Export EXIF (`raw/exif.rs`)

- `export_app1` reads with kamadak only. RW2 and ORF, which kamadak cannot open, export with no EXIF at all (no capture time, no lens), although the loader already has that data from rawler for display. Build the kept fields from `ExifInfo` when the reader fails.
- Every GPS field is copied into every export and there is no setting for it. Location in shared files is a privacy matter; an "Include location" checkbox in the export panel is the usual answer.
- Software is written as "RAW Editor"; the app's name is "RawEditor".

### PERF 6.7: Import reads far more than it needs (`raw/processor.rs`)

- `process_image` reads the whole RAW (25–100 MB) for each cache job (`:59`) and scans all of it for JPEG markers. A 50,000-image import reads over 1 TB to make thumbnails. The preview's offset and length are in the TIFF IFDs for most formats (rawler's `Decoder` trait also has preview and thumbnail methods; not checked how complete they are in 0.7.2); read that range and keep the full scan as the fallback.
- The embedded JPEG is decoded at full size (often the full sensor resolution) to make a 1280 px tier. A DCT-domain scaled decode (1/2, 1/4, 1/8) would cut decode time several times; `jpeg-decoder` (`Decoder::scale`) and turbojpeg offer it; I did not check zune-jpeg.
- `largest_embedded_jpeg` searches with `windows(3).position`; `memchr::memmem` is several times faster on a 50 MB buffer. Minor next to the two above.
- image 0.25 already decodes JPEG through zune-jpeg, so the `fast-jpeg` feature may now buy little. Suspected; measure before keeping the extra code path. `Cargo.lock` also holds two versions of `image` (0.24.9 and 0.25.8).

### CLEANUP 6.8

- Tiers are sized by width, not by long edge (`resize_to_width`): a portrait frame gets a 1280×1920 working preview and a 256×384 thumbnail, 2.25× the pixels of a landscape one, against the documented "256/384/1280 px".
- `save_tier` writes straight to the final path at the encoder's default quality. A crash mid-write leaves a truncated JPEG that the row then points at. Write to a temporary name and rename, as the profile and model downloads do.
- The tests in `processor.rs` exercise a local `compute_max_height` helper that production code does not call, and one asserts a local variable it has just set. They test nothing in the module.


## 7. Brush and AI mask layers, haze, denoise, local contrast, loupe

Read: `gpu/{brush,ai_mask,denoise,local_contrast,loupe}.rs` (Rust halves; the WGSL strings were skipped), `core/{brush,haze}.rs`, and the stage code in `gpu/shared.rs` 1205–1420. Everything here is from reading; nothing was run.

No bug found in `core/haze.rs` (`HazeModel::default()` is a 1×1 map, not empty; `analyse`'s clamps cannot panic), in the Rust half of `gpu/denoise.rs`, or in `gpu/local_contrast.rs` (`MAX_PASSES` = 27 is exactly the largest pass list `run` can build). The loupe plans its denoise with the same WB (`dp.ceil`) as the main stage's `analysis_wb`, so the two agree.

### PERF 7.1: Every brush point redraws every stroke of every mask

- `prepare_stages` re-rasterises when `st.brush_key != Some(params.brush)` (`shared.rs`, brush stage), which compares the whole 20 KB `BrushData`. Any change, including one new point on the live stroke, calls `BrushRaster::rasterize`.
- `rasterize` (`gpu/brush.rs`) uploads all points and strokes, clears all `MAX_MASKS` layers, then draws every stroke again. Each stroke is a scissored full-screen triangle whose fragment shader loops over all of that stroke's segments.
- So painting costs (pixels in every stroke's bounding box) × (segments in that stroke), once per accepted mouse move, and it grows as the mask fills up. A mask with a hundred long strokes redraws all hundred to add one dab.
- Options, smallest first:
  1. Key per layer: clear and redraw only the layers whose strokes changed. Painting on mask 3 then leaves the other seven alone.
  2. Keep committed strokes in the layer and draw only the live stroke on top (a second "live" layer, merged on release). This makes a drag cost one stroke, however full the mask is. Erase strokes make order matter, so the merge has to happen at `finish`.
  3. Draw each segment as its own quad (a capsule) instead of one triangle per stroke looping over segments. Fragment cost then follows the painted area, not bounding box × segment count.

### PERF 7.2: Brush and AI layer arrays are all-or-nothing

- The brush array is `MAX_MASKS` layers of R16Float at up to 2048 px on the long side: about 45 MB at 3:2, allocated as soon as one stroke exists on one mask.
- `prepare_ai_masks` re-uploads all `MAX_MASKS` slots whenever any id, the orientation or the bitmap generation changes. For each slot it rotates and resizes the bitmap on the CPU (`Bitmap::to_display(..).resized(..)`), and for an empty slot it allocates a zero buffer the size of a layer. During AI Select every click changes one id, so every click redoes all eight.
- Fix: keep the previous per-slot key and upload only the slots that differ. Allocate layers for the highest slot in use, or accept the fixed size and only fix the upload.
- `rebuild_mask_group` creates fresh placeholder textures on each call. Make them once in `SharedContext`.

### PERF 7.3: Stage targets are never released

- `st.denoise` holds two `Rgba16Float` targets the size of the demosaic output; `st.local` holds two more at the analysis size (halved once above 12 MP), plus the coarse pair. At 24 MP full resolution that is about 384 MB for denoise and 96 MB for local contrast, on top of the 192 MB demosaic texture.
- They are created on first use and kept until the `ImageResources` is dropped. The comment at `shared.rs:698` says denoise is kept on purpose so that turning NR back on is free. The cost is that an image where NR was tried once and set back to zero holds 384 MB for as long as it stays open, and after a full-resolution upgrade the preview set (`preview_resources`) holds its own copies too.
- A 4 GB card (the GTX 1650 this is tested on) has little slack at 45 MP: demosaic 360 MB + denoise 720 MB + local 180 MB, before the preview set and the loupe's crop.
- Suggest: drop `st.denoise` when NR has been off for a commit (not mid-drag), and drop the full-resolution set's stage targets when the view goes back to fit, since the display then renders from the preview set anyway.
- `analysis_size` halves once only, so `MAX_ANALYSIS_PIXELS` is not a maximum: a 100 MP mosaic is analysed at 25 MP (two 200 MB targets). Loop the halving, or rename the constant.

### RISK 7.4: Brush capacity is shared and silent (not checked)

- `BrushData` holds `MAX_STROKES` 128 and `MAX_POINTS` 2048 across ALL masks of an image. `begin` returns `None` when full (`handlers/masks.rs:259` tests it). Whether the user is told why the brush stopped painting was not checked; if not, it looks like a dead tool.
- 2048 points go fast: `extend` keeps a point every `radius × 0.25`, so a small brush on a detailed edge can spend several hundred points on one stroke before `finish` simplifies it. The live stroke is not simplified until release, so one long drag can fill the buffer by itself.
- This cap exists only so `EditParams` stays `Copy` (see 1.11). Moving strokes behind an `Arc<Vec<_>>` removes the cap, the 20 KB copies and the whole-struct comparison in 7.1 together.

### RISK 7.5: `denoise::plan` divides by the WB multipliers (not checked)

- `plan` computes `inv_wb = 1.0 / wb[c]` from `analysis_wb`, which is the demosaic `ceil` (the as-shot multipliers). A file whose metadata gives a zero or NaN multiplier would put `inf` into every denoise threshold. Whether the loader guarantees positive multipliers was not checked; a `max(1e-3)` at the point `analysis_wb` is built would settle it for denoise, haze (`set_haze` divides by it too) and local contrast.

### CLEANUP 7.6: Loupe

- `Loupe::render` builds a new colour bind group on every frame (`main.color_bind_group_with`). There are only three possible inputs (the crop's demosaic view and its two denoise targets), so they can be built with the crop and reused.
- `ensure_crop` returns `Err("the preview area moved past the prepared crop")` when an override does not cover the window. That is a normal race while the AI-denoise modal's preview is dragged, not a fault; if the handler logs it at error level it will be noisy. Not checked how the handler treats it.


## 8. Core: noise model, curves, tone mapping, settings, updates, memory, lensfun, lens database

Everything here is from reading the code. Nothing was run. Hand-computed numbers are marked.

### BUG 8.1: the Filmic operator turns very bright highlights grey, then black

`core/tonemap.rs:358` (`filmic`), and the same formula in the shader's built-in block at `gpu/shaders.rs:1753`.

- The midtone lift is `x * (1.08 - 0.08 * x)`. That is a parabola: it peaks at x = 6.75 and reaches zero at x = 13.5.
- The tone LUT covers scene values up to `TONE_LUT_MAX` = 16, so the falling part is inside the baked range.
- Hand-computed: `filmic(6.75)` ≈ 0.963, `filmic(10)` ≈ 0.951, `filmic(13)` = 0.52, `filmic(13.5)` = 0.
- So with Filmic selected, a highlight pushed past about 6.75 (a light source, or a bright sky with Exposure raised 3 stops or more) gets darker again, and past 13.5 it is black.
- The tests do not catch it: `OPERATORS` (line 390) lists Reinhard, Hable, ACES and GT only, so `every_operator_is_monotonic_and_bounded` and `bake_produces_a_full_monotonic_table` skip Filmic. `filmic_matches_shader_form` stops at x = 3.
- The shader's own block (Camera with no profile curve) has the same parabola per channel. Not checked: whether values above 6.75 can reach it there. On the DCP path the 0.7 knee runs first for Camera, which keeps values under 1, so that path is probably safe; the profile-less path has no knee.
- Fix: clamp the lift's input at its peak (`x.min(6.75)`), or replace the lift by a form that keeps rising. Change both sides together, and add Filmic to `OPERATORS`.

### RISK 8.2 (suspected): one dead-flat brightness bin can collapse the noise model

`core/noise.rs`, `flat_variance` and `fit_lower_envelope`.

- `flat_variance` starts from the 10% quantile of a bin's block variances. If 10% or more of the blocks in a bin have exactly zero residual (black-clipped borders, a dead-flat synthetic area, a blown region where every site reads the same), the bin's variance comes out as 0.
- `fit_lower_envelope` weights each bin by `n / (v * v).max(1e-30)`. A bin with `v = 0` gets a weight about 10³⁰ times any other, so it alone decides the fit.
- The model then collapses to almost no noise, and every NR threshold (they are in units of its σ) becomes tiny: the sliders do almost nothing on that image.
- Fix: floor each bin's variance at the quantisation variance (1/12 DN², in the model's units), and cap the weight ratio between bins.
- Not checked: whether clipped bins are already excluded before the fit. If they are, only the black end is exposed.

### RISK 8.3: a curve whose points all sit on the diagonal is drawn but not applied

`core/curves.rs:76`.

- `is_identity` is true when every point has `x ≈ y`.
- `eval_with` is flat outside the first and last points, and `move_point` lets the end points move inwards.
- So a curve such as (0.2, 0.2)–(0.8, 0.8) clips shadows and highlights when evaluated, yet counts as identity, `curve_active` stays 0, and the edit has no effect while the editor shows it.
- Fix: identity also needs the first point at x = 0 and the last at x = 1 (or evaluate the end segments as extensions of the diagonal).
- Smaller: `from_points` removes points closer than `MIN_POINT_GAP × 0.5`, while `insert` refuses at `MIN_POINT_GAP`. Pick one.

### RISK 8.4: a settings file that fails to parse is silently replaced by defaults

`core/settings.rs`, `load_or_default`.

- Nine older fields have no `#[serde(default)]`: `cache_capacity`, `raw_preload_budget_mb`, `thumbnail_size`, `auto_advance`, `histogram_enabled`, `preview_preload_behind`/`ahead`, `raw_preload_behind`/`ahead`.
- A file missing any of them (an older build, a hand edit) or damaged in any way fails to parse. The app logs a warning and starts on full defaults.
- The next `save_preferences` then overwrites the file. That loses the per-camera profile defaults, the update-check answer (the first-run prompt comes back), the folded sections and the appearance.
- Fix: put `#[serde(default)]` on the struct, so any missing field takes its default. Before writing over a file that could not be read, rename it to `settings.json.bad`.

### PERF 8.5: the thumbnail-size slider writes the settings file on every step

`app/handlers/window.rs:291–293`.

- `handle_set_thumbnail_size` calls `save_preferences` per slider change: a synchronous write and rename on the UI thread for each mouse move (constraint 2).
- This adds to the note on synchronous settings writes in section 4. `handle_set_min_free_ram` (`window.rs:346`) was not checked.
- Fix: save on release, as the Develop sliders do with `commit`.

### RISK 8.6: "packaged" is decided by a path prefix that is too wide and too narrow

`core/updates.rs:78` (from commit `fd91513`).

- Any executable under `/usr/` counts as packaged. That includes `/usr/local/`, where a manual install lives: it never checks for updates and is told "your package manager".
- Flatpak (`/app/`) and Nix (`/nix/store/`) installs are not recognised, so they would offer a download that the user should not install by hand.
- Fix: exclude `/usr/local/`; treat `/app/` with `FLATPAK_ID` set, and `/nix/store/`, as packaged. `RAWEDITOR_PACKAGED` at build time stays the reliable route.

### RISK 8.7: the memory gate does nothing on macOS

`core/memory.rs`.

- `available_bytes()` returns `None` on anything but Linux and Windows.
- So the export memory gate and the "minimum free RAM" preference have no effect on macOS, which is a shipped platform. Not checked: whether Preferences hides the control there.
- On Linux it reads `MemAvailable`, which ignores cgroup limits (containers, systemd slices).
- Fix: on macOS use `host_statistics64` (free + inactive pages), or hide the preference where it cannot work.

### RISK 8.8: lensfun download

`core/lensfun.rs`.

- `check()` asks both mirrors one after the other even when the first one answered. A dead second mirror adds its whole timeout (up to 60 s) to every check. Fix: stop at the first good answer.
- `unpack` caps each file (`MAX_FILE` 16 MB) and the file count (`MAX_FILES` 2000), but not the total. A hostile or broken archive can expand to about 32 GB in memory. Fix: a total cap (the real database is a few MB).
- `swap_in` renames the live folder to `.updates-old`, then the staging folder into place. If the process dies between the two, there is no database, and startup does not restore `.updates-old`. Fix: restore it at startup when the live folder is missing.
- `install` parses every XML file twice (once to validate, once to load). Minor.

### CLEANUP 8.9: lens database

`core/lens.rs`.

- `LensDb::load` removes duplicates with a linear scan per lens, so loading is quadratic in the database size, on every reload. A `HashSet` of (maker, model, crop) keys makes it linear.
- `LensCalibration::interpolate` uses only the vignetting rows measured at the longest focus distance. That is a fair default, but close-up shots get the infinity correction; the subject distance from EXIF is not used.
- `gpu_params` falls back to crop 1.5 and aspect 1.5 when there is no profile. The manual distortion and vignetting sliders are scaled by those, so on a full-frame or 4:3 camera with no profile and no entry in the lensfun camera table the manual sliders act a little differently. `ctx.camera_crop` covers the usual case.
- Nothing wrong found in `interp_focal`, `constrain_scale`, or the calibration parsing.

### No findings

`core/highlights.rs`, `core/paths.rs`, `core/desktop.rs`, `core/viewport.rs`, `gpu/params.rs` (non-test part).


## 9. AI models, SAM, AI masks, PMRID cache; update/model/lensfun handlers; viewport overlay; mask handlers

Everything here is from reading only; nothing was run.

### RISK 9.1: Denoise cache can be applied to the wrong image
- `ml/denoise.rs` finds a cache by image id (`cache/denoised/<image_id>-<model tag>.bin`) and `load_cache` validates magic, width and height only.
- Image ids are `AUTOINCREMENT` and restart at 1 when the database is deleted (the documented migration path, constraint 8), while the cache folder survives. A new image with the same id and sensor size passes `has_cache` (`handlers/ai_denoise.rs:340`) and `load_cache`, and another photo's mosaic is blended into it.
- Fix: store a source fingerprint in the header (file size + mtime, or a hash of a mosaic sample) and check it; or clear `cache/denoised` when a new database is created.

### RISK 9.2: Denoise cache files are never removed
- No code deletes a cache file (grep found none): not on Remove, not on image delete or folder removal, not when the model hash changes (old `-<tag>.bin` files stay). About 48 MB each at 24 MP, so disk use only grows.
- Fix: delete on Remove and on image delete, and sweep at startup like AI masks and edited proxies (keep files whose image has `ai_denoise > 0` and whose tag is the current model's).

### RISK 9.3 (suspected): model download timeout
- `ml/models.rs` `agent()` sets `timeout_recv_body(60 s)` and the comment calls it a per-read inactivity limit. In ureq 3 it is, from memory (not checked), a limit on the whole body. If so the 109 MB SAM encoder must arrive in 60 s (about 15 Mbit/s) or the download fails every time.
- Fix: no body timeout plus an inactivity check in the `receive` loop, or a limit scaled by file size. There is also no resume: a failed download restarts from zero (HTTP Range onto the `.part` file).

### BUG 9.4: Brush, range and AI masks draw radial handles at the image corner
- `ui/preview_renderer.rs` `draw_mask_overlay` and `mask_handle_positions` branch on `mask_type == 0`, else radial. Brush (2), range (3) and AI (4) masks have `ax = ay = bx = by = 0`, so they take the radial branch: three pins stacked on UV (0,0) (the image's top-left, or off the crop) and a rotation pin 45 px to its right.
- When the brush is not armed (always for range and AI masks) those pins are grabbable (`:1088`), and a drag writes centre/radius/rotation into a mask that does not use them, marking edits dirty.
- Fix: return early from both functions unless the type is linear or radial.

### 9.5: ending an AI-select session after the image changed → see BUG 1.1
- This is BUG 1.1, which has the order confirmed by reading: `update()` runs `dispatch` and then `ai_mask::track`, so `handle_image_selected` has already swapped `current_edit_params` when `handle_done` deletes the session's slot.
- One addition: `handle_mask_ready` writes `ai_id` into `masks[slot]` whatever that slot's type, so deleting a lower-numbered mask while a session is open would redirect it. Not checked whether the UI allows deleting a mask during a session.

### RISK 9.6 (minor): update check, lensfun reload
- `handlers/updates.rs`: `manual` travels in the message. Clicking Check for Updates while the startup check is running starts nothing, the startup result arrives with `manual: false`, and the dialog may never show a result. Keep a "manual check pending" flag in `UpdatesState`. `handle_open_page` also reports "Opened …" whatever `open_url` returned.
- `handlers/lensfun.rs` `database_changed` clears `raw_cache`, but RAW preloads already decoding return with the old lens lookup and are inserted into the cleared cache. Bump a database generation and drop results decoded under an older one.
- `handlers/lens.rs`: defringe hues use `rem_euclid(360.0)`, so exactly 360 becomes 0 (slider range not checked).
- `handlers/masks.rs:214-215`: `value.clamp(0.0, mask.luma_hi)` and `clamp(mask.luma_lo, 1.0)` panic if a saved edit holds `luma_hi < 0` or `luma_lo > 1` (`f32::clamp` panics when min > max). Use `min`/`max`.
- `handle_brush_stroke` returns before `StrokeEvent::End` when the brush was disarmed mid-stroke, so `is_painting` stays true and the stroke is never finished or committed.

### PERF / CLEANUP 9.7
- `views/preferences.rs:751`: `is_installed` runs in `view` (three `fs::metadata` calls per rebuild while the AI models page is open). Cache it in `ModelsState`.
- `ml/runtime.rs`: sessions take every logical core, so the UI and render thread compete with PMRID or the SAM encoder. Leave one core free or use physical cores.
- `ml/segment.rs`: `decode` copies about 16 MB of embeddings per click; `prepare` holds about 130 MB of transient copies at 24 MP.
- `ml/denoise.rs`: `effective_mosaic` reads 48 MB, converts to a second 48 MB vector and blends into a third on every resource build; `tiled` holds about 300 MB of transients at 24 MP.
- `ml/masks.rs`: the prompts JSON is written before the PNG and not atomically; a leftover `<id>.png.part` of a referenced id is never swept.
- Each AI-select click writes a new PNG + JSON, kept until the next startup sweep.

### Settled questions
- Eyedroppers send cropped-display UV; `sample_raw_rgb` undoes the crop, so the two agree (1.2 still stands).
- The user is told when the brush buffer is full (`masks.rs:260`).

### No findings
`ml/mod.rs`, `handlers/models.rs`, the tiling/ramp/k-sigma code in `ml/denoise.rs`, the viewport shader widget and crop overlay in `ui/preview_renderer.rs` apart from 9.4.


## 10. Keyboard shortcuts, `EditParams` helpers, paste (second pass over `state.rs` and `core/types.rs`)

Files read: `app/state.rs` lines 1050–1750 (edit, history, proxy and cache methods; `subscription`), `core/types.rs` lines 425–832, `handlers/develop.rs` `handle_paste_settings`, `core/brush.rs` (de)serialisation. Everything here is from reading; nothing was run.

### RISK 10.1: single-letter shortcuts fire with a modifier held

- `subscription()` (`state.rs`, the keyboard block) handles the `command()` combinations first (C, V, A), and falls through to the single-key `match` for everything else. That `match` does not look at the modifiers, except for Z and Y.
- So:
  - Ctrl/Cmd+R → `ResetEdits` (all edits on the image; undoable, but a surprise for anyone with a browser reflex).
  - Cmd+W on macOS → `ToggleWbPicker`, where the user expects the window to close.
  - Ctrl+D → `MarkForRemovalConfirmed`.
  - Ctrl/Alt+1…5, Ctrl+P/X/U → rating and flag.
  - Alt+C, Ctrl+I, Ctrl+Space, and so on.
- Alt is also the slider's fine-drag modifier (§I), so Alt is often down while the pointer is over the sidebar. Key presses a hovered slider captured are dropped, but once the pointer leaves the slider they are not.
- Fix: return `None` from the single-key block when `modifiers.command() || modifiers.control() || modifiers.alt()`, and keep the Z/Y cases above it. Shift must stay allowed ("?" and the upper-case letters).

### RISK 10.2 (minor): a held arrow key is never released if the window loses focus

- `window::Event::Unfocused` sends only `SetBeforePeek(false)`. The key-up that would send `NavigationKeyReleased` goes to another window.
- The reveal glide (§H) then keeps its feed-forward speed until the next key event. Not checked how long `Spring::track` runs without repeats (the hold rate is a time average, so it probably decays), so this may only be a short overshoot.
- Fix: one `Message::WindowUnfocused` that ends the peek and releases navigation. It is also the natural place to end a brush stroke or a drag that was in progress.

### RISK 10.3 (minor): key repeat repeats the action

- A held key repeats `SetRating`, `SetFlag`, `ResetEdits`, `ToggleCrop`, `ToggleInfoHud`, `ToggleWbPicker` and `MarkForRemovalConfirmed`. Each rating and flag repeat is a synchronous SQLite write; the toggles flicker.
- iced 0.13's `KeyPressed` has no repeat flag (not checked against the source; I believe it does not). Without one, track the keys that are down in `update` (set on press, cleared on release) and ignore a press of a key that is already down, for everything except the arrows.

### RISK 10.4: `without_ai_masks` trusts `mask_count`

- `core/types.rs:475`: the loop bound is clamped to `MAX_MASKS`, but inside it `let count = self.mask_count as usize` is not, and `copy_within(i + 1..count, i)` and `self.masks[count - 1]` index with it. A saved edit with `mask_count > MAX_MASKS` panics here, on paste and on anything else that calls it.
- `turn_quarter` (`:598`) uses `.take(mask_count)`, which is safe; `ai_masks` (`:493`) clamps. The shader side clamps separately. So the clamp is repeated per reader and missed in one.
- Fix: clamp once in `from_json` (`mask_count = mask_count.min(MAX_MASKS)`, and the same for `brush.stroke_count`, which `BrushData`'s deserialiser already guarantees), then readers can trust it. That also covers `handlers/masks.rs`, noted in 9.6 for the `luma` clamps: sanitise `luma_lo ≤ luma_hi` in the same place.

### RISK 10.5 (suspected, not checked): pasting masks onto an image with a different turn or shape

- `apply_categories` copies `masks`, `mask_count` and `brush` under the `masks` category, and `rotation`, `quarter_turns`, `crop` under `geometry`. They are independent checkboxes.
- Mask shapes and brush strokes live in the TURNED frame (§D). Pasting masks without geometry onto an image whose `quarter_turns` differs puts every linear, radial and brush mask a quarter turn away from where it was drawn. Pasting with geometry forces the source's turn onto the destination, which is right for a burst but wrong for a mix of portrait and landscape frames.
- Brush radii are fractions of the image height and dabs assume the layer's aspect, so a paste between a landscape and a portrait frame (EXIF orientation differs, turns equal) also changes the brush size and stretches nothing but still lands in a different place. That part is arguably "as designed": UV is the only frame two images share.
- Fix for the turn mismatch: in `handle_paste_settings`, when `masks` is pasted without `geometry`, turn the pasted masks by `(dst.quarter_turns − src.quarter_turns) mod 4` with the existing `turn_quarter` maths before applying. `EditClipboard` already keeps the source params, so the source turn is known.

### Small things

- `from_json` treats any `temperature > 100.0` as a legacy Kelvin value. The slider's own range must stay at or under 100 for that to hold; a comment on the slider range (or a `const`) would tie the two together.
- `handle_paste_settings` loads and saves every other selected image's edits on the UI thread, in one transaction (`save_edit_params_many`). Fine for tens of images; for a paste onto a few thousand it is a visible stall. Same shape as the rating and flag writes in section 2.
- `CopyCategories::{all, none}` and `is_empty` list the fields by hand, as do `describe_categories` and the copy dialog. One `const ALL: [(Category, &str); N]` would keep them together; adding a category today means touching five places.
- `commit_current_state` compares `without_tool_state()` copies of a 21.7 KB struct on every commit, and pushes a full copy onto the history (see PERF 1.11). No new finding, only a second place where the struct's size is paid.


## 11. Colour maths and the colour shader

Files read: `color.rs` (all non-test code), `gpu/shaders.rs` (the whole colour shader: uniforms, mask, Oklab, gamut, output, tone and `color_pass`), `core/perceptual.rs` `levels`, `core/tonemap.rs` `filmic`. Everything here is from reading; nothing was run.

### BUG 11.1: the fold-back in 8.1 is reachable on the default rendering path (settles 8.1's open question)

- 8.1 found that the filmic midtone lift `x·(1.08 − 0.08·x)` peaks at 6.75 and is zero at 13.5, and left open whether such values reach the shader's own block (`shaders.rs:1753`).
- They do. On the profile-less path (`has_dcp == 0`) nothing bounds `color` before Step 15: it is camera value × WB multiplier (often about 2 for red or blue) × highlight reconstruction (which only raises) × `2^exposure` × lens gain × the Texture/Clarity gain, then mask exposure and Whites. The 0.7 knee that protects the DCP path does not run here.
- A clipped channel is about 2.0 after WB. At Exposure +1.75 it is 6.7 (the peak); at +2.75 it is 13.5 and the lift returns zero or less. The lift is per channel, so before the highlight goes dark it changes hue: the largest channel folds first.
- After the lift, `pre_tone_max > 0.0` is false for a fully negative pixel, the shoulder is skipped, and `gamut_map` returns black for `L ≤ 0`.
- This is the path every camera takes until the user installs or downloads a profile, with the default tone mapper (Camera). Seeing it needs Exposure of about +2 or more on a frame with clipped areas, which is why it has probably gone unnoticed.
- Fix as in 8.1 (`min(x, 6.75)` before the lift, or a lift that fades out by 1.0: `x·(1 + t·max(1 − x, 0))`), in the shader and in `core::tonemap::filmic` together.

### RISK 11.2: `textureSample` in non-uniform control flow

- `shaders.rs:1167`: the lens-off branch reads the input with `textureSample` (implicit derivatives). It sits after several early `return`s that depend on the fragment's coordinates, and inside an `if`.
- WGSL requires implicit-derivative sampling to be in uniform control flow. naga in wgpu 0.19 does not enforce it (the validation test passes); Tint does, and newer naga versions have been adding the analysis. A wgpu upgrade could reject the shader, and the failure mode for a rejected shader in this app is a blank render and a log line.
- The texture has one mip level, so the derivative buys nothing. Use `textureSampleLevel(input_texture, texture_sampler, src_uv, 0.0)`, as the lens branch and every other read already do. The two branches then differ only in the UVs.

### RISK 11.3: sharpening is measured in input texels

- Step 9 (`:1507`) builds a 5×5 Gaussian high-pass from `textureLoad`s around `pixel_coords`. Its radius is two texels of whatever the input texture is.
- The develop preview is a subsampled mosaic; export and the loupe use the full one. So the same Amount sharpens features 2–4× larger at fit view than in the export, and looks stronger there. Texture and Clarity avoid this by defining their scales as fractions of the long edge (§G); sharpening does not.
- This is how capture sharpening behaves in most editors ("judge it at 1:1"), and the loupe gives a 1:1 view, so it may be accepted. Two cheap improvements:
  - skip the stage (or scale Amount by `1/stride`) when the input is subsampled, so the fit view does not promise crispness the export will not have;
  - or say so in the slider's hint.
- The masking term is `smoothstep(0.0, 0.08, laplacian)` in square-root luma, a fixed threshold. The image's noise model (σ at this level) is already known on the CPU; expressing the threshold in σ would stop high-ISO files from sharpening their grain at Masking 0–30. Suggestion only.
- Cost: 25 extra texel reads per fragment on every slider tick whenever Sharpening > 0 (the default?). Not measured; it is inside the 3 ms figure CLAUDE.md quotes.

### RISK 11.4: masks are evaluated in display UV, before Straighten and lens correction

- `mask_weight(m, mi, input.tex_coords, …)` (`:1644`) uses the output coordinate, while the image is read at `tex_coords` after rotation (Step 0a), orientation and the lens remap.
- So a linear, radial or brush mask stays where it was drawn on screen when Straighten, a lens-profile toggle or Constrain changes afterwards, and the picture moves underneath it. Quarter turns are handled (`turn_quarter`); these are not.
- BUG 1.3 is the AI-mask case of the same thing (there the bitmap is also computed in the sensor frame, so it is wrong immediately, not only after a later change).
- A fix that covers both: evaluate masks at the source coordinate (`tex_coords` after Step 0, turned back to the display orientation) and store shapes in that frame. That is a data migration for saved masks, so the cheaper step is to warn or re-map when Straighten changes on an image that has masks. Design decision; noted, not urgent.

### Small things in the shader

- `perceptual_levels` returns `vec3(0)` for `y ≤ 1e-9`, and `gamut_map` returns black for `L ≤ 0`. Both are right for today's slider ranges (Blacks 0–0.2 only crushes). If Blacks ever gets a negative (lifting) range, exact-zero pixels would stay black beside lifted neighbours, and the multiply would turn deep-shadow chroma noise into saturated speckle; a lift needs an additive term.
- The crop-mode dim (`color * 0.3`, `:1180`) runs before WB and highlight reconstruction, so the clip test sees dimmed values and blown areas outside the crop are not reconstructed while cropping. Cosmetic; moving the dim to the end of the pass would also keep it out of the tone curve.
- `mask_weight`'s colour range adds `MASK_CHROMA_EPS` and takes logs of camera-native ratios; `cam0` is the bilinear (and, with lens correction, per-channel shifted) sample, so on a lens-corrected image the range is tested against TCA-corrected colour while the eyedropper (`sample_raw_rgb`) reads the mosaic. Tiny difference, same family as BUG 1.2.
- The DCP branch converts ProPhoto to HSV with `v == pp.r` float comparisons and clamps `s` to 1. Fine for in-range data. With a non-Camera tone mapper the knee is skipped and `pp` can be far above 1; the table's value axis is clamped (`clamp(v, 0, 1)`), so every overbright samples the top value slice. That is the intended behaviour for a 1-slice LookTable; for a profile with several value divisions the top slice is applied to everything above 1.0. Probably what Adobe's pipeline does too (not checked).
- `gamut_map` runs for every pixel, in gamut or not: three cube roots, the cusp search and one Halley step, and for a wide space the 18-step march plus 20 bisections. Wide spaces are export only, so this is not a display cost; it does make a ProPhoto or P3 export noticeably slower than sRGB (not measured). A cheap early-out exists for sRGB only: a pixel whose channels are all within, say, [0.02, 0.9] of each other's range cannot be past `GAMUT_THRESHOLD`… that needs proving against the boundary, so only worth it if a profile shows the stage matters.
- `output_matrix`, `ok_lms_to_srgb` and the ProPhoto→sRGB matrix are rebuilt per fragment from literals. Compilers fold these; no action.
- The hue axis of the HueSatMap texture is handled correctly (a wrap column is baked, `shared.rs:621`, and the shader indexes `h·(width − 1)`). Listed because it is the classic mistake and I checked for it.

### `color.rs`

- `as_shot_kelvin_tint(raw)` reads `raw.dcp_profile`, the automatic pick at decode, and is called from twelve places in `gpu/render_functions.rs`, `gpu/loupe.rs` and `handlers/loading.rs`. CLAUDE.md §B says app paths must not render with that field. `loading.rs:181` overwrites the field with the resolved profile on the Develop path, which makes those calls correct there; whether export and the full-res upgrade do the same was not followed to the end (open question from section 3). The robust shape is to delete the one-argument function and make every caller pass the profile it resolved (`as_shot_kelvin_tint_for`).
- `xy_to_kelvin_tint`: `dt / (last_dt + dt)` is 0/0 when a chromaticity sits exactly on two consecutive isotherms' zero line; `f32::clamp` passes NaN through, so the Kelvin would be NaN and the WB multipliers with it. Practically unreachable; a `.max(1e-12)` on the denominator removes it.
- As-shot multipliers go to the shader as read from the file (`solve_wb` returns `None` at the default sliders). The shader takes `log(wb3)` for the highlight correction, and `1/wb` appears on the CPU with a `max(1e-6)` guard. A file with a zero multiplier would give −∞ there. The loader could clamp as-shot multipliers into the same 0.05–20 range `kelvin_tint_to_wb` uses.
- `calculate_cam_to_srgb` mixes cgmath (column-major) and flat row-major arrays by hand, with comments on each line; `mat3_mul`/`mat3_apply` exist for exactly this. Rewriting it on the flat helpers plus one `invert_flat` would remove the transposition bookkeeping CLAUDE.md warns about. No bug found in it.
- `compose_orientation` treats any orientation other than 1/3/6/8 as "no turn". Mirrored values are normalised earlier (`normalize_orientation`), so this is safe as long as every caller passes a normalised value.


## 12. Perceptual maths, output spaces, demosaic, edit serialisation

Files read: `core/perceptual.rs`, `core/output_space.rs`, `core/tone_regions.rs`, `core/guided.rs`, `core/histogram.rs`, `gpu/demosaic.rs` (all, shader included), `ui/viewport.wgsl`, `core/types.rs` 1–424. Read, not run.

### RISK 12.1: one NaN in an edit makes the whole edit unreadable (the likeliest trigger for RISK 5.2)

- `EditParams::to_json` is `serde_json::to_string`. serde_json writes a non-finite `f32` as `null`, without an error. `from_json` then fails on that `null` (an `f32` field does not accept it), and RISK 5.2 takes over: the image opens with defaults and the next flush saves them over the user's work.
- Typed slider values are filtered (`parse_slider_entry` keeps finite numbers only), so the sliders are safe. The fields computed from geometry or pixels are not guarded: crop rectangles and aspect presets (divisions by image dimensions), `turn_quarter` (brush radii rescaled by the height ratio, radial radii swapped), the level tool's angle, the WB eyedropper's temperature and tint (`xy_to_kelvin_tint`, see section 11), a mask's `color_target`, brush stroke points.
- I did not find a concrete NaN source; the point is that the cost of one is the whole edit, silently.
- Fix, two small parts:
  - in `to_json` (or `flush_edits`), refuse to save a non-finite edit: log it and keep the previous row. A `debug_assert!` there would also find the source during development;
  - correct RISK 5.2's third bullet: the nineteen original fields (`exposure` … `crop`) have no `#[serde(default)]`. That is harmless (every saved edit has them) but a struct-level `#[serde(default)]` on `EditParams` and `MaskParams` would cover them and every future field, where today a forgotten attribute on a new field makes every existing edit fail to parse.

### RISK 12.2 (minor): Contrast has a flat spot at L = 1 on unbounded data

- `contrast_curve` (`perceptual.rs:263`) is an S-curve on [0, 1] and the identity above 1. For `k > 1` its slope falls to zero as L reaches 1, then jumps back to 1 above it.
- On the DCP path Contrast runs after the tone curve, where almost nothing is above 1, so the curve ends where the data ends. On the profile-less path it runs before Step 15's tone block, with values well above 1: a bright gradient is compressed just below scene value 1.0 and resumes its full slope above it. With Contrast near its maximum that can show as a faint band in skies around the clipping point.
- Options: on the fallback path apply Contrast after the tone block, as the DCP path effectively does; or extend the curve above 1 with the pivot's log slope so it is monotone in slope. Not checked on an image.

### RISK 12.3 (minor): mask Hue pulls wide-gamut colours into sRGB

- `hue_shift` (`:255`) ends with `c.min(max_saturation(da, db) * L)`: the rotated colour's chroma is clipped to the sRGB cone at its new hue.
- CLAUDE.md §B3 says a colour already past the cone is "held, not boosted or pulled in", and that the only fit happens at Step 15b. Saturation and Vibrance follow that (`soft_limit(...).max(s)`); Hue does not, so a P3 or ProPhoto export loses out-of-sRGB colour inside any mask with a Hue shift, and a hard clip (not a roll-off) appears where the cone is narrow.
- Rotating without the clamp and leaving the fit to `gamut_map` is consistent with the rest. If the clamp is there for a reason (hues whose cone is much narrower, where chroma would explode after the rotation), limit it to the larger of the old and new cones: `c.min(max(s_old, s_new) * L)`.

### Small things

- `output_space.rs`: `srgb_to_space_matrix`, `encode`, `decode` and `space_to_srgb_matrix` are `#[cfg(test)]`, so at run time the WGSL literals are the only copy of the export matrices and the Rust side exists to test them. That is fine, but the module comment should say so, because the ICC profile is built from `to_xyz()` at run time while the pixels come from the literals; only the test ties the two.
- The ICC profiles are v4.3 with parametric curves. Readers without v4 support ignore the profile: harmless for sRGB, wrong colours for the other three. Adobe RGB could be written as a v2 profile at no cost in accuracy (its gamma 563/256 is exactly representable in a v2 `curv` tag, which is what Adobe's own profile does). P3 and ProPhoto would need sampled curves. Only worth doing if a user reports a viewer that shows washed-out wide-gamut exports. Not checked against any viewer.
- ICC description strings say "RAW Editor"; the app's name everywhere else is "RawEditor". These strings show up in other applications' profile pickers.
- `histogram::calculate` counts all pixels of the overview, including, in crop mode, the dimmed area outside the crop (if the overview is rendered in crop mode; not checked).
- `core::guided::filter` builds eight summed-area tables in `f64` one after another, each `(w+1)(h+1)`: about 22 MB each for a 2048-px mask, freed between calls. Fine; a separable running-sum box filter would avoid the tables if this ever runs on larger inputs.
- `gpu/demosaic.rs`: the transient targets are 16 bytes per pixel (`Rgba16Float` directions + two `R32Float` planes), 384 MB at 24 MP and 720 MB at 45 MP, on top of the 8 B/px output. They are dropped at the end of `run` but only reclaimed at the next `poll` (constraint 5). Related to PERF 7.3. The `opp` plane is only read at red and blue sites and `green` is copied into a 16-bit float output anyway, so both could be `R16Float` if a precision test (`eval_tests`) agrees; that would save 4 B/px.

### No findings

- `gpu/demosaic.rs`: the RCD passes match the reference formulation (direction statistics, ratio-corrected green, diagonal then cardinal colour differences); mirroring keeps CFA parity; black slots and the colour table agree for all four patterns; the caps and the WB divide are consistent with §A.
- `core/tone_regions.rs`: the gain slopes stay under 1 stop per stop as documented (0.78 for Highlights, 0.53 for Shadows), and the two ranges meet at −2 EV without overlapping.
- `core/perceptual.rs` gamut functions, `ui/viewport.wgsl`: nothing.


## 13. UI widgets, scroll glide, Develop / Library / Cull / Export views

Everything in this section was read, not run.

### BUG 13.1: Ctrl/Cmd-click reset on a slider never commits

`ui/widgets/slider.rs:210-214`:

```rust
if state.modifiers.command() {
    if let Some(d) = self.default { change(self, d); }
    state.drag = None;
}
```

The value changes, but no drag starts, so the release finds `state.drag == None` and
`on_release` (`Message::CommitEdit`, set in `slider_field`) is never published. The edit
is dirty but gets no undo snapshot and no flush until some other commit point fires.
A Ctrl-click reset followed directly by Ctrl+Z undoes the edit *before* it.

Fix: publish `on_release` in that branch (after the change message).

### BUG 13.2: double-click reset on a slider track makes two undo steps

The first press of the double-click reaches the slider: the value jumps to the click
position and the release commits. The second press is captured by `DoubleClick` and
publishes the reset with `commit: true`. Undo after a double-click reset therefore
returns to the jumped value, not to the value the user had. Double-clicking the label
is unaffected.

Options: reset through the label only; or have `DoubleClick` remember the value at the
first press and, on the second, replace the top undo entry instead of pushing one.

### BUG 13.3: a triple-click in the curve editor removes two points

`ui/curve_editor.rs`: the double-click test is keyed on the point *index*, and
`last_click = Some((now, i))` is written before the point is removed. A third click
within 350 ms hits whichever point slid into index `i` and removes that too.

Fix: clear `last_click` after a removal.

### RISK 13.4: the stored scroll offset is not clamped to the scrollable's maximum

- `handlers/scroll.rs`: `release` computes `(glide.to + direction * past).max(0.0)`.
- `filmstrip::scroll_to_reveal` and `GridGeometry::scroll_to_reveal` return
  `Some(offset.max(0.0))`.
- `set_offset` stores the value as given in `filmstrip_scroll_x` / `grid_scroll_y`.

iced clamps the real offset; the stored one, which `visible_items` and
`GridGeometry::visible_rows` use for virtualisation, can run past the end. With
`Reveal::Center` on the last image and a 2560 px strip the overshoot is up to
(view − `ITEM_WIDTH` 172)/2 ≈ 1194 px ≈ 6.9 items, more than `OVERSCAN_ITEMS = 4`, so
the leftmost visible slots would be spacers until the next real scroll event.

Not checked: whether iced 0.13's scrollable fires `on_scroll` after a `scroll_to` task
(that would correct the stored value one message later and reduce this to a one-frame
flash).

Fix: clamp where the offset is stored, to `count·ITEM_WIDTH − view_width` for the strip
and `total_rows·row_height + 2·GRID_PADDING − viewport_height` for the grid.

### PERF 13.5: a mouse move over the Develop image rebuilds the whole view

`views/develop.rs:814`: `mouse_area(stacked_image)…on_move(Message::MouseMoved)`.
Every pointer move over the image is one message, and iced rebuilds the view after
every message (CLAUDE.md §E) even when `handle_mouse_moved` does nothing useful: with
no drag in progress `handle_pan_interaction` (`navigation.rs:560`) only records
`last_cursor_position`. During a pan it returns `Task::done(Message::Pan(..))`, so one
pointer move is two messages and two rebuilds.

It is widget-scoped, not a global subscription, so it is within the letter of §E. The
cost is what one rebuild costs, which is PERF 13.6 and 13.7 below.

Ideas, cheapest first:
- attach `on_move` only while it can do something (`drag_mode != None`, crop mode,
  mask overlay on, loupe targeting);
- or move pan/handle dragging into the existing canvas overlay (`CropOverlay`), which
  already has the cursor and can publish only on a real drag.

### PERF 13.6: per-rebuild walks over the whole catalogue in the views

This is the list promised in section 1 (`displayed_images()` / `displayed_ids()`
allocate a `Vec` over the catalogue per call).

| Where | What | When |
|---|---|---|
| `views/develop.rs:65` | `editor.displayed_images()` for the filmstrip | every Develop rebuild |
| `views/develop.rs:767` | `editor.images.iter().find(..)` + `filename.clone()` (filename overlay) | every Develop rebuild |
| `views/cull.rs:19` | `displayed_images()` | every Cull rebuild |
| `views/cull.rs:56`, `:99` | `editor.images.iter().find(..)` twice | every Cull rebuild |
| `views/cull.rs:109` | `displayed_ids()` (cache-debug overlay only) | when that overlay is on |
| `views/library.rs:735` | `collect_folder_stats`: a `HashMap<String, usize>` with a `to_string_lossy().to_string()` per image | every Library rebuild |
| `views/library.rs:739-754` | `displayed_images()`, then three more passes (cached, deleted, selected counts) | every Library rebuild |
| `views/export.rs:266` | `editor.images.iter().find(..)` inside a loop over the pending ids | every Export rebuild; O(selected × catalogue) |

With 13.5, Develop does the first two per mouse move. At 50 000 images the Library
rebuild allocates 50 000 short strings for the folder list each time, including on
every `GridScrolled`.

Fix: keep the derived data in state and refresh it where its inputs change.
- A cached `displayed: Vec<usize>` (indices into `images`) plus an `id → index` map,
  rebuilt when `images`, the filter or the sort changes. Both `displayed_*` calls and
  every `find` above become lookups.
- `folder_stats` and the status-bar counts stored next to it, refreshed at the same
  points (plus selection changes for the selected count).
- `export.rs:266` with "select all" at 50 000 images is 2.5 × 10⁹ comparisons per
  rebuild without the map; it also builds one row per selected image with no
  virtualisation (the Library grid and filmstrip are virtualised, this list is not).

### PERF 13.7: the Develop sidebar builds every section every time

- `views::develop::section(editor, title, which, trailing, body: Element)` takes its
  body already built, and `Collapse::layout` always lays the child out at full height
  and multiplies by the open fraction. All nine sections are built and laid out on
  every message, folded or not (events and drawing are skipped when closed).
- Each of the ~60 sliders runs `slider_key` (`format!("{:?}", on_change(0.0))`),
  `rail_gradient(&on_change(0.0))` and a display `format!` per rebuild.
- `section_edited` runs nine times per rebuild.

None of it was measured. If the profiler graph shows view time mattering during a
drag: pass the body as a closure and build it only while the section is open or
animating; compare the typed-entry key only when an entry is active
(`editor.slider_entry.is_some()`) instead of formatting ~60 keys.

### PERF 13.8: the curve editor re-tessellates on every redraw

`ui/curve_editor.rs` has no `canvas::Cache`: the histogram fill (258 vertices) and up
to four 129-segment curves are rebuilt on each redraw while the Curves section is
open. Minor; a cache keyed on (curves, channel, histogram generation) removes it.

### BUG 13.9: deleting a mask during an AI-select session moves the session onto another mask

View: while AI select is open, only the "AI" add button is disabled
(`views/develop.rs` mask rows). `Message::DeleteMask(i)` (line 969) and the Linear /
Radial / Brush / Range add buttons stay enabled.

Handler: `handlers/masks.rs:136` `handle_delete_mask` shifts the slots down
(`copy_within`) and fixes `selected_mask`, but does not look at `editor.ai_select`
(the file never mentions it). The session keeps its `slot: usize`
(`handlers/ai_mask.rs:68`). After deleting a mask *below* the session's slot:

- the next decoder result writes its `ai_id` into `masks[s.slot]`, which is now the
  mask that used to be one above, or a default slot past `mask_count`;
- Cancel restores `prev_ai_id` into that wrong slot (`ai_mask.rs:313-314`), or, for a
  session that created its mask, calls `handle_delete_mask(editor, s.slot)` and
  deletes the wrong mask (`:299`, `:311`).

Deleting the session's own mask leaves the session open on a slot that now holds its
neighbour. This is the same family as BUG 1.1.

Fix: in `handle_delete_mask`, if a session is open, end it when `index == s.slot` and
decrement `s.slot` when `index < s.slot`; and disable the row's delete button for the
session's own mask.

### Small things

- `views/develop.rs:496`, `:499`, `:773` and several `Color::from_rgba(0,0,0,0.6)`
  scrims in `views/cull.rs:85`, `:93`: text over photos uses `Color::WHITE` in
  Develop where the `on_photo` token exists (Cull already uses it). The scrims are
  allowed literals (over a photo), but the same value is written out in four places.
- `views/library.rs:192`, `:210`: `compact_path` writes `".../"` and `"..."`; the
  design language says "…". It also truncates by `len()` (bytes) in one branch and
  by `chars()` in the other, so a non-ASCII folder name is cut shorter than an ASCII
  one. It runs once per folder per rebuild (see 13.6).
- `views/library.rs:296-298`: the placeholder label for `ThumbnailState::Ready` is
  "Generating…", which is unreachable (`has_loaded_thumb` is true exactly then). The
  three states cannot say "generating" at all: a row being processed shows "Queued…".
- `views/library.rs:152`: `is_image_selected` is a one-line wrapper around
  `editor.is_selected`.
- `views/cull.rs:118`: `n - 1` is safe only because `position` found the id, so
  `n ≥ 1`; fine, but `preview_preload_behind.min(n - 1)` counts the same neighbour
  twice (once behind, once ahead) when `n` is small. Debug overlay only.
- `views/cull.rs:25`: the button reads "Export {n}" with no noun; the Export tab says
  "Export 1 Image" / "Export n Images".
- `views/develop.rs:1265-1271`: two arms produce the same
  `format!("Default ({})", label(installed[0]))`; the `Some(file)`-but-not-found case
  silently shows the best match as if it were the saved default.
- `views/develop.rs:1285`: `.text_size(12)` is a literal where every other size is a
  `theme::size` token; `Length::Fixed(60.0)` for the "Profile" label against
  `LABEL_WIDTH` 80 for slider labels, so the pick list does not line up with the
  slider tracks.
- `ui/widgets/slider.rs:197`: `handle_width(&Theme::Dark, bounds)` hard-codes a theme
  (harmless: only the width is read).
- `EditorReadiness::Ready` with no `gpu_context` / `image_resources` draws a blank
  canvas with no message.
- Wheel zoom sends the sentinel `Point::new(-1.0, -1.0)` for "no cursor"; an
  `Option<Point>` would say it.
- Contrast is `-10.0..=10.0` with step 0.005 on a ~170 px track: 4000 steps on 170 px,
  so the step is never reachable by mouse (fine-drag included) and the useful range
  is a few pixels around the centre.

### Read with nothing to report

`ui/motion.rs` (non-test code), `ui/widgets/{button, modal, double_click, fade_image}.rs`,
`ui/filmstrip.rs` apart from 13.4, `views/cull.rs` apart from the above. Two notes:
`button` re-runs its style closure on every event for every mounted button (cheap),
and `is_pressed` can stick if a button is disabled between press and release, same as
upstream iced. The glide ends without a key-up: `step_glide` forces `release` once
`last_retarget` is older than `hold_gap`.


### BUG 13.10: a typo in Export's "Max long edge" exports at full size

`views/export.rs:124-125`:

```rust
text_input("2048", &s.max_width.to_string())
    .on_input(|v| Message::SetExportWidth(v.parse().unwrap_or(0)))
```

Anything that does not parse as a `u32` becomes 0: clearing the field, a stray letter
("2048x"), a minus sign. `handle_set_export_width` stores it as given, and
`handlers/export.rs:579` reads `max_width == 0` as "do not resize". So with
"Resize long edge" ticked, a typo silently exports full-resolution files, and the
digits typed so far are replaced by "0" in the field.

Fix: keep the text as typed in state (a `String`), parse it where it is used, and
disable the Export button with a one-line reason while it is not a positive number;
or at least ignore input that does not parse (keep the previous value) and treat an
empty field as "not set".

### IDEA 13.11: export settings are not remembered

`ExportSettings` lives only in `RawEditor` (`state.rs:342`, initialised from
`ExportSettings::default()` at `:958`); `core/settings.rs` does not mention it. Format,
quality, colour space, resize, destination folder and subfolder go back to the defaults
at every launch. For a user who always exports the same way this is six controls per
session. Adding the struct to `AppSettings` (`#[serde(default)]`) and saving at
`ExportConfirmed` would fix it; `base_path` needs a fallback when the folder no longer
exists.

### Small things (modals, Preferences, Export, start-up)

- `views/modals.rs:144-149`: the Export dialog counts `multi_selection` or else 1, and
  its Export button is always enabled; the Export tab counts
  `export_target_ids(editor)` and disables the button at zero. With no image selected
  the dialog offers "Export 1 Image". Use `export_target_ids` in both.
- `handlers/export.rs:92` `export_target_ids` calls `displayed_ids()` and is run by
  `view_queue` on every Export-tab rebuild: one more row for the table in 13.6.
- `views/modals.rs:439`: the title is "Delete Image" for any count.
- `views/modals.rs:218`: `folder_image_count` walks the catalogue per rebuild while
  the Remove Folder dialog is open (short-lived; only matters with 13.6).
- `views/modals.rs:519-521`, `views/export.rs:62-71`, `:102`, `:149-150`: `.size(14)`,
  `.size(16)`, `.text_size(12)`, `.text_size(13)` literals on checkboxes, radios and
  pick lists where the text elsewhere uses `theme::size` tokens (12 and 13 may well
  equal `SMALL` / `BODY`; they are just not named).
- `views/export.rs:136`: `base_path.join(&subfolder)`: a subfolder starting with `/`
  replaces the destination and `..` climbs out of it. The resulting path is shown in
  the hint below, so it is visible, but nothing says it is unusual.
- `views/preferences.rs:371`: `core::memory::available_bytes()` is called in `view`;
  on Linux that reads `/proc/meminfo` on every rebuild while the Memory page is open
  (every slider tick there). The figure also only refreshes when some other message
  arrives, so it looks live but is not. Read it when the page opens and on a slow
  timer. (Same pattern as `is_installed` in section 9.)
- `views/preferences.rs:373`: where the platform has no reading (macOS), the page says
  "check disabled" but the "Keep RAM Free" slider stays active and does nothing.
  This answers the earlier open question.
- `views/preferences.rs:832-854` (Folders page): the Data card does not list
  `models/` or `ai_masks/` (the latter is user work, CLAUDE.md §J) and the Cache card
  does not list `denoised/`. "Back this folder up to keep your edits" is still right.
- Preferences uses iced's own `slider` with `ProSlider::style`; Develop and the AI
  Denoise dialog use `ui::widgets::slider`. Two slider implementations, so fine-drag
  (Alt/Shift) and double-click reset exist in one place and not the other.
- `core/paths.rs:20-32`: if `dirs` returns nothing, both `data_dir()` and
  `cache_dir()` fall back to `~/raw-editor`, so data and cache share a folder. I found
  no code that deletes the cache folder as a whole (only `thumb/`, `instant/`,
  `working/` at `database/library.rs:659`), so this is harmless today; a future
  "clear cache" must not remove `cache_dir()` itself.
- `core/desktop.rs`: the entry follows the running binary by design, so every
  `cargo run-quick` repoints the user's launcher at `target/quick/raw-editor` until
  the installed build runs again. An environment switch (or skipping when the path
  contains `/target/`) would keep development runs from touching it. Quoting and
  escaping of `Exec` / `TryExec` / `Icon` look right against the desktop-entry spec.
- `raw/processor.rs:97-103`: `imageops::rotate90(&img)` on a `DynamicImage` returns
  RGBA8, so every rotated (portrait) preview is carried as four channels through the
  three resizes and handed to the JPEG encoder as RGBA8. `img.rotate90()` /
  `rotate180()` / `rotate270()` keep RGB8, save a third of the memory and the
  conversion. Not checked: how `image` 0.25.8's JPEG encoder treats RGBA8 input
  (early 0.25 releases rejected it; since portrait thumbnails work, this version must
  convert).
- `main.rs:25`: only the first argument is checked for `--version`; any other
  argument starts the GUI without comment. Fine for now.

### Read with nothing to report (second batch)

`views/layout.rs` (root, main view, update badge), `views/ai_denoise.rs`,
`ui/widgets/transition.rs` (`FadeThrough`, `Entrance`), `core/logging.rs`,
`core/paths.rs` and `core/desktop.rs` apart from the notes above.


## 14. Rest of the views: mask and lens rows, Library cards, Preferences, title bar

Read, not run, apart from the two places marked "checked".

### BUG 14.1: marking an image for removal throws away its star rating (checked by reading)

The mark is a sentinel in the `rating` column (`MARKED_FOR_REMOVAL_RATING` = 255;
`handlers/delete.rs:1-14` says this was chosen to avoid a schema change).
`delete.rs:86-91` writes 255 over the rating, and un-marking writes 0
(`delete.rs:59-64`). So Delete, then Delete again to undo it, turns a 5-star image
into an unrated one, for the whole selection, with no way back.

The sentinel also leaks into every reader of `rating`: the `>=` filter (documented at
`state.rs:1172`), the card badge, and any future sort or statistic.

Fix, smallest first:
- keep the old rating in the sentinel's place: store `200 + rating` (200-205) when
  marking and subtract on un-mark; `rating >= 200` is "marked". No schema change.
- or a real `marked INTEGER NOT NULL DEFAULT 0` column, which constraint 8 in
  CLAUDE.md allows as long as every query and `models.rs` change together.

### RISK 14.2 (suspected): the centred tabs overlap the menus and window buttons in a narrow window

`views/layout.rs:512-541`: the tab group (4 × `TAB_WIDTH` 112 = 448 px) is centred in
a full-width layer stacked *above* the row holding the menus, the update badge and
the window controls. `main.rs` allows a window 600 px wide. At 600 px the tabs span
76-524 px; the three 45 px window buttons start at about 465 px and the four menu
labels end at roughly 200 px, so the Export tab sits on top of the minimise button
and Library on top of the Help menu. The update badge ("Update 0.9.26") needs
another ~120 px. No overlap from about 850 px (about 1000 px with the badge).

Fix: raise `min_size` to what the bar needs, or drop the tab labels (icons only)
below a width threshold.

### Small things

- `views/library.rs:328-405`: Pick / Reject / Marked / star badges are drawn only when
  the thumbnail has loaded (`has_loaded_thumb`). A file that is missing, or still
  queued, shows no sign of being flagged or marked.
- `views/library.rs:756-760`: an empty catalogue shows "No images match this filter —
  try another folder or rating". On a first run there is no filter; it should say
  that nothing has been imported yet.
- `handlers/masks.rs:212`: the radial mask's Rotation is `clamp(-180, 180)`. CLAUDE.md
  §E asks for `wrap_degrees` on cyclic angles; typing 270 gives 180 instead of −90.
  (Dragging the on-canvas handle goes through another path, not re-read.)
- `views/develop.rs:1056-1118`: mask slider reset values are literals in the view
  (Low 0.0, High 1.0, Smooth 0.1, Tolerance 0.3, Feather 0.5) next to
  `MaskParams::default()` in `core/types.rs:415`. The global sliders take theirs from
  `EditParams::default()`; these should take theirs from `MaskParams::default()`.
- `views/develop.rs:1040`: brush Size has range 0.002-0.3 with step 0.001 on a
  linear track; small brushes occupy the first few pixels. A square-root or log
  mapping would spread them.
- `views/layout.rs:176`: the splash reads "Work in progress build" on every release.
- `views/layout.rs:362-417`: the menu bar has no Export, Undo / Redo, Copy / Paste
  Settings or Reset entries; those exist only as shortcuts and sidebar buttons. A
  menu is where people look for shortcuts they have forgotten.
- `views/preferences.rs:435-443` groups profiles by `p.camera.eq_ignore_ascii_case`
  and reads the default with `camera_profile_defaults.get(&camera)`, while Develop
  resolves both through the word / alias matching (`for_camera`, `default_for`). A
  default saved under one spelling can be in force in Develop while Preferences shows
  "Automatic" for that camera (same root as the `default_for` note in section 6).
- `views/preferences.rs:513`: a raw `"\u{f019}"` glyph where `ui::icons::DOWNLOAD`
  exists (`layout.rs:554` does the same with `"\u{f0aa}"`).

### RISK 14.3: the Sensor Correction controls never commit (debug-only panel)

The panel is shown only when `debug::SHOW_SENSOR_CORRECTION` is set, so this matters
only if it is ever turned on for users.

`views/develop.rs:357-376`: the four black-offset sliders are iced's plain `slider`
with no `on_release`, and the two "Shift Grid" checkboxes publish only
`BlackPhaseChanged`. Both handlers (`handlers/develop.rs:38-54`) change the params and
call `update_pipeline`, nothing else. So these edits are marked dirty but get no undo
snapshot and no flush of their own; they are saved only when some other commit point
fires (image switch, tab switch, another slider's release), and Ctrl+Z after one of
them undoes the edit before it. Same effect as BUG 13.1, and the same two messages
RISK 1.4 lists as missing from `is_slider_change`.

They also bypass `slider_field`: no typed entry, no double-click reset, no fine drag,
unlike every other Develop slider.

Fix: route them through `slider_field` (which adds the commit), and commit in the
checkbox handler.

### Small things (Develop geometry)

- `views/develop.rs:473`: the aspect buttons read `editor.image_resources` without the
  `image_id == selected_image_id` check that `geometry_tools` uses two functions up
  (`:424-427`). While a new image loads, the button computes its crop from the
  previous image's dimensions and applies it to the new one.
- `views/develop.rs:408-410`: the presets are 1:1, 16:9 and "2:3", and 2:3 is always
  width:height, so on a landscape frame it cuts a portrait crop. There is no 3:2,
  4:3 or 4:5, and no "original ratio". Flipping the ratio to match the frame (or a
  swap button) would cover the common cases.
- `views/develop.rs:150`: "Reset all edits" acts on one click with no confirmation.
  Fine if undo restores it (not re-checked here; see RISK 1.7 for what it leaves
  armed).

### Read with nothing to report

`views/develop.rs` tone-mapper controls, lens and defringe sections, profile and
database prompts, `slider_row*`; `views/library.rs` filter bar, sidebar, toolbar;
`views/preferences.rs` General and update controls; `ui/widgets/indicator.rs`;
`ui/widgets/no_operate.rs` (skimmed: `operate` is dropped for the widget and its
overlays, as CLAUDE.md §H describes).


## Not reviewed

State when the review stopped. I cannot read the usage meter from inside the session,
so the stopping point is a judgement, not a measured 80%.

Not read, or only skimmed for one question:

- `app/views/preferences.rs`: lens profiles and licences (lines 594-744, 900-1231).
- `app/views/modals.rs`: Help, About and Copy Settings (lines 48-142, 253-411).
- `ui/widgets/profiler_graph.rs`,
  `ui/{styles, theme, loupe, dialog, histogram, icons, text}.rs`.
- `app/message.rs` (the enum itself) and `app/state.rs` lines 1-1050 beyond the
  functions cited in sections 1, 2 and 10.
- `core/profiler.rs`.
- The WGSL sources were read for the colour path (sections 11-12); the denoise,
  local-contrast, brush and loupe shaders were read only as far as section 7 cites.
- Tests were not reviewed as tests (coverage, flakiness), apart from the notes on
  `raw/processor.rs` and the GPU `eval_tests` mentioned in passing.

After you asked me to stop checking, findings come from reading only, and each
section or finding says so ("read, not run", "not checked").

Open questions noted along the way and not resolved:

- Does iced 0.13's scrollable fire `on_scroll` after a `scroll_to` task? It decides
  whether RISK 13.4 is a lasting gap in the strip or a one-frame flash.
- Do export and the full-res upgrade overwrite `raw.dcp_profile` the way
  `loading.rs:181` does?
- Does entering crop mode reset zoom, and is an aspect preset held during a corner
  drag?
- Can as-shot WB multipliers be zero for any supported camera (several divisions
  assume not)?
- Are clipped bins excluded before the noise-model fit?
- Does `handle_set_min_free_ram` (`window.rs:346`) write settings per slider step?
- Does iced 0.13's `KeyPressed` carry a repeat flag (would simplify the hold-rate
  logic in `handlers/scroll.rs`)?
- Is the overview (histogram) pass rendered in crop mode?
- How does `image` 0.25.8's JPEG encoder treat RGBA8 input (see 13, small things)?
