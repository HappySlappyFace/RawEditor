package dev.raweditor

import android.content.ContentValues
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.graphics.Path
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

/** RAW extensions the engine opens (Bayer formats, as `RAW_EXTENSIONS`). */
private val RAW = setOf("nef", "cr2", "cr3", "arw", "raf", "rw2", "orf", "dng", "pef", "srw", "nrw")

private val Orange = Color(0xFFF97316)
private val Muted = Color(0xFFA1A1AA)

class MainActivity : ComponentActivity() {
    private var engine = 0L
    private lateinit var hdr: Hdr

    /** Where photos live: the app's own folder, which adb can push into too. */
    private val photos: File get() = getExternalFilesDir(null) ?: filesDir

    private val prefs get() = getSharedPreferences("raweditor", MODE_PRIVATE)

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        // Exports the last run never got to move into the gallery.
        File(filesDir, "exports").listFiles()?.forEach { it.delete() }
        engine = NativeEngine.create(filesDir.absolutePath)
        hdr = Hdr(this, engine)
        hdr.setEnabled(prefs.getBoolean("hdr", true))
        // Reopen the last photo, else the first RAW in the folder.
        val last = prefs.getString("last", null)?.let(::File)?.takeIf { it.exists() }
        val raw = last ?: photos.listFiles()
            ?.filter { it.extension.lowercase() in RAW }
            ?.minByOrNull { it.name }
        if (raw != null) open(raw)
        setContent { Editor(engine, hasPhoto = raw != null, ::importAndOpen, ::saveToGallery, hdr, ::setHdr) }
    }

    private fun setHdr(on: Boolean) {
        prefs.edit().putBoolean("hdr", on).apply()
        hdr.setEnabled(on)
    }

    private fun open(file: File) {
        prefs.edit().putString("last", file.absolutePath).apply()
        NativeEngine.open(engine, file.absolutePath)
    }

    /**
     * The engine reads RAWs by path, and a picked document is only a URI,
     * so the photo is copied into the app's folder first (once: a file of
     * the same name and size is reused). Returns an error to show, or null.
     */
    private suspend fun importAndOpen(uri: Uri): String? = withContext(Dispatchers.IO) {
        val name = contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            ?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
            ?: uri.lastPathSegment?.substringAfterLast('/')
            ?: return@withContext "That file has no name"
        if (File(name).extension.lowercase() !in RAW) {
            return@withContext "$name is not a RAW file this app opens"
        }
        val target = File(photos, File(name).name)
        val size = contentResolver.openFileDescriptor(uri, "r")?.use { it.statSize } ?: -1L
        if (!(target.exists() && target.length() == size)) {
            val part = File(photos, "${target.name}.part")
            try {
                contentResolver.openInputStream(uri)?.use { input ->
                    part.outputStream().use { input.copyTo(it, 1 shl 20) }
                } ?: return@withContext "Could not read $name"
                if (!part.renameTo(target)) return@withContext "Could not save $name"
            } catch (e: Exception) {
                part.delete()
                return@withContext "Could not copy $name: ${e.message}"
            }
        }
        withContext(Dispatchers.Main) { open(target) }
        null
    }

    /**
     * Move an exported JPEG into the gallery (Pictures/RawEditor; a name
     * already there gets a number) and delete the engine's copy. Returns
     * what to tell the user.
     */
    private suspend fun saveToGallery(path: String): Result<String> = withContext(Dispatchers.IO) {
        val file = File(path)
        try {
            runCatching {
                // Earlier versions need the storage permission for this.
                check(Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) { "Saving to the gallery needs Android 10 or newer" }
                val values = ContentValues().apply {
                    put(MediaStore.Images.Media.DISPLAY_NAME, file.name)
                    put(MediaStore.Images.Media.MIME_TYPE, "image/jpeg")
                    put(MediaStore.Images.Media.RELATIVE_PATH, "${Environment.DIRECTORY_PICTURES}/RawEditor")
                    // Hidden from other apps until it is complete.
                    put(MediaStore.Images.Media.IS_PENDING, 1)
                }
                val collection = MediaStore.Images.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
                val uri = checkNotNull(contentResolver.insert(collection, values)) { "The gallery refused the photo" }
                try {
                    val out = checkNotNull(contentResolver.openOutputStream(uri)) { "Could not write to the gallery" }
                    out.use { o -> file.inputStream().use { it.copyTo(o, 1 shl 20) } }
                    values.clear()
                    values.put(MediaStore.Images.Media.IS_PENDING, 0)
                    contentResolver.update(uri, values, null, null)
                } catch (e: Exception) {
                    contentResolver.delete(uri, null, null)
                    throw e
                }
                "Saved to Pictures/RawEditor"
            }
        } finally {
            file.delete()
        }
    }

    override fun onDestroy() {
        hdr.release()
        NativeEngine.destroy(engine)
        engine = 0L
        super.onDestroy()
    }
}

/** One slider, as the engine describes it ([NativeEngine.controls]). */
private data class Control(val section: String, val label: String, val min: Float, val max: Float, val default: Float)

private val Controls: List<Control> = NativeEngine.controls().lines().map { line ->
    val f = line.split('\t')
    Control(f[0], f[1], f[2].toFloat(), f[3].toFloat(), f[4].toFloat())
}
private val Sections: List<String> = Controls.map { it.section }.distinct()
/** Every section's panel is as tall as the longest, so the photo above never resizes. */
private val Rows: Int = Sections.maxOf { s -> Controls.count { it.section == s } }

private val Panel = Color(0xFF111113)

@Composable
private fun Editor(
    engine: Long,
    hasPhoto: Boolean,
    importAndOpen: suspend (Uri) -> String?,
    saveToGallery: suspend (String) -> Result<String>,
    hdr: Hdr,
    setHdr: (Boolean) -> Unit,
) {
    val values = remember { mutableStateListOf(*Controls.map { it.default }.toTypedArray()) }
    var section by remember { mutableStateOf(Sections.first()) }
    var stats by remember { mutableStateOf("") }
    var opened by remember { mutableIntStateOf(0) }
    var photo by remember { mutableStateOf(hasPhoto) }
    var error by remember { mutableStateOf<String?>(null) }
    var notice by remember { mutableStateOf<String?>(null) }
    var exporting by remember { mutableStateOf(false) }
    var hdrOn by remember { mutableStateOf(hdr.enabled) }
    var history by remember { mutableIntStateOf(0) }
    var cropFrame by remember { mutableStateOf(FloatArray(0)) }
    val cropping = section == CropSection
    val scope = rememberCoroutineScope()

    // Looked at once per display frame, so Undo moves the sliders with the
    // photo: the edit changed under them (a photo opened, undo, redo,
    // reset), and whether Undo and Redo have a step to take.
    LaunchedEffect(engine) {
        var seen = -1
        while (true) {
            withFrameNanos { }
            val version = NativeEngine.valuesVersion(engine)
            if (version != seen) {
                seen = version
                NativeEngine.openedValues(engine).forEachIndexed { i, v -> if (i < values.size) values[i] = v }
            }
            history = NativeEngine.history(engine)
            val frame = NativeEngine.cropFrame(engine)
            if (!frame.contentEquals(cropFrame)) cropFrame = frame
        }
    }

    // The Crop tab is the crop tool.
    LaunchedEffect(cropping) { NativeEngine.setCropping(engine, cropping) }

    LaunchedEffect(engine) {
        while (true) {
            stats = NativeEngine.stats(engine)
            opened = NativeEngine.openedCount(engine)
            // An export finished: into the gallery with it.
            NativeEngine.takeExport(engine)?.let { result ->
                val (kind, text) = result.split('\t', limit = 2).let { it[0] to it.getOrElse(1) { "" } }
                if (kind == "ok") {
                    saveToGallery(text).fold({ notice = it }, { error = it.message ?: "Could not save to the gallery" })
                } else {
                    error = "Export failed: $text"
                }
                exporting = false
            }
            delay(250)
        }
    }

    // A notice goes after a few seconds; the readout comes back.
    LaunchedEffect(notice) {
        if (notice != null) {
            delay(4000)
            notice = null
        }
    }

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) {
            photo = true
            error = null
            scope.launch { error = importAndOpen(uri) }
        }
    }

    Column(Modifier.fillMaxSize().background(Color(0xFF0A0A0A))) {
        // The photo: the engine draws the surface; gestures go to it.
        Box(Modifier.weight(1f).fillMaxWidth()) {
            AndroidView(
                modifier = Modifier.fillMaxSize(),
                factory = { context ->
                    SurfaceView(context).apply {
                        holder.addCallback(object : SurfaceHolder.Callback {
                            override fun surfaceCreated(holder: SurfaceHolder) {}

                            override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
                                // Ask for the display's top rate; the engine
                                // only draws when something changed.
                                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                                    holder.surface.setFrameRate(120f, Surface.FRAME_RATE_COMPATIBILITY_DEFAULT)
                                }
                                NativeEngine.surfaceChanged(engine, holder.surface, width, height)
                                hdr.attach(this@apply)
                            }

                            override fun surfaceDestroyed(holder: SurfaceHolder) {
                                NativeEngine.surfaceDestroyed(engine)
                            }
                        })
                    }
                },
            )
            Histogram(
                engine,
                Modifier
                    .align(Alignment.TopEnd)
                    .statusBarsPadding()
                    .padding(12.dp)
                    .size(width = 144.dp, height = 72.dp),
            )
            // The SurfaceView's size, so its pixels are the surface's.
            if (cropping) {
                CropOverlay(engine, cropFrame, Modifier.fillMaxSize())
            } else {
                Box(
                    Modifier
                        .fillMaxSize()
                        .pointerInput(engine) {
                            detectTransformGestures { centroid, pan, zoom, _ ->
                                NativeEngine.transformView(engine, zoom, centroid.x, centroid.y, pan.x, pan.y)
                            }
                        }
                        .pointerInput(engine) {
                            detectTapGestures(onDoubleTap = { NativeEngine.toggleZoom(engine, it.x, it.y) })
                        },
                )
            }
        }

        Column(
            Modifier
                .fillMaxWidth()
                .background(Panel)
                .navigationBarsPadding()
                .padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 8.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    error ?: notice ?: if (photo) stats else "No photo yet",
                    color = if (error != null) Color(0xFFF87171) else if (notice != null) Color.White else Muted,
                    fontFamily = FontFamily.Monospace,
                    fontSize = 10.sp,
                    maxLines = 2,
                    modifier = Modifier.weight(1f),
                )
                TextButton(onClick = { picker.launch(arrayOf("image/*", "application/octet-stream")) }) {
                    Text("Open…", color = Orange)
                }
                if (hdr.supported) {
                    TextButton(onClick = {
                        hdrOn = !hdrOn
                        setHdr(hdrOn)
                    }) {
                        Text("HDR", color = if (hdrOn) Orange else Muted)
                    }
                }
                val canExport = opened > 0 && !exporting
                TextButton(
                    enabled = canExport,
                    onClick = {
                        exporting = true
                        error = null
                        notice = null
                        NativeEngine.export(engine)
                    },
                ) {
                    Text(if (exporting) "Exporting…" else "Export", color = if (canExport) Orange else Muted)
                }
            }
            Row(
                horizontalArrangement = Arrangement.spacedBy(20.dp),
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.padding(vertical = 4.dp),
            ) {
                Sections.forEach { s ->
                    Text(
                        s,
                        color = if (s == section) Orange else Muted,
                        fontSize = 14.sp,
                        fontWeight = if (s == section) FontWeight.SemiBold else FontWeight.Normal,
                        modifier = Modifier
                            .clickable { section = s }
                            .padding(vertical = 8.dp),
                    )
                }
                Spacer(Modifier.weight(1f))
                Row(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                    Action("Undo", enabled = history and 1 != 0) { NativeEngine.undo(engine) }
                    Action("Redo", enabled = history and 2 != 0) { NativeEngine.redo(engine) }
                    Action("Reset All", enabled = opened > 0) { NativeEngine.resetAll(engine) }
                }
            }
            val shown = Controls.withIndex().filter { it.value.section == section }
            shown.forEach { (i, c) ->
                SliderRow(
                    label = c.label,
                    value = values[i],
                    range = c.min..c.max,
                    shown = NativeEngine.display(engine, i, values[i]),
                    onChange = {
                        values[i] = it
                        NativeEngine.setEdit(engine, i, it)
                    },
                    onRelease = { NativeEngine.commitEdit(engine) },
                    onReset = {
                        values[i] = c.default
                        NativeEngine.setEdit(engine, i, c.default)
                        NativeEngine.commitEdit(engine)
                    },
                )
            }
            if (cropping) {
                CropTools(engine, landscape = cropFrame.size < 8 || cropFrame[2] >= cropFrame[3], enabled = opened > 0)
            }
            repeat(Rows - shown.size - (if (cropping) CropToolRows else 0)) { Spacer(Modifier.height(RowHeight)) }
        }
    }
}

private val RowHeight = 48.dp

private const val CropSection = "Crop"
private const val CropToolRows = 2

/** The crop on the surface, in pixels, from the engine's crop frame. */
private fun cropRect(frame: FloatArray): Rect? {
    if (frame.size < 8) return null
    val (x, y, w, h) = frame
    return Rect(x + frame[4] * w, y + frame[5] * h, x + (frame[4] + frame[6]) * w, y + (frame[5] + frame[7]) * h)
}

/**
 * The crop tool's handles over the photo (the engine dims what is cut away):
 * drag a corner to resize, inside to move. The engine owns the maths
 * (`Session::drag_crop`); this only says which handle and how far.
 */
@Composable
private fun CropOverlay(engine: Long, frame: FloatArray, modifier: Modifier) {
    val current by rememberUpdatedState(frame)
    Canvas(
        modifier.pointerInput(engine) {
            val reach = 32.dp.toPx()
            var which = -1
            detectDragGestures(
                onDragStart = { at ->
                    val r = cropRect(current)
                    which = when {
                        r == null -> -1
                        (at - r.topLeft).getDistance() < reach -> 0
                        (at - r.topRight).getDistance() < reach -> 1
                        (at - r.bottomLeft).getDistance() < reach -> 2
                        (at - r.bottomRight).getDistance() < reach -> 3
                        r.contains(at) -> 4
                        else -> -1
                    }
                },
                onDragEnd = { if (which >= 0) NativeEngine.cropEnd(engine) },
                onDragCancel = { if (which >= 0) NativeEngine.cropEnd(engine) },
            ) { change, _ ->
                if (which >= 0) {
                    val from = change.previousPosition
                    val to = change.position
                    NativeEngine.cropDrag(engine, which, from.x, from.y, to.x, to.y)
                }
            }
        },
    ) {
        val r = cropRect(frame) ?: return@Canvas
        drawCropFrame(r)
    }
}

private fun DrawScope.drawCropFrame(r: Rect) {
    val line = 1.dp.toPx()
    val thirds = Color(0x66FFFFFF)
    for (i in 1..2) {
        val x = r.left + r.width * i / 3
        val y = r.top + r.height * i / 3
        drawLine(thirds, Offset(x, r.top), Offset(x, r.bottom), line)
        drawLine(thirds, Offset(r.left, y), Offset(r.right, y), line)
    }
    drawRect(Color.White, r.topLeft, r.size, style = androidx.compose.ui.graphics.drawscope.Stroke(line))
    // Corner brackets: where a finger grabs.
    val arm = 20.dp.toPx()
    val thick = 3.dp.toPx()
    listOf(
        r.topLeft to Offset(1f, 1f),
        r.topRight to Offset(-1f, 1f),
        r.bottomLeft to Offset(1f, -1f),
        r.bottomRight to Offset(-1f, -1f),
    ).forEach { (corner, dir) ->
        drawLine(Color.White, corner, corner + Offset(arm * dir.x, 0f), thick)
        drawLine(Color.White, corner, corner + Offset(0f, arm * dir.y), thick)
    }
}

/** A row of actions under a label, lined up with the sliders' labels. */
@Composable
private fun ToolRow(label: String, actions: @Composable () -> Unit) {
    Row(Modifier.height(RowHeight), verticalAlignment = Alignment.CenterVertically) {
        Text(label, color = Color.White, fontSize = 13.sp, modifier = Modifier.width(80.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(20.dp)) { actions() }
    }
}

/** Quarter turns and centred aspect presets, in the photo's own orientation. */
@Composable
private fun CropTools(engine: Long, landscape: Boolean, enabled: Boolean) {
    ToolRow("Rotate") {
        Action("Left", enabled) { NativeEngine.turn(engine, false) }
        Action("Right", enabled) { NativeEngine.turn(engine, true) }
    }
    // (label, width / height) for a landscape photo; flipped for a portrait one.
    val presets = listOf("Full" to 0f, "1:1" to 1f, "5:4" to 5f / 4f, "3:2" to 3f / 2f, "16:9" to 16f / 9f)
    ToolRow("Aspect") {
        presets.forEach { (label, ratio) ->
            val (shown, r) = if (landscape || ratio == 0f || ratio == 1f) {
                label to ratio
            } else {
                label.split(':').reversed().joinToString(":") to 1f / ratio
            }
            Action(shown, enabled) { NativeEngine.cropAspect(engine, r) }
        }
    }
}

/** A word in the section row that does something; muted while it can't. */
@Composable
private fun Action(label: String, enabled: Boolean, onClick: () -> Unit) {
    Text(
        label,
        color = if (enabled) Color.White else Muted.copy(alpha = 0.5f),
        fontSize = 14.sp,
        modifier = Modifier
            .clickable(enabled = enabled, onClick = onClick)
            .padding(vertical = 8.dp),
    )
}

private val Channels = listOf(Color(0x80FF0000), Color(0x8000FF00), Color(0x800000FF))

/**
 * The engine's histogram (a frame behind the photo, which nobody can see),
 * drawn as the desktop draws it: each channel's bins against the tallest.
 * Looked at once per display frame, fetched only when it changed.
 */
@Composable
private fun Histogram(engine: Long, modifier: Modifier) {
    var counts by remember { mutableStateOf(IntArray(0)) }
    LaunchedEffect(engine) {
        var seen = -1
        while (true) {
            withFrameNanos { }
            val version = NativeEngine.histogramVersion(engine)
            if (version != seen) {
                seen = version
                counts = NativeEngine.histogram(engine)
            }
        }
    }
    if (counts.size != 769) return
    // A hairline so the box shows over a dark photo.
    Canvas(modifier.background(Color(0x990A0A0A)).border(1.dp, Color(0x33FFFFFF))) {
        // The engine's scale: the clipped end bins don't set it, and are
        // drawn clamped to the top.
        val max = counts[768].coerceAtLeast(1).toFloat()
        val bar = size.width / 256f
        for (c in 0 until 3) {
            val path = Path()
            path.moveTo(0f, size.height)
            for (i in 0 until 256) {
                val top = size.height * (1f - (counts[c * 256 + i] / max).coerceAtMost(1f))
                path.lineTo(i * bar, top)
                path.lineTo((i + 1) * bar, top)
            }
            path.lineTo(size.width, size.height)
            path.close()
            drawPath(path, Channels[c])
        }
    }
}

/** Label (double tap resets), track, and the value as the engine reads it. */
@Composable
private fun SliderRow(
    label: String,
    value: Float,
    range: ClosedFloatingPointRange<Float>,
    shown: String,
    onChange: (Float) -> Unit,
    onRelease: () -> Unit,
    onReset: () -> Unit,
) {
    Row(Modifier.height(RowHeight), verticalAlignment = Alignment.CenterVertically) {
        Text(
            label,
            color = Color.White,
            fontSize = 13.sp,
            modifier = Modifier
                .width(80.dp)
                .pointerInput(Unit) { detectTapGestures(onDoubleTap = { onReset() }) },
        )
        Slider(
            value = value,
            onValueChange = onChange,
            onValueChangeFinished = onRelease,
            valueRange = range,
            colors = SliderDefaults.colors(thumbColor = Orange, activeTrackColor = Orange, inactiveTrackColor = Color(0xFF3F3F46)),
            modifier = Modifier.weight(1f),
        )
        Text(
            shown,
            color = Muted,
            fontFamily = FontFamily.Monospace,
            fontSize = 12.sp,
            textAlign = TextAlign.End,
            modifier = Modifier.width(72.dp),
        )
    }
}
