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
    /** The sliders, one per line: section, label, min, max, default (tab-separated). */
    external fun controls(): String
    /** Slider [index] moved to [value], mid-drag. */
    external fun setEdit(handle: Long, index: Int, value: Float)
    /** A drag ended, or a reset: an undo step, saved. */
    external fun commitEdit(handle: Long)
    /** How slider [index] reads at [value] for the open photo. */
    external fun display(handle: Long, index: Int, value: Float): String
    /** A pinch or drag: scale about (x, y), then move by (dx, dy), in surface pixels. */
    external fun transformView(handle: Long, scale: Float, x: Float, y: Float, dx: Float, dy: Float)
    /** Double tap: back to fit, or zoom in about (x, y). */
    external fun toggleZoom(handle: Long, x: Float, y: Float)
    /** How many photos have opened; when it moves, [openedValues] has the new one's sliders. */
    external fun openedCount(handle: Long): Int
    external fun openedValues(handle: Long): FloatArray
    /** Moves whenever the histogram does: fetch [histogram] only then. */
    external fun histogramVersion(handle: Long): Int
    /** Red, green, blue: 256 counts each (empty before the first frame). */
    external fun histogram(handle: Long): IntArray
    external fun stats(handle: Long): String
    /** Save and stop. The handle is dead afterwards. */
    external fun destroy(handle: Long)
}
