package dev.raweditor

import android.net.Uri
import android.os.Build
import android.os.Bundle
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
import androidx.compose.foundation.gestures.detectTapGestures
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

    /** Where photos live: the app's own folder, which adb can push into too. */
    private val photos: File get() = getExternalFilesDir(null) ?: filesDir

    private val prefs get() = getSharedPreferences("raweditor", MODE_PRIVATE)

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        engine = NativeEngine.create(filesDir.absolutePath)
        // Reopen the last photo, else the first RAW in the folder.
        val last = prefs.getString("last", null)?.let(::File)?.takeIf { it.exists() }
        val raw = last ?: photos.listFiles()
            ?.filter { it.extension.lowercase() in RAW }
            ?.minByOrNull { it.name }
        if (raw != null) open(raw)
        setContent { Editor(engine, hasPhoto = raw != null, ::importAndOpen) }
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

    override fun onDestroy() {
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
private fun Editor(engine: Long, hasPhoto: Boolean, importAndOpen: suspend (Uri) -> String?) {
    val values = remember { mutableStateListOf(*Controls.map { it.default }.toTypedArray()) }
    var section by remember { mutableStateOf(Sections.first()) }
    var stats by remember { mutableStateOf("") }
    var opened by remember { mutableIntStateOf(0) }
    var photo by remember { mutableStateOf(hasPhoto) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(engine) {
        while (true) {
            stats = NativeEngine.stats(engine)
            // A photo opened: the sliders start at its saved edit.
            val count = NativeEngine.openedCount(engine)
            if (count != opened) {
                opened = count
                NativeEngine.openedValues(engine).forEachIndexed { i, v -> if (i < values.size) values[i] = v }
            }
            delay(250)
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

        Column(
            Modifier
                .fillMaxWidth()
                .background(Panel)
                .navigationBarsPadding()
                .padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 8.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    error ?: if (photo) stats else "No photo yet",
                    color = if (error != null) Color(0xFFF87171) else Muted,
                    fontFamily = FontFamily.Monospace,
                    fontSize = 10.sp,
                    maxLines = 2,
                    modifier = Modifier.weight(1f),
                )
                TextButton(onClick = { picker.launch(arrayOf("image/*", "application/octet-stream")) }) {
                    Text("Open…", color = Orange)
                }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(20.dp), modifier = Modifier.padding(vertical = 4.dp)) {
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
            repeat(Rows - shown.size) { Spacer(Modifier.height(RowHeight)) }
        }
    }
}

private val RowHeight = 48.dp

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
    if (counts.size != 768) return
    Canvas(modifier.background(Color(0x990A0A0A))) {
        val max = counts.max().coerceAtLeast(1).toFloat()
        val bar = size.width / 256f
        for (c in 0 until 3) {
            val path = Path()
            path.moveTo(0f, size.height)
            for (i in 0 until 256) {
                path.lineTo(i * bar, size.height * (1f - counts[c * 256 + i] / max))
                path.lineTo((i + 1) * bar, size.height * (1f - counts[c * 256 + i] / max))
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
