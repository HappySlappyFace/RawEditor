package dev.raweditor

import android.os.Build
import android.os.Bundle
import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.delay

/** RAW extensions the engine opens (Bayer formats, as `RAW_EXTENSIONS`). */
private val RAW = setOf("nef", "cr2", "cr3", "arw", "raf", "rw2", "orf", "dng", "pef", "srw", "nrw")

private val Orange = Color(0xFFF97316)

class MainActivity : ComponentActivity() {
    private var engine = 0L

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        engine = NativeEngine.create(filesDir.absolutePath)
        // For now the photo is the first RAW pushed into the app's own
        // folder (tools/android-app.sh does that); no picker yet.
        val raw = getExternalFilesDir(null)?.listFiles()
            ?.filter { it.extension.lowercase() in RAW }
            ?.minByOrNull { it.name }
        if (raw != null) NativeEngine.open(engine, raw.absolutePath)
        setContent { Editor(engine, hasPhoto = raw != null) }
    }

    override fun onDestroy() {
        NativeEngine.destroy(engine)
        engine = 0L
        super.onDestroy()
    }
}

@Composable
private fun Editor(engine: Long, hasPhoto: Boolean) {
    var exposure by remember { mutableFloatStateOf(0f) }
    var stats by remember { mutableStateOf("") }
    LaunchedEffect(engine) {
        while (true) {
            stats = NativeEngine.stats(engine)
            delay(250)
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

        Column(
            Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .background(Color(0xCC0A0A0A))
                .navigationBarsPadding()
                .padding(horizontal = 20.dp, vertical = 12.dp),
        ) {
            Text(
                if (hasPhoto) stats else "No photo: push a RAW into Android/data/dev.raweditor/files/",
                color = Color(0xFFA1A1AA),
                fontFamily = FontFamily.Monospace,
                fontSize = 11.sp,
            )
            Text(
                "Exposure  %+.2f EV".format(exposure),
                color = Color.White,
                fontSize = 14.sp,
                modifier = Modifier.padding(top = 8.dp),
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
