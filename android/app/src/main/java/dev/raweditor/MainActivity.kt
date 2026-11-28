package dev.raweditor

import android.content.ActivityNotFoundException
import android.content.ContentValues
import android.content.Intent
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
import androidx.activity.SystemBarStyle
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.horizontalDrag
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.systemGestureExclusion
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalViewConfiguration
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

/** RAW extensions the engine opens (Bayer formats, as `RAW_EXTENSIONS`). */
private val RAW = setOf("nef", "cr2", "cr3", "arw", "raf", "rw2", "orf", "dng", "pef", "srw", "nrw")

class MainActivity : ComponentActivity() {
    private var engine = 0L
    private lateinit var hdr: Hdr

    /** Where photos live: the app's own folder, which adb can push into too. */
    private val photos: File get() = getExternalFilesDir(null) ?: filesDir

    private val prefs get() = getSharedPreferences("raweditor", MODE_PRIVATE)

    override fun onCreate(savedInstanceState: Bundle?) {
        // The app is dark whatever the system's theme: light bar icons.
        val bars = SystemBarStyle.dark(android.graphics.Color.TRANSPARENT)
        enableEdgeToEdge(statusBarStyle = bars, navigationBarStyle = bars)
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
        setContent {
            RawEditorTheme {
                Editor(engine, hasPhoto = raw != null, ::importAndOpen, ::saveToGallery, ::viewInGallery, hdr, ::setHdr)
            }
        }
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
     * where it went.
     */
    private suspend fun saveToGallery(path: String): Result<Uri> = withContext(Dispatchers.IO) {
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
                uri
            }
        } finally {
            file.delete()
        }
    }

    /** Show a saved export in whichever app views photos. False if none does. */
    private fun viewInGallery(uri: Uri): Boolean = try {
        startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(uri, "image/jpeg").addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
        true
    } catch (e: ActivityNotFoundException) {
        false
    }

    override fun onDestroy() {
        hdr.release()
        NativeEngine.destroy(engine)
        engine = 0L
        super.onDestroy()
    }
}

/** One slider, as the engine describes it ([NativeEngine.controls]). */
private data class Control(val section: String, val label: String, val min: Float, val max: Float, val default: Float, val rail: String)

private val Controls: List<Control> = NativeEngine.controls().lines().map { line ->
    val f = line.split('\t')
    Control(f[0], f[1], f[2].toFloat(), f[3].toFloat(), f[4].toFloat(), f.getOrElse(5) { "plain" })
}
private val Sections: List<String> = Controls.map { it.section }.distinct()
/** Every section's panel is as tall as the longest, so the photo above never resizes. */
private val Rows: Int = Sections.maxOf { s -> Controls.count { it.section == s } }

/** Each section's tab icon; a section the app doesn't know gets the sliders. */
private fun sectionIcon(section: String) = when (section) {
    "Light" -> Icons.Light
    "Colour" -> Icons.Colour
    "Presence" -> Icons.Presence
    CropSection -> Icons.Crop
    else -> Icons.Sliders
}

/** The open photo, as the engine reports it ([NativeEngine.photo]). */
private data class PhotoInfo(
    val state: String = "none",
    val name: String = "",
    val camera: String = "",
    val exposure: String = "",
    val lens: String = "",
    val error: String = "",
) {
    companion object {
        fun parse(line: String): PhotoInfo {
            val f = line.split('\t')
            fun at(i: Int) = f.getOrElse(i) { "" }
            return PhotoInfo(at(0), at(1), at(2), at(3), at(4), at(5))
        }
    }
}

/** A message over the bottom of the photo: what happened, maybe an action. */
private data class Notice(val text: String, val isError: Boolean = false, val action: String? = null, val onAction: (() -> Unit)? = null)

@Composable
private fun Editor(
    engine: Long,
    hasPhoto: Boolean,
    importAndOpen: suspend (Uri) -> String?,
    saveToGallery: suspend (String) -> Result<Uri>,
    viewInGallery: (Uri) -> Boolean,
    hdr: Hdr,
    setHdr: (Boolean) -> Unit,
) {
    val values = remember { mutableStateListOf(*Controls.map { it.default }.toTypedArray()) }
    var section by remember { mutableStateOf(Sections.first()) }
    var stats by remember { mutableStateOf("") }
    var showTimings by remember { mutableStateOf(false) }
    var photo by remember { mutableStateOf(PhotoInfo()) }
    // A photo was asked for and the engine hasn't said so yet.
    var expecting by remember { mutableStateOf(hasPhoto) }
    var opened by remember { mutableIntStateOf(0) }
    var notice by remember { mutableStateOf<Notice?>(null) }
    var exporting by remember { mutableStateOf(false) }
    var hdrOn by remember { mutableStateOf(hdr.enabled) }
    var history by remember { mutableIntStateOf(0) }
    var peeking by remember { mutableStateOf(false) }
    var cropFrame by remember { mutableStateOf(FloatArray(0)) }
    val cropping = section == CropSection
    val ready = photo.state == "ready"
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

    // The Crop tab is the crop tool. Its frame stays clear of the screen's
    // edges (the sides are the back gesture's) and of the bar over the top.
    val density = LocalDensity.current
    val statusBar = WindowInsets.statusBars.getTop(density)
    LaunchedEffect(cropping, statusBar) {
        with(density) {
            val side = CropSideInset.toPx()
            val top = statusBar + (Space.TopBar + CropEdgeInset).toPx()
            NativeEngine.setCropping(engine, cropping, side, top, side, CropEdgeInset.toPx())
        }
    }

    LaunchedEffect(engine) {
        while (true) {
            stats = NativeEngine.stats(engine)
            opened = NativeEngine.openedCount(engine)
            val now = PhotoInfo.parse(NativeEngine.photo(engine))
            if (now != photo) photo = now
            if (now.state != "none") expecting = false
            // An export finished: into the gallery with it.
            NativeEngine.takeExport(engine)?.let { result ->
                val (kind, text) = result.split('\t', limit = 2).let { it[0] to it.getOrElse(1) { "" } }
                notice = if (kind == "ok") {
                    saveToGallery(text).fold(
                        { uri ->
                            Notice("Saved to Pictures/RawEditor", action = "View") {
                                if (!viewInGallery(uri)) notice = Notice("No app here shows photos", isError = true)
                            }
                        },
                        { Notice(it.message ?: "Could not save to the gallery", isError = true) },
                    )
                } else {
                    Notice("Export failed: $text", isError = true)
                }
                exporting = false
            }
            delay(250)
        }
    }

    // A notice goes after a few seconds.
    LaunchedEffect(notice) {
        if (notice != null) {
            delay(if (notice?.action != null) 6000 else 4000)
            notice = null
        }
    }

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) {
            notice = null
            expecting = true
            scope.launch {
                importAndOpen(uri)?.let {
                    notice = Notice(it, isError = true)
                    expecting = false
                }
            }
        }
    }
    val openPhoto = { picker.launch(arrayOf("image/*", "application/octet-stream")) }

    Column(Modifier.fillMaxSize().background(Colours.Chrome)) {
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
            // The SurfaceView's size, so its pixels are the surface's.
            if (cropping) {
                CropOverlay(engine, cropFrame, Modifier.fillMaxSize())
            } else {
                PhotoGestures(engine, enabled = ready, onPeek = { peeking = it }, modifier = Modifier.fillMaxSize())
            }

            when {
                photo.state == "loading" || (expecting && photo.state == "none") -> Opening(photo.name, Modifier.align(Alignment.Center))
                photo.state == "failed" -> Message(
                    icon = Icons.Warning,
                    title = "${photo.name.ifEmpty { "The photo" }} did not open",
                    body = photo.error,
                    action = "Open Another…",
                    onAction = openPhoto,
                    modifier = Modifier.align(Alignment.Center),
                )
                photo.state == "none" -> Message(
                    icon = Icons.Image,
                    title = "No photo open",
                    body = "Open a RAW file to start editing.",
                    action = "Open Photo…",
                    onAction = openPhoto,
                    modifier = Modifier.align(Alignment.Center),
                )
            }

            Column(Modifier.fillMaxWidth().align(Alignment.TopCenter)) {
                TopBar(
                    photo = photo,
                    detail = if (showTimings) stats else null,
                    canUndo = history and 1 != 0,
                    canRedo = history and 2 != 0,
                    hdrSupported = hdr.supported,
                    hdrOn = hdrOn,
                    exporting = exporting,
                    canExport = opened > 0 && ready && !exporting,
                    showTimings = showTimings,
                    onUndo = { NativeEngine.undo(engine) },
                    onRedo = { NativeEngine.redo(engine) },
                    onHdr = {
                        hdrOn = !hdrOn
                        setHdr(hdrOn)
                    },
                    onExport = {
                        exporting = true
                        notice = null
                        NativeEngine.export(engine)
                    },
                    onOpen = openPhoto,
                    onResetAll = { NativeEngine.resetAll(engine) },
                    onToggleTimings = { showTimings = !showTimings },
                )
                // Not over the crop tool: its top right corner is a handle.
                if (!cropping && ready) {
                    Histogram(
                        engine,
                        Modifier
                            .align(Alignment.End)
                            .padding(end = Space.Gutter)
                            .size(width = 144.dp, height = 72.dp),
                    )
                }
            }

            if (peeking) {
                Badge("BEFORE", Modifier.align(Alignment.BottomStart).padding(Space.Gutter))
            }
            notice?.let { n ->
                NoticeBar(n, onDismiss = { notice = null }, modifier = Modifier.align(Alignment.BottomCenter).padding(Space.XL))
            }
        }

        Column(
            Modifier
                .fillMaxWidth()
                .background(Colours.Bg)
                .navigationBarsPadding()
                .padding(start = Space.Gutter, end = Space.Gutter, bottom = Space.M),
        ) {
            Tabs(section, onSelect = { section = it })
            val shown = Controls.withIndex().filter { it.value.section == section }
            shown.forEach { (i, c) ->
                SliderRow(
                    label = c.label,
                    value = values[i],
                    range = c.min..c.max,
                    rail = Rails.colours(c.rail),
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
                CropTools(engine, landscape = cropFrame.size < 8 || cropFrame[2] >= cropFrame[3], enabled = ready)
            }
            repeat(Rows - shown.size - (if (cropping) CropToolRows else 0)) { Spacer(Modifier.height(Space.Row)) }
        }
    }
}

/**
 * Over the top of the photo: its name and exposure (long press for the
 * frame timings), then Undo, Redo, HDR, Export and a menu.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun TopBar(
    photo: PhotoInfo,
    detail: String?,
    canUndo: Boolean,
    canRedo: Boolean,
    hdrSupported: Boolean,
    hdrOn: Boolean,
    exporting: Boolean,
    canExport: Boolean,
    showTimings: Boolean,
    onUndo: () -> Unit,
    onRedo: () -> Unit,
    onHdr: () -> Unit,
    onExport: () -> Unit,
    onOpen: () -> Unit,
    onResetAll: () -> Unit,
    onToggleTimings: () -> Unit,
) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            // A scrim, so the bar reads over a bright photo.
            .background(Brush.verticalGradient(listOf(Color(0x99000000), Color.Transparent)))
            .statusBarsPadding()
            .height(Space.TopBar)
            .padding(start = Space.Gutter, end = Space.XS),
    ) {
        Column(
            Modifier
                .weight(1f)
                .combinedClickable(onClick = {}, onLongClick = onToggleTimings),
        ) {
            if (photo.name.isNotEmpty()) {
                Text(
                    photo.name,
                    color = Colours.OnPhoto,
                    fontSize = TextSize.Label,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            val line = detail ?: listOf(photo.exposure, photo.lens).filter { it.isNotEmpty() }.joinToString("  ·  ")
            if (line.isNotEmpty()) {
                Text(
                    line,
                    color = Colours.TextSecondary,
                    fontFamily = Mono,
                    fontSize = TextSize.Caption,
                    maxLines = if (detail != null) 2 else 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        IconButton(Icons.Undo, "Undo", canUndo, onUndo)
        IconButton(Icons.Redo, "Redo", canRedo, onRedo)
        if (hdrSupported) {
            Box(
                contentAlignment = Alignment.Center,
                modifier = Modifier.size(Space.Touch).clickable(onClick = onHdr),
            ) {
                Text(
                    "HDR",
                    color = if (hdrOn) Colours.Accent else Colours.TextMuted,
                    fontSize = TextSize.Caption,
                    fontWeight = FontWeight.Bold,
                    modifier = Modifier
                        .border(1.dp, if (hdrOn) Colours.Accent else Colours.TextMuted, RoundedCornerShape(Radius.Control))
                        .padding(horizontal = Space.XS, vertical = Space.XXS),
                )
            }
        }
        if (exporting) {
            Box(Modifier.size(Space.Touch), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(Modifier.size(18.dp), color = Colours.Accent, strokeWidth = 2.dp)
            }
        } else {
            IconButton(Icons.Export, "Export", canExport, onExport, tint = Colours.Accent)
        }
        var menu by remember { mutableStateOf(false) }
        Box {
            IconButton(Icons.More, "More", true, { menu = true })
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                DropdownMenuItem(text = { Text("Open Photo…") }, onClick = { menu = false; onOpen() })
                DropdownMenuItem(
                    text = { Text("Reset All") },
                    enabled = photo.state == "ready",
                    onClick = { menu = false; onResetAll() },
                )
                DropdownMenuItem(
                    text = { Text(if (showTimings) "Hide Timings" else "Show Timings") },
                    onClick = { menu = false; onToggleTimings() },
                )
            }
        }
    }
}

/** A glyph from the icon font in a touch-sized target; muted while it can't act. */
@Composable
private fun IconButton(glyph: String, description: String, enabled: Boolean, onClick: () -> Unit, tint: Color = Colours.OnPhoto) {
    Box(
        contentAlignment = Alignment.Center,
        modifier = Modifier.size(Space.Touch).clickable(enabled = enabled, onClickLabel = description, onClick = onClick),
    ) {
        Icon(glyph, TextSize.Heading, if (enabled) tint else Colours.TextDisabled)
    }
}

@Composable
private fun Icon(glyph: String, size: TextUnit, colour: Color) {
    Text(glyph, fontFamily = IconFont, fontSize = size, color = colour)
}

/** The sections, each an icon over its name. */
@Composable
private fun Tabs(selected: String, onSelect: (String) -> Unit) {
    Row(Modifier.fillMaxWidth()) {
        Sections.forEach { s ->
            val on = s == selected
            val colour = if (on) Colours.Accent else Colours.TextMuted
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                modifier = Modifier
                    .weight(1f)
                    .clickable { onSelect(s) }
                    .padding(top = Space.M),
            ) {
                Icon(sectionIcon(s), TextSize.Label, colour)
                Text(
                    s,
                    color = colour,
                    fontSize = TextSize.Small,
                    fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal,
                    modifier = Modifier.padding(top = Space.XXS, bottom = Space.S),
                )
                Box(
                    Modifier
                        .width(24.dp)
                        .height(2.dp)
                        .background(if (on) Colours.Accent else Color.Transparent, RoundedCornerShape(1.dp)),
                )
            }
        }
    }
}

/**
 * Pinch and drag to zoom and pan, double tap for fit or 3×, and hold to see
 * the photo unedited (the desktop's Space peek). A hold that turns into a
 * drag is a drag: the pan takes the pointer and the peek never starts.
 */
@Composable
private fun PhotoGestures(engine: Long, enabled: Boolean, onPeek: (Boolean) -> Unit, modifier: Modifier) {
    val hold = LocalViewConfiguration.current.longPressTimeoutMillis
    val canPeek by rememberUpdatedState(enabled)
    Box(
        modifier
            .pointerInput(engine) {
                detectTransformGestures { centroid, pan, zoom, _ ->
                    NativeEngine.transformView(engine, zoom, centroid.x, centroid.y, pan.x, pan.y)
                }
            }
            .pointerInput(engine) {
                coroutineScopeFor { scope ->
                    detectTapGestures(
                        onDoubleTap = { NativeEngine.toggleZoom(engine, it.x, it.y) },
                        onPress = {
                            var peek = false
                            val timer = scope.launch {
                                delay(hold)
                                if (canPeek) {
                                    peek = true
                                    NativeEngine.setPeek(engine, true)
                                    onPeek(true)
                                }
                            }
                            tryAwaitRelease()
                            timer.cancel()
                            if (peek) {
                                NativeEngine.setPeek(engine, false)
                                onPeek(false)
                            }
                        },
                    )
                }
            },
    )
}

/** Runs [block] with a scope that lives as long as the pointer input. */
private suspend fun coroutineScopeFor(block: suspend (kotlinx.coroutines.CoroutineScope) -> Unit) =
    kotlinx.coroutines.coroutineScope { block(this) }

/** While a photo opens: a spinner and its name. */
@Composable
private fun Opening(name: String, modifier: Modifier) {
    Column(modifier, horizontalAlignment = Alignment.CenterHorizontally) {
        CircularProgressIndicator(Modifier.size(28.dp), color = Colours.Accent, strokeWidth = 2.5.dp)
        Text(
            if (name.isEmpty()) "Opening…" else "Opening $name…",
            color = Colours.TextSecondary,
            fontSize = TextSize.Body,
            modifier = Modifier.padding(top = Space.XL),
        )
    }
}

/** A centred message where the photo would be, with one thing to do about it. */
@Composable
private fun Message(icon: String, title: String, body: String, action: String, onAction: () -> Unit, modifier: Modifier) {
    Column(modifier.padding(horizontal = 32.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Icon(icon, TextSize.Display, Colours.TextMuted)
        Text(
            title,
            color = Colours.Text,
            fontSize = TextSize.Heading,
            fontWeight = FontWeight.SemiBold,
            textAlign = TextAlign.Center,
            modifier = Modifier.padding(top = Space.XL),
        )
        if (body.isNotEmpty()) {
            Text(
                body,
                color = Colours.TextMuted,
                fontSize = TextSize.Body,
                textAlign = TextAlign.Center,
                modifier = Modifier.padding(top = Space.XS),
            )
        }
        Text(
            action,
            color = Colours.OnAccent,
            fontSize = TextSize.Label,
            fontWeight = FontWeight.SemiBold,
            modifier = Modifier
                .padding(top = Space.XXL)
                .clip(RoundedCornerShape(Radius.Card))
                .background(Colours.Accent)
                .clickable(onClick = onAction)
                .padding(horizontal = Space.XXL, vertical = Space.L),
        )
    }
}

/** A small label over the photo. */
@Composable
private fun Badge(text: String, modifier: Modifier) {
    Text(
        text,
        color = Colours.OnPhoto,
        fontSize = TextSize.Caption,
        fontWeight = FontWeight.Bold,
        modifier = modifier
            .background(Color(0x99000000), RoundedCornerShape(Radius.Control))
            .padding(horizontal = Space.M, vertical = Space.XS),
    )
}

@Composable
private fun NoticeBar(notice: Notice, onDismiss: () -> Unit, modifier: Modifier) {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(Radius.Dialog))
            .background(Colours.SurfaceRaised)
            .border(1.dp, Colours.BorderSubtle, RoundedCornerShape(Radius.Dialog))
            .clickable(onClick = onDismiss)
            .padding(start = Space.XXL, end = Space.S),
    ) {
        Text(
            notice.text,
            color = if (notice.isError) Colours.Error else Colours.Text,
            fontSize = TextSize.Body,
            modifier = Modifier.weight(1f).padding(vertical = Space.XL),
        )
        if (notice.action != null && notice.onAction != null) {
            Text(
                notice.action,
                color = Colours.Accent,
                fontSize = TextSize.Body,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier
                    .clickable {
                        onDismiss()
                        notice.onAction.invoke()
                    }
                    .padding(horizontal = Space.L, vertical = Space.XL),
            )
        }
    }
}

private const val CropSection = "Crop"
/** The crop frame's distance from the screen's sides (the back gesture's) and from the bars and panel. */
private val CropSideInset = 40.dp
private val CropEdgeInset = 16.dp
/** How far from a corner a finger still grabs it. */
private val CropReach = 32.dp
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
    val reach = with(LocalDensity.current) { CropReach.toPx() }
    // The back gesture would take a drag that starts near a side: not
    // around the corners.
    val corners = cropRect(frame)?.let { listOf(it.topLeft, it.topRight, it.bottomLeft, it.bottomRight) }.orEmpty()
    val excluded = corners.fold(modifier) { m, c ->
        m.systemGestureExclusion { Rect(c.x - reach, c.y - reach, c.x + reach, c.y + reach) }
    }
    Canvas(
        excluded.pointerInput(engine) {
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
    // Over the photo: literal, as the desktop's canvas overlays are.
    val thirds = Color(0x66FFFFFF)
    for (i in 1..2) {
        val x = r.left + r.width * i / 3
        val y = r.top + r.height * i / 3
        drawLine(thirds, Offset(x, r.top), Offset(x, r.bottom), line)
        drawLine(thirds, Offset(r.left, y), Offset(r.right, y), line)
    }
    drawRect(Colours.OnPhoto, r.topLeft, r.size, style = Stroke(line))
    // Corner brackets: where a finger grabs.
    val arm = 20.dp.toPx()
    val thick = 3.dp.toPx()
    listOf(
        r.topLeft to Offset(1f, 1f),
        r.topRight to Offset(-1f, 1f),
        r.bottomLeft to Offset(1f, -1f),
        r.bottomRight to Offset(-1f, -1f),
    ).forEach { (corner, dir) ->
        drawLine(Colours.OnPhoto, corner, corner + Offset(arm * dir.x, 0f), thick)
        drawLine(Colours.OnPhoto, corner, corner + Offset(0f, arm * dir.y), thick)
    }
}

/** A row of actions under a label, lined up with the sliders' labels. */
@Composable
private fun ToolRow(label: String, actions: @Composable () -> Unit) {
    Row(Modifier.height(Space.Row), verticalAlignment = Alignment.CenterVertically) {
        Text(label, color = Colours.Text, fontSize = TextSize.Body, modifier = Modifier.width(LabelWidth))
        Row(horizontalArrangement = Arrangement.spacedBy(Space.S)) { actions() }
    }
}

/** A small outlined button, as the desktop's neutral buttons. */
@Composable
private fun Chip(label: String, enabled: Boolean, onClick: () -> Unit) {
    val shape = RoundedCornerShape(Radius.Control)
    Text(
        label,
        color = if (enabled) Colours.Text else Colours.TextDisabled,
        fontSize = TextSize.Small,
        modifier = Modifier
            .clip(shape)
            .background(Colours.Control)
            .border(1.dp, Colours.Border, shape)
            .clickable(enabled = enabled, onClick = onClick)
            .padding(horizontal = Space.L, vertical = Space.S),
    )
}

/** Quarter turns and centred aspect presets, in the photo's own orientation. */
@Composable
private fun CropTools(engine: Long, landscape: Boolean, enabled: Boolean) {
    ToolRow("Rotate") {
        Chip("Left", enabled) { NativeEngine.turn(engine, false) }
        Chip("Right", enabled) { NativeEngine.turn(engine, true) }
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
            Chip(shown, enabled) { NativeEngine.cropAspect(engine, r) }
        }
    }
}

private val LabelWidth: Dp = 80.dp

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
    // A hairline so the box shows over a dark photo (literal: over the photo).
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

private val RailWidth = 4.dp
private val HandleRadius = 7.dp

/**
 * The desktop's slider (`ProSlider`): a thin grey rail, or a gradient of
 * what the slider does, and a light round handle, brighter while held.
 * Touching the rail away from the handle jumps there, as a click does on
 * the desktop; a drag that starts on the handle moves it by the finger's
 * distance, so a fine adjustment never jumps first.
 */
@Composable
private fun RailSlider(
    value: Float,
    range: ClosedFloatingPointRange<Float>,
    rail: List<Color>?,
    onChange: (Float) -> Unit,
    onRelease: () -> Unit,
    modifier: Modifier,
) {
    var held by remember { mutableStateOf(false) }
    val current by rememberUpdatedState(value)
    val change by rememberUpdatedState(onChange)
    val release by rememberUpdatedState(onRelease)
    val span = range.endInclusive - range.start
    Canvas(
        modifier.pointerInput(range) {
            val inset = HandleRadius.toPx()
            val width = (size.width - 2 * inset).coerceAtLeast(1f)
            fun xOf(v: Float) = inset + (v - range.start) / span * width
            fun valueAt(x: Float) = range.start + ((x - inset) / width).coerceIn(0f, 1f) * span
            awaitEachGesture {
                val down = awaitFirstDown()
                held = true
                var v = current
                if (kotlin.math.abs(down.position.x - xOf(v)) > viewConfiguration.touchSlop * 2) {
                    v = valueAt(down.position.x)
                    change(v)
                }
                down.consume()
                horizontalDrag(down.id) { move ->
                    v = (v + move.positionChange().x / width * span).coerceIn(range.start, range.endInclusive)
                    change(v)
                    move.consume()
                }
                held = false
                release()
            }
        },
    ) {
        val inset = HandleRadius.toPx()
        val y = size.height / 2
        val railH = RailWidth.toPx()
        val railTop = Offset(inset, y - railH / 2)
        val railSize = androidx.compose.ui.geometry.Size(size.width - 2 * inset, railH)
        val corner = androidx.compose.ui.geometry.CornerRadius(railH / 2)
        if (rail != null) {
            drawRoundRect(Brush.horizontalGradient(rail, startX = inset, endX = size.width - inset), railTop, railSize, corner)
        } else {
            drawRoundRect(Colours.ControlActive, railTop, railSize, corner)
        }
        val x = inset + ((current - range.start) / span).coerceIn(0f, 1f) * (size.width - 2 * inset)
        drawCircle(if (held) Colours.TextStrong else Colours.Text, HandleRadius.toPx(), Offset(x, y))
        drawCircle(Colours.TextStrong, HandleRadius.toPx(), Offset(x, y), style = Stroke(1.dp.toPx()))
    }
}

/** Label (double tap resets), track, and the value as the engine reads it. */
@Composable
private fun SliderRow(
    label: String,
    value: Float,
    range: ClosedFloatingPointRange<Float>,
    rail: List<Color>?,
    shown: String,
    onChange: (Float) -> Unit,
    onRelease: () -> Unit,
    onReset: () -> Unit,
) {
    Row(Modifier.height(Space.Row), verticalAlignment = Alignment.CenterVertically) {
        Text(
            label,
            color = Colours.Text,
            fontSize = TextSize.Body,
            modifier = Modifier
                .width(LabelWidth)
                .pointerInput(Unit) { detectTapGestures(onDoubleTap = { onReset() }) },
        )
        RailSlider(value, range, rail, onChange, onRelease, Modifier.weight(1f).fillMaxHeight())
        Text(
            shown,
            color = Colours.TextSecondary,
            fontFamily = Mono,
            fontSize = TextSize.Small,
            textAlign = TextAlign.End,
            modifier = Modifier.width(72.dp),
        )
    }
}
