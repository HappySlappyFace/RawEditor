package dev.raweditor

import android.content.pm.ActivityInfo
import android.os.Build
import android.view.Display
import android.view.SurfaceControl
import android.view.SurfaceView
import androidx.activity.ComponentActivity
import java.util.function.Consumer

/**
 * The photo shown in HDR. Three parts must agree:
 * - the window asks for HDR ([ActivityInfo.COLOR_MODE_HDR]);
 * - the SurfaceView's layer says how bright its pixels go and how bright
 *   it would like the display to let them (extended range brightness);
 * - the engine draws an HDR surface for the display's current HDR/SDR
 *   ratio, which Android raises and lowers with screen brightness and
 *   ambient light, and tells us about.
 *
 * Android 14+ on an HDR display ([supported]); elsewhere it stays off.
 */
class Hdr(private val activity: ComponentActivity, private val engine: Long) {
    var enabled = false
        private set

    private var view: SurfaceView? = null
    private var ratio = 1f
    private var listening = false
    private val listener = Consumer<Display> { d ->
        if (Build.VERSION.SDK_INT >= 34) onRatio(d.hdrSdrRatio)
    }

    private val display: Display?
        get() = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            activity.display
        } else {
            @Suppress("DEPRECATION")
            activity.windowManager.defaultDisplay
        }

    val supported: Boolean
        get() = Build.VERSION.SDK_INT >= 34 && display?.let { it.isHdr && it.isHdrSdrRatioAvailable } == true

    fun setEnabled(on: Boolean) {
        enabled = on && supported
        NativeEngine.setHdr(engine, enabled)
        activity.window.colorMode = if (enabled) ActivityInfo.COLOR_MODE_HDR else ActivityInfo.COLOR_MODE_DEFAULT
        val d = display
        if (Build.VERSION.SDK_INT >= 34 && d != null) {
            if (enabled && !listening) d.registerHdrSdrRatioChangedListener(activity.mainExecutor, listener)
            if (!enabled && listening) d.unregisterHdrSdrRatioChangedListener(listener)
            listening = enabled
            onRatio(if (enabled) d.hdrSdrRatio else 1f)
        }
    }

    /** The SurfaceView has a (new) surface: tell its layer again. */
    fun attach(view: SurfaceView) {
        this.view = view
        applyToLayer()
    }

    fun release() {
        if (Build.VERSION.SDK_INT >= 34 && listening) display?.unregisterHdrSdrRatioChangedListener(listener)
        listening = false
    }

    private fun onRatio(r: Float) {
        ratio = r.coerceAtLeast(1f)
        NativeEngine.setHeadroom(engine, ratio.coerceAtMost(MAX_HEADROOM))
        applyToLayer()
    }

    private fun applyToLayer() {
        if (Build.VERSION.SDK_INT < 34) return
        val sc = view?.surfaceControl?.takeIf { it.isValid } ?: return
        // What the frames are drawn for, and what we would like (our cap;
        // Android grants what the panel can). Asking is what makes it raise
        // the ratio at all.
        val current = if (enabled) ratio.coerceAtMost(MAX_HEADROOM) else 1f
        val desired = if (enabled) MAX_HEADROOM else 1f
        SurfaceControl.Transaction().setExtendedRangeBrightness(sc, current, desired).apply()
    }

    companion object {
        /** Highlights go at most two stops above SDR white. */
        const val MAX_HEADROOM = 4f
    }
}
