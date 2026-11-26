//! The JNI functions Kotlin calls (`dev.raweditor.NativeEngine`). They only
//! post messages to the render thread, so no UI-thread call waits on a
//! render, except `surfaceDestroyed`, which must (Android reclaims the
//! surface once it returns).
//!
//! Every function takes the `long` handle `create` returned.

use jni::objects::{JObject, JString};
use jni::sys::{jfloat, jfloatArray, jint, jintArray, jlong, jstring};
use jni::JNIEnv;

use crate::engine::{Command, Engine};
use crate::{controls, logcat};

fn engine<'a>(handle: jlong) -> Option<&'a Engine> {
    // SAFETY: `handle` is the pointer `create` leaked, valid until `destroy`.
    unsafe { (handle as *const Engine).as_ref() }
}

fn string(env: &mut JNIEnv, s: &JString) -> Option<String> {
    env.get_string(s).ok().map(Into::into)
}

/// Start the engine. `filesDir` is the app's private folder: the catalogue,
/// settings and caches go there.
#[no_mangle]
pub extern "system" fn Java_dev_raweditor_NativeEngine_create(
    mut env: JNIEnv,
    _: JObject,
    files_dir: JString,
) -> jlong {
    logcat::init();
    let Some(files) = string(&mut env, &files_dir) else { return 0 };
    Box::into_raw(Box::new(Engine::start(files.into()))) as jlong
}

/// Open a RAW file (imported into the catalogue first if new).
#[no_mangle]
pub extern "system" fn Java_dev_raweditor_NativeEngine_open(mut env: JNIEnv, _: JObject, handle: jlong, path: JString) {
    if let (Some(e), Some(path)) = (engine(handle), string(&mut env, &path)) {
        e.send(Command::Open(path.into()));
    }
}

/// The SurfaceView's surface exists, or changed size.
#[no_mangle]
pub extern "system" fn Java_dev_raweditor_NativeEngine_surfaceChanged(
    env: JNIEnv,
    _: JObject,
    handle: jlong,
    surface: JObject,
    width: jint,
    height: jint,
) {
    let Some(e) = engine(handle) else { return };
    // SAFETY: a live android.view.Surface from the callback that handed it over.
    let window = unsafe { ndk::native_window::NativeWindow::from_surface(env.get_raw().cast(), surface.as_raw().cast()) };
    match window {
        Some(window) => e.send(Command::Surface(window, width.max(1) as u32, height.max(1) as u32)),
        None => tracing::error!("the Surface has no native window"),
    }
}

/// The surface is about to go. Returns once the engine has let go of it.
#[no_mangle]
pub extern "system" fn Java_dev_raweditor_NativeEngine_surfaceDestroyed(_: JNIEnv, _: JObject, handle: jlong) {
    if let Some(e) = engine(handle) {
        e.release_surface();
    }
}

/// The controls, one per line: `section, label, min, max, default`,
/// tab-separated. Kotlin refers to them by line number.
#[no_mangle]
pub extern "system" fn Java_dev_raweditor_NativeEngine_controls(env: JNIEnv, _: JObject) -> jstring {
    env.new_string(controls::spec()).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

/// Control `index` moved to `value` (mid-drag: no undo step yet).
#[no_mangle]
pub extern "system" fn Java_dev_raweditor_NativeEngine_setEdit(_: JNIEnv, _: JObject, handle: jlong, index: jint, value: jfloat) {
    if let Some(e) = engine(handle) {
        e.send(Command::Edit(index.max(0) as usize, value));
    }
}

/// A drag ended (or a reset): an undo step, saved.
#[no_mangle]
pub extern "system" fn Java_dev_raweditor_NativeEngine_commitEdit(_: JNIEnv, _: JObject, handle: jlong) {
    if let Some(e) = engine(handle) {
        e.send(Command::Commit);
    }
}

/// How control `index` reads at `value` for the open photo (white balance
/// in kelvin from its as-shot value).
#[no_mangle]
pub extern "system" fn Java_dev_raweditor_NativeEngine_display(
    env: JNIEnv,
    _: JObject,
    handle: jlong,
    index: jint,
    value: jfloat,
) -> jstring {
    let text = engine(handle).map(|e| e.show(index.max(0) as usize, value)).unwrap_or_default();
    env.new_string(text).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}


/// A pinch or drag on the photo: scale the zoom about (x, y), then move by
/// (dx, dy), all in the surface's pixels.
#[no_mangle]
pub extern "system" fn Java_dev_raweditor_NativeEngine_transformView(
    _: JNIEnv,
    _: JObject,
    handle: jlong,
    scale: jfloat,
    x: jfloat,
    y: jfloat,
    dx: jfloat,
    dy: jfloat,
) {
    if let Some(e) = engine(handle) {
        e.send(Command::View { scale, at: [x, y], pan: [dx, dy] });
    }
}

/// A double tap at (x, y): back to fit, or zoom in about it.
#[no_mangle]
pub extern "system" fn Java_dev_raweditor_NativeEngine_toggleZoom(_: JNIEnv, _: JObject, handle: jlong, x: jfloat, y: jfloat) {
    if let Some(e) = engine(handle) {
        e.send(Command::ToggleZoom([x, y]));
    }
}

/// How many photos have opened so far; when it moves, `openedValues` has
/// the new photo's saved edit.
#[no_mangle]
pub extern "system" fn Java_dev_raweditor_NativeEngine_openedCount(_: JNIEnv, _: JObject, handle: jlong) -> jint {
    engine(handle).map_or(0, |e| e.opened().0 as jint)
}

/// The last opened photo's values for every control, in index order.
#[no_mangle]
pub extern "system" fn Java_dev_raweditor_NativeEngine_openedValues(env: JNIEnv, _: JObject, handle: jlong) -> jfloatArray {
    let values = engine(handle).map(|e| e.opened().1).unwrap_or_default();
    let Ok(array) = env.new_float_array(values.len() as jint) else { return std::ptr::null_mut() };
    let _ = env.set_float_array_region(&array, 0, &values);
    array.into_raw()
}

/// Moves whenever the histogram does: fetch [`histogram`] only then.
#[no_mangle]
pub extern "system" fn Java_dev_raweditor_NativeEngine_histogramVersion(_: JNIEnv, _: JObject, handle: jlong) -> jint {
    engine(handle).map_or(0, |e| e.histogram_version() as jint)
}

/// The histogram: red, green, blue, 256 counts each, then the count to
/// scale them to (clipped end bins excluded; they are drawn clamped). Empty
/// before the first frame. A frame behind the screen, which nobody can see.
#[no_mangle]
pub extern "system" fn Java_dev_raweditor_NativeEngine_histogram(env: JNIEnv, _: JObject, handle: jlong) -> jintArray {
    let counts: Vec<jint> = engine(handle).map(|e| e.histogram()).unwrap_or_default().into_iter().map(|c| c.min(i32::MAX as u32) as jint).collect();
    let Ok(array) = env.new_int_array(counts.len() as jint) else { return std::ptr::null_mut() };
    let _ = env.set_int_array_region(&array, 0, &counts);
    array.into_raw()
}

/// One line for the on-screen readout: status and frame timings.
#[no_mangle]
pub extern "system" fn Java_dev_raweditor_NativeEngine_stats(env: JNIEnv, _: JObject, handle: jlong) -> jstring {
    let text = engine(handle).map(Engine::summary).unwrap_or_default();
    env.new_string(text).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

/// Stop the engine, saving the edit, and free it. The handle is dead after.
#[no_mangle]
pub extern "system" fn Java_dev_raweditor_NativeEngine_destroy(_: JNIEnv, _: JObject, handle: jlong) {
    if handle != 0 {
        // SAFETY: the pointer `create` leaked; Kotlin calls this once.
        drop(unsafe { Box::from_raw(handle as *mut Engine) });
    }
}
