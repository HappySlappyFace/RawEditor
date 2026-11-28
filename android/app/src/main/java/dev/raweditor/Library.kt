package dev.raweditor

import android.graphics.BitmapFactory
import android.util.LruCache
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** One photo in the catalogue ([NativeEngine.libraryList]). */
data class LibraryItem(val id: Long, val name: String, val path: String, val thumb: String) {
    companion object {
        /** The engine's list: one photo per line, newest first. */
        fun parse(list: String): List<LibraryItem> = list.lines().mapNotNull { line ->
            val f = line.split('\t')
            val id = f.getOrNull(0)?.toLongOrNull() ?: return@mapNotNull null
            LibraryItem(id, f.getOrElse(1) { "" }, f.getOrElse(2) { "" }, f.getOrElse(3) { "" })
        }
    }
}

/**
 * Decoded thumbnails, by file. The session writes each thumbnail once under
 * its own name (an edited one gets a new name every save), so a path never
 * goes stale. Bounded by memory, not count.
 */
private object Thumbnails {
    private val cache = object : LruCache<String, ImageBitmap>(48 * 1024 * 1024) {
        override fun sizeOf(key: String, value: ImageBitmap) = value.width * value.height * 4
    }

    /** About this many pixels on the long edge: a grid cell's, not the file's. */
    private const val TARGET = 360

    fun cached(path: String): ImageBitmap? = cache.get(path)

    suspend fun load(path: String): ImageBitmap? = cache.get(path) ?: withContext(Dispatchers.IO) {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeFile(path, bounds)
        if (bounds.outWidth <= 0) return@withContext null
        var sample = 1
        while (maxOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >= TARGET) sample *= 2
        val bitmap = BitmapFactory.decodeFile(path, BitmapFactory.Options().apply { inSampleSize = sample })
            ?: return@withContext null
        bitmap.asImageBitmap().also { cache.put(path, it) }
    }
}

/**
 * Every photo imported, newest first, as a grid of square thumbnails (the
 * desktop's contact sheet: square corners, no file names). Tapping one
 * opens it; the photo being edited is outlined.
 */
@Composable
fun LibraryScreen(
    items: List<LibraryItem>,
    current: String?,
    importing: Boolean,
    onOpen: (LibraryItem) -> Unit,
    onImport: () -> Unit,
) {
    Column(Modifier.fillMaxSize().background(Colours.Chrome)) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .statusBarsPadding()
                .height(Space.TopBar)
                .padding(start = Space.Gutter, end = Space.XS),
        ) {
            Column(Modifier.weight(1f)) {
                Text("Library", color = Colours.TextStrong, fontSize = TextSize.Title, fontWeight = FontWeight.SemiBold)
                if (items.isNotEmpty()) {
                    Text(
                        if (items.size == 1) "1 photo" else "${items.size} photos",
                        color = Colours.TextMuted,
                        fontSize = TextSize.Caption,
                    )
                }
            }
            if (importing) {
                Box(Modifier.size(Space.Touch), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(Modifier.size(18.dp), color = Colours.Accent, strokeWidth = 2.dp)
                }
            } else {
                IconButton(Icons.Import, "Import Photos", true, onImport, tint = Colours.Accent)
            }
        }
        if (items.isEmpty()) {
            Box(Modifier.fillMaxSize()) {
                Message(
                    icon = Icons.Image,
                    title = "No photos yet",
                    body = "Import RAW files to start editing.",
                    action = "Import Photos…",
                    onAction = onImport,
                    modifier = Modifier.align(Alignment.Center),
                )
            }
            return@Column
        }
        LazyVerticalGrid(
            columns = GridCells.Adaptive(112.dp),
            horizontalArrangement = Arrangement.spacedBy(GridGap),
            verticalArrangement = Arrangement.spacedBy(GridGap),
            contentPadding = WindowInsets.navigationBars.asPaddingValues().let { PaddingValues(bottom = it.calculateBottomPadding()) },
            modifier = Modifier.fillMaxSize(),
        ) {
            items(items, key = { it.id }) { item ->
                Thumbnail(item, selected = item.path == current, onClick = { onOpen(item) })
            }
        }
    }
}

private val GridGap = 2.dp

@Composable
private fun Thumbnail(item: LibraryItem, selected: Boolean, onClick: () -> Unit) {
    val bitmap by produceState(Thumbnails.cached(item.thumb), item.thumb) {
        if (value == null && item.thumb.isNotEmpty()) value = Thumbnails.load(item.thumb)
    }
    Box(
        Modifier
            .aspectRatio(1f)
            .background(Colours.Surface)
            .clickable(onClickLabel = item.name, onClick = onClick),
    ) {
        bitmap?.let {
            Image(it, contentDescription = item.name, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
        }
        if (selected) {
            Box(Modifier.fillMaxSize().border(2.dp, Colours.Accent))
        }
    }
}
