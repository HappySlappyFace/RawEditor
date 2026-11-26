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
import androidx.compose.foundation.background
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
import androidx.compose.runtime.mutableFloatStateOf
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

@Composable
private fun Editor(engine: Long, hasPhoto: Boolean, importAndOpen: suspend (Uri) -> String?) {
    var exposure by remember { mutableFloatStateOf(0f) }
    var stats by remember { mutableStateOf("") }
    var opened by remember { mutableIntStateOf(0) }
    var photo by remember { mutableStateOf(hasPhoto) }
    var error by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(engine) {
        while (true) {
            stats = NativeEngine.stats(engine)
            // A photo opened: the slider starts at its saved exposure.
            val count = NativeEngine.openedCount(engine)
            if (count != opened) {
                opened = count
                exposure = NativeEngine.openedExposure(engine)
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

    Box(Modifier.fillMaxSize().background(Color(0xFF0A0A0A))) {
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

        // Gestures on the photo. This layer is the SurfaceView's size, so its
        // pixels are the surface's; the controls below sit above it and keep
        // their own touches.
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

        Column(
            Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .background(Color(0xCC0A0A0A))
                .navigationBarsPadding()
                .padding(horizontal = 20.dp, vertical = 12.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    error ?: if (photo) stats else "No photo yet",
                    color = if (error != null) Color(0xFFF87171) else Muted,
                    fontFamily = FontFamily.Monospace,
                    fontSize = 11.sp,
                    modifier = Modifier.weight(1f),
                )
                TextButton(onClick = { picker.launch(arrayOf("image/*", "application/octet-stream")) }) {
                    Text("Open…", color = Orange)
                }
            }
            Text(
                "Exposure  %+.2f EV".format(exposure),
                color = Color.White,
                fontSize = 14.sp,
                modifier = Modifier.padding(top = 4.dp),
            )
            Slider(
                value = exposure,
                onValueChange = {
                    exposure = it
                    NativeEngine.setExposure(engine, it)
                },
                valueRange = -3f..3f,
                colors = SliderDefaults.colors(thumbColor = Orange, activeTrackColor = Orange),
            )
        }
    }
}
