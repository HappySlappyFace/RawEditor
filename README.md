# RAW Editor

A blazing-fast, native, cross-platform RAW photo editor built in Rust.

## Vision

Replace Lightroom Classic with a modern, performant alternative that achieves:
- **240+ fps UI rendering**
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

**Phase 1**: Architecture & Environment Setup ✅

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
