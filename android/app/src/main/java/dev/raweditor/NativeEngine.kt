package dev.raweditor

import android.view.Surface

/**
 * The Rust engine (crates/raw-editor-android). Every call but
 * [surfaceDestroyed] only posts a message to its render thread and returns.
 */
object NativeEngine {
    init {
        System.loadLibrary("raweditor")
    }

    /** Start the engine; its catalogue and caches go under [filesDir]. */
    external fun create(filesDir: String): Long
    external fun open(handle: Long, path: String)
    external fun surfaceChanged(handle: Long, surface: Surface, width: Int, height: Int)
    /** Returns once the engine has let go of the surface. */
    external fun surfaceDestroyed(handle: Long)
    external fun setExposure(handle: Long, ev: Float)
    /** A pinch or drag: scale about (x, y), then move by (dx, dy), in surface pixels. */
    external fun transformView(handle: Long, scale: Float, x: Float, y: Float, dx: Float, dy: Float)
    /** Double tap: back to fit, or zoom in about (x, y). */
    external fun toggleZoom(handle: Long, x: Float, y: Float)
    /** How many photos have opened; when it moves, [openedExposure] is the new one's. */
    external fun openedCount(handle: Long): Int
    external fun openedExposure(handle: Long): Float
    external fun stats(handle: Long): String
    /** Save and stop. The handle is dead afterwards. */
    external fun destroy(handle: Long)
}
