# RAW Editor

A blazing-fast, native, cross-platform RAW photo editor built in Rust.

## Vision

Replace Lightroom Classic with a modern, performant alternative that achieves:
- **Designed with performance as the highest priority**
- **Zero web technologies** (native all the way)
- **High-throughput batch processing**
- **Cross-platform support** (Windows, Linux, macOS)

## Tech Stack

- **Rust** - Systems programming language for maximum performance
- **iced** - Native GUI framework with Elm architecture
- **wgpu** - Cross-platform GPU API for compute and rendering
- **rusqlite** - Embedded SQLite for catalog management
- **rawloader** - RAW image decoding library

## Architecture

```
src/
├─ state/   # Database, edit stack, job queue
├─ ui/      # All iced widgets and layouts
├─ gpu/     # wgpu pipelines, shaders, caching
├─ raw/     # RAW decoding abstraction
├─ color/   # Color science, profile management
└─ main.rs  # Application entrypoint
```

## Current Status

Currently the app is capable of importing RAW (.NEF as of now) images, extracting thumbnails or generating them, storing references to the sqlite3 database, and the Develop tab has a 10-slider creative engine that
is gpu accelerated (Vulkan using WGPU) to process all the sliders.
Currently I managed to get 60fps editing workflow on a Ryzen 3 5425U APU.
And I am testing this software Nikon RAW currently as that's my primary shooting brand, specifically D3300
but color science is still not correctly implemented.
Currently the workflow pipeline is still scuffed but that will be resolved in later versions.

## Features Implemented

### Core Infrastructure
- SQLite database for image catalog and edit storage
- Non-destructive editing with full edit history persistence
- RAW image decoding (Nikon NEF format tested on D3300)
- Thumbnail extraction from embedded JPEG previews
- Thumbnail generation for images without embedded previews
- Cross-platform GPU acceleration via wgpu (Vulkan/Metal/DirectX 12)

### Library Module
- Grid-based thumbnail browser
- Image import and cataloging
- Quick image selection and navigation

### Develop Module
- Real-time GPU-accelerated RAW processing pipeline
- Live histogram display
- 60fps editing workflow on mid-range hardware (tested on Ryzen 3 5425U APU)

### Tone Adjustments
- Exposure compensation (-5 to +5 stops)
- Contrast adjustment
- Highlights recovery
- Shadows lift/recovery
- Whites (white point control)
- Blacks (black point control)

### Color Adjustments
- Relative White balance (Temperature and Tint)
- Basic color matrix application
- Saturation (global color intensity)
- Vibrance (smart saturation with skin tone protection)

### Viewing & Navigation
- Mouse wheel zoom (10% to 1000%)
- Zoom-to-cursor (pixel-perfect stability)
- Click-and-drag panning
- Double-click to reset view
- Before/After comparison toggle (Spacebar)
- Arrow key image navigation
- Reset edits (R key)

### Performance Features
- GPU shader-based transformations
- Smart render caching
- Viewport-aware coordinate tracking
- Real-time preview updates

### Known Limitations
- Color science implementation incomplete (accurate color rendering in progress)
- Single RAW format tested (Nikon NEF)
- Workflow pipeline still being refined

## Building

```bash
# Development build
cargo build

# Run the application
cargo run

# Release build (optimized)
cargo build --release
cargo run --release
```

## Requirements

- Rust 1.70+ (2021 edition)
- GPU with Vulkan/Metal/DX12 support

## License

MIT

## Todo
Hard coded 1280px previews for the develop panel, add a setting with more resolution options
Implement iced's Multi-Window API instead of Single-Window API