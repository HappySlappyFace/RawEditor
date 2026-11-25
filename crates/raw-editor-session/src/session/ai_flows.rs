//! The AI tools end to end: the real models through ONNX Runtime, the real
//! GPU and real files, driven through the session's own effects the way the
//! app runs them (`testkit::drive`). The unit tests in `ai_denoise`/`ai_mask`
//! deliver every result by hand; these let the work happen.
//!
//! Ignored like every GPU test, and they skip themselves when a model is not
//! installed (CI downloads them). Files go to a temp folder
//! (`core::paths::redirect_for_tests`), never the user's.

use std::sync::Arc;

use crate::ml::models::{self, ModelId};
use crate::raw::loader::RawDataResult;

use super::testkit::{deliver, drive, open_in_develop, session_with_images};
use super::{Session, SessionEvent};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("runtime")
}

/// The temp folder a test's files go to, redirected; removed when the test
/// ends, failing ones included.
struct TempRoot(std::path::PathBuf);

impl TempRoot {
    fn redirect() -> Self {
        let root = std::env::temp_dir().join(format!("raw-editor-ai-flows-{}", std::process::id()));
        crate::core::paths::redirect_for_tests(&root);
        TempRoot(root)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn installed(id: ModelId) -> bool {
    let ok = models::is_installed(id);
    if !ok {
        eprintln!("skipped: the {id:?} model is not installed (RAWEDITOR_MODELS_DIR)");
    }
    ok
}

/// The test RAW: `crate::ml::segment::test_support::disc_raw` (a bright disc
/// left of centre on a dark field), made neutral under its as-shot WB as a
/// real frame is (`disc_raw` stores one value at every site, which its WB
/// and identity matrix render as a colour far outside sRGB: black), with
/// shot noise (sd ∝ √signal) and read noise, deterministic.
fn noisy_disc(w: u32, h: u32) -> RawDataResult {
    let mut raw = crate::ml::segment::test_support::disc_raw(w, h);
    let colours = crate::gpu::demosaic::tile_colors(raw.cfa_pattern);
    for (i, v) in raw.data.iter_mut().enumerate() {
        let (x, y) = (i as u32 % w, i as u32 / w);
        let c = colours[((y % 2) * 2 + x % 2) as usize] as usize;
        *v = (*v as f32 / raw.wb_multipliers[c]) as u16;
    }
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut uniform = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f32 / (1u64 << 53) as f32
    };
    for v in raw.data.iter_mut() {
        let gauss = (-2.0 * uniform().max(1e-9).ln()).sqrt() * (std::f32::consts::TAU * uniform()).cos();
        let sd = (*v as f32 * 2.0 + 64.0).sqrt();
        *v = (*v as f32 + gauss * sd).clamp(0.0, raw.white_level as f32) as u16;
    }
    raw
}

/// Image 1 open in Develop on `raw`: GPU resources built and the first
/// render done, for real. The viewport shows it under 1:1, so no
/// full-resolution upgrade joins in.
fn developed(rt: &tokio::runtime::Runtime, raw: RawDataResult) -> Session {
    let mut s = session_with_images(1);
    s.viewport_size = (400.0, 300.0);
    s.scale_factor = 1.0;
    // Its decode would read a file that is not there; the RAW is handed in.
    let _ = open_in_develop(&mut s, 1);
    let load = deliver(&mut s, SessionEvent::RawDataLoaded(1, Ok(raw)));
    let ran = drive(&mut s, rt, load);
    assert!(ran.contains(&"develop resources") && ran.contains(&"render"), "{ran:?}");
    assert!(s.rendered_preview_bytes.is_some(), "no render landed: {ran:?}");
    s
}

/// Mean absolute luma step between horizontal neighbours: noise, on these
/// flat test frames.
fn roughness(rgba: &[u8], w: usize) -> f64 {
    let luma = |i: usize| 0.2126 * rgba[i] as f64 + 0.7152 * rgba[i + 1] as f64 + 0.0722 * rgba[i + 2] as f64;
    let px = rgba.len() / 4;
    let (mut sum, mut n) = (0.0, 0usize);
    for p in 0..px - 1 {
        if (p + 1) % w != 0 {
            sum += (luma(p * 4) - luma(p * 4 + 4)).abs();
            n += 1;
        }
    }
    sum / n as f64
}

fn rendered(s: &Session) -> (Arc<[u8]>, usize, usize) {
    let (w, h) = s.rendered_preview_dims;
    (s.rendered_preview_bytes.clone().expect("a render"), w as usize, h as usize)
}

/// The dialog previews PMRID live on a crop, Apply computes the whole image
/// and caches it, the edit is set, and the image is rebuilt from the
/// denoised mosaic. A missing cache (cleared, or another machine) is
/// recomputed when the image is next built.
#[test]
#[ignore]
fn ai_denoise_previews_applies_and_regenerates() {
    if !installed(ModelId::Denoise) {
        return;
    }
    let temp = TempRoot::redirect();
    let root = temp.0.clone();
    let rt = runtime();
    let mut s = developed(&rt, noisy_disc(640, 480));
    let (before, w, _) = rendered(&s);

    // Open: the engine, a denoised crop, its preview.
    let open = s.open_ai_denoise().expect("model installed");
    let ran = drive(&mut s, &rt, open);
    assert_eq!(s.ai_denoise.error, None);
    for label in ["AI denoise engine", "AI denoise crop", "AI denoise preview"] {
        assert!(ran.contains(&label), "{label} missing: {ran:?}");
    }
    let after_frame = s.ai_denoise.frame.clone().expect("a preview");
    let before_press = s.set_ai_denoise_before(true);
    drive(&mut s, &rt, before_press);
    let before_frame = s.ai_denoise.frame.clone().expect("a preview");
    let (r_on, r_off) = (
        roughness(&after_frame.rgba, after_frame.size as usize),
        roughness(&before_frame.rgba, before_frame.size as usize),
    );
    eprintln!("preview roughness: before {r_off:.2}, AI denoise {r_on:.2}");
    assert!(r_on < r_off * 0.8, "the preview did not show the denoise");
    let release = s.set_ai_denoise_before(false);
    drive(&mut s, &rt, release);

    // Apply: the whole image, cached, set as the edit, rebuilt with it.
    let amount = s.ai_denoise.amount;
    let apply = s.apply_ai_denoise();
    let ran = drive(&mut s, &rt, apply);
    assert!(ran.contains(&"AI denoise") && ran.contains(&"develop resources"), "{ran:?}");
    assert_eq!(s.ai_denoise.image_id, None, "the dialog stays open");
    assert_eq!(s.current_edit_params.ai_denoise, amount);
    let res = s.image_resources.clone().expect("resources");
    assert_eq!(res.ai_denoise, (amount, amount), "rebuilt without the denoised mosaic");
    let raw = s.raw_for(1).expect("RAW cached");
    assert!(crate::ml::denoise::has_cache(1, &raw.data, raw.width, raw.height), "no cache written");
    assert!(root.join("cache").join("denoised").is_dir(), "the cache is not in the redirected folder");
    let (after, w2, _) = rendered(&s);
    let (r_before, r_after) = (roughness(&before, w), roughness(&after, w2));
    eprintln!("render roughness: before {r_before:.2}, after {r_after:.2}");
    assert!(r_after < r_before * 0.8, "the render did not take the denoised mosaic");

    // The cache goes; the next build of the image recomputes it.
    std::fs::remove_dir_all(root.join("cache").join("denoised")).expect("clear the cache");
    let reload = s.reload_selected_image();
    let ran = drive(&mut s, &rt, reload);
    assert!(ran.contains(&"AI denoise regenerate"), "{ran:?}");
    assert!(s.status.is_empty(), "{}", s.status);
    let res = s.image_resources.clone().expect("resources");
    assert_eq!(res.ai_denoise, (amount, amount), "not rebuilt after regenerating");

}

/// A click selects the disc through SAM: the mask's bitmap is written, its
/// edits apply to the disc and not the field, and Refine reopens it with its
/// prompts while Cancel puts it back.
#[test]
#[ignore]
fn ai_select_masks_what_was_clicked() {
    if !installed(ModelId::Segment) {
        return;
    }
    let temp = TempRoot::redirect();
    let root = temp.0.clone();
    let rt = runtime();
    let mut s = developed(&rt, noisy_disc(640, 480));

    let start = s.ai_select_start().expect("model installed");
    let ran = drive(&mut s, &rt, start);
    assert!(ran.contains(&"AI select engine"), "{ran:?}");
    let sel = s.ai_select.as_ref().expect("a selection session");
    assert!(sel.engine.is_some(), "no engine: {:?}", sel.error);
    let slot = sel.slot;

    // The disc's centre (disc_raw: 0.4 across, half-way down).
    let click = s.ai_prompt(0.4, 0.5, true);
    let ran = drive(&mut s, &rt, click);
    assert!(ran.contains(&"AI select decode"), "{ran:?}");
    let ai_id = s.current_edit_params.masks[slot].ai_id;
    assert_ne!(ai_id, 0, "no mask was set");
    let png = crate::ml::masks::png_path(1, ai_id);
    assert!(png.starts_with(&root), "{} is not in the redirected folder", png.display());
    let bitmap = crate::ml::masks::load(1, ai_id).expect("the bitmap was written");
    let at = |u: f32, v: f32| {
        let (x, y) = ((u * bitmap.width as f32) as u32, (v * bitmap.height as f32) as u32);
        bitmap.data[(y * bitmap.width + x) as usize]
    };
    assert!(at(0.4, 0.5) > 200, "the disc is not selected ({})", at(0.4, 0.5));
    assert!(at(0.05, 0.05) < 50 && at(0.9, 0.9) < 50, "the field is selected");

    let done = s.ai_select_done();
    drive(&mut s, &rt, done);
    assert!(s.ai_select.is_none());

    // −2 EV through the mask: the disc darkens, the field doesn't.
    let (plain, w, h) = rendered(&s);
    s.current_edit_params.masks[slot].exposure = -2.0;
    let edit = s.update_pipeline();
    drive(&mut s, &rt, edit);
    let (masked, w2, h2) = rendered(&s);
    assert_eq!((w, h), (w2, h2));
    let luma = |img: &[u8], u: f32, v: f32| {
        let i = (((v * h as f32) as usize) * w + (u * w as f32) as usize) * 4;
        (img[i] as f64 + img[i + 1] as f64 + img[i + 2] as f64) / 3.0
    };
    let (disc0, disc1) = (luma(&plain, 0.4, 0.5), luma(&masked, 0.4, 0.5));
    let (field0, field1) = (luma(&plain, 0.9, 0.1), luma(&masked, 0.9, 0.1));
    eprintln!("disc {disc0:.0} → {disc1:.0}, field {field0:.0} → {field1:.0}");
    assert!(disc1 < disc0 - 40.0, "the mask's edit missed the disc");
    assert!((field1 - field0).abs() < 4.0, "the mask's edit reached the field");

    // Refine reopens with the saved prompt; Cancel restores the mask.
    let refine = s.ai_select_refine(slot).expect("model installed");
    drive(&mut s, &rt, refine);
    assert_eq!(s.ai_select.as_ref().expect("reopened").prompts.points.len(), 1, "prompts not reloaded");
    let click = s.ai_prompt(0.9, 0.1, true);
    drive(&mut s, &rt, click);
    assert_ne!(s.current_edit_params.masks[slot].ai_id, ai_id, "a new click did not make a new mask");
    let cancel = s.ai_select_cancel();
    drive(&mut s, &rt, cancel);
    assert_eq!(s.current_edit_params.masks[slot].ai_id, ai_id, "Cancel did not restore the mask");

}
