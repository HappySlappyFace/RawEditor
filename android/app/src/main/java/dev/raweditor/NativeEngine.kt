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
    /** Draw in HDR when the surface can, or not. */
    external fun setHdr(handle: Long, on: Boolean)
    /** The display's current HDR/SDR ratio: HDR frames are drawn for it. */
    external fun setHeadroom(handle: Long, ratio: Float)
    /** True while the surface is drawn in HDR. */
    external fun hdrActive(handle: Long): Boolean
    /** Save the open photo, as edited, as a full-size JPEG in the app's folder. */
    external fun export(handle: Long)
    /** The next finished export, once: "ok\t<path>" or "error\t<why>"; null while there is none. */
    external fun takeExport(handle: Long): String?
    /** How many photos have opened. */
    external fun openedCount(handle: Long): Int
    /** Moves whenever the edit changed under the sliders (a photo opened, undo, redo, reset). */
    external fun valuesVersion(handle: Long): Int
    /** Every slider's value for the open photo's edit, in index order. */
    external fun openedValues(handle: Long): FloatArray
    /** Step back through the open photo's history. */
    external fun undo(handle: Long)
    /** Step forward again. */
    external fun redo(handle: Long)
    /** Every edit back to its default, as one undo step. */
    external fun resetAll(handle: Long)
    /** The crop tool on or off: the whole frame shown, the part cut away dimmed. */
    external fun setCropping(handle: Long, on: Boolean)
    /** Crop handle [which] (0 top left, 1 top right, 2 bottom left, 3 bottom right, 4 the whole crop) dragged, surface pixels. */
    external fun cropDrag(handle: Long, which: Int, x0: Float, y0: Float, x1: Float, y1: Float)
    /** The crop drag ended: an undo step. */
    external fun cropEnd(handle: Long)
    /** A centred crop of [ratio] (width / height); 0 for the whole frame. */
    external fun cropAspect(handle: Long, ratio: Float)
    /** A quarter turn of the whole frame. */
    external fun turn(handle: Long, clockwise: Boolean)
    /** While cropping: the image's rectangle on the surface (x, y, w, h, pixels), then the crop (left, top, w, h, fractions); empty otherwise. */
    external fun cropFrame(handle: Long): FloatArray
    /** Bit 0: Undo has a step to take; bit 1: Redo does. */
    external fun history(handle: Long): Int
    /** Moves whenever the histogram does: fetch [histogram] only then. */
    external fun histogramVersion(handle: Long): Int
    /** Red, green, blue: 256 counts each, then the count to scale to (empty before the first frame). */
    external fun histogram(handle: Long): IntArray
    external fun stats(handle: Long): String
    /** Save and stop. The handle is dead afterwards. */
    external fun destroy(handle: Long)
}
