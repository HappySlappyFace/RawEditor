# RAW Editor
![Project Logo](./assets/logo.png)

**A blazing-fast, native, no-nonsense RAW photo editor for photographers who value their time.**

Built from the ground up in **Rust**, RAW Editor is designed to be the lightweight, high-performance alternative to bloated legacy software. No web tech, no electron, no subscriptions—just raw system performance and clean code.

---

## 📸 Why RAW Editor?

Most editors feel heavy because they are. We prioritized **GPU-native rendering** and **asynchronous pipelines** to ensure that your editing workflow remains fluid even on modest hardware.

- **Zero Lag Editing**: 60fps slider response on mid-range APUs.
- **Native Power**: Built with `iced` and `wgpu` for direct access to your GPU (Vulkan, Metal, DX12).
- **Pro Color Science**: Phase 128 "ACES" filmic tone mapping and scene-referred processing.
- **Pixel-Perfect**: Viewport-aligned rendering with full DPI scaling support (Phase 134/135).

---

## 🛠 Features

### 🌈 The Develop Module (The Creative Engine)
The heart of the app. A real-time, non-destructive pipeline that processes RAW sensor data directly on your GPU.

> **[PLACEHOLDER: Add a screenshot of the Develop Tab here - ./docs/screenshots/develop_view.png]**

- **Exposure & Tone**: -5 to +5 stops exposure, highlights recovery, and shadow lifting.
- **Color Mastery**: Advanced White Balance (Temp/Tint) and Vibrance/Saturation.
- **Non-Destructive Crop (New!)**: Toggle crop mode to see your full image dimmed while you adjust your composition visually.
- **High-Fidelity Previews**: Razor-sharp bilinear downscaling from full 24MP+ sensor data to your specific screen resolution.

### 🚀 Performance HUD (Press F3)
Ever wonder how hard your GPU is working? Our built-in profiler (Phase 104) gives you a frame-by-frame breakdown of render times, upload latency, and pipeline health.

> **[PLACEHOLDER: Add a screenshot of the F3 Framegraph HUD here - ./docs/screenshots/performance_hud.png]**

### 📦 Library & Cataloging
Blazing fast image ingestion and metadata management.
- **SQLite Powered**: Industrial-grade catalog management.
- **Multi-Tier Caching**: Thumbnails, instant previews, and working buffers managed automatically.
- **Smart Navigation**: Arrow key browsing, zoom-to-cursor, and pixel-perfect panning.

---

## ⚡ Tech Stack

- **Rust**: The backbone for safety and speed.
- **wgpu**: Cutting-edge GPU abstraction for compute and fragment shaders.
- **iced**: Native, elm-inspired GUI for a responsive interface.
- **rawloader**: Low-level RAW decoding (Phase 135 optimized).
- **rusqlite**: Lightning-fast local database.

---

## 🚀 Getting Started

### Prerequisites
- **Rust 1.75+**
- A GPU with Vulkan, Metal, or DirectX 12 support.

### Installation
```bash
# Clone the repository
git clone https://github.com/your-repo/raw-editor.git
cd raw-editor

# Run in release mode for the best experience
cargo run --release
```

---

## ⌨️ Shortcuts for Power Users

- `Space`: Toggle Before/After comparison.
- `F3`: Toggle Performance HUD.
- `R`: Reset all edits on current image.
- `C`: (Planned) Quick Crop toggle.
- `Double Click`: Reset zoom/pan.

---

## 🗺 Roadmap
- [x] Phase 128: Color Science Revamp (ACES/Display-Referred).
- [x] Phase 134: Viewport & DPI Alignment.
- [x] Phase 135: Non-Destructive Crop & Sharp Routing.
- [ ] Multi-Window support via iced.
- [ ] Export Studio Batch Processing.
- [ ] Support for Sony ARW / Canon CR3.

---

## 📄 License
MIT - Created by Ayman REBAI. Feel free to contribute or fork!