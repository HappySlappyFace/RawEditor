# Phase 23: Splash Screen Setup

## Adding Your Custom Splash Image

### Step 1: Create the assets folder
```bash
mkdir -p assets
```

### Step 2: Add your splash image
Place your image in the `assets` folder:
- `assets/splash.png` (recommended: 400x400px or larger)
- Or `assets/splash.jpg`

### Step 3: Enable the image in code
In `src/main.rs`, find the `view_splash()` function and:

1. **Comment out** the emoji line:
```rust
// text("📸").size(120).center(),
```

2. **Uncomment** the image widget:
```rust
iced::widget::image("assets/splash.png")
    .width(400)
    .height(400),
```

## Current Splash Screen Features

✅ **Borderless window** - No title bar during splash (Adobe-style)
✅ **Centered on screen** - Opens in the center
✅ **900x600 window size** - Optimized for splash
✅ **Dark theme** - Professional appearance
✅ **Instant loading** - Shows immediately, database loads in background

## Customization Options

### Change window size
In `main()` function:
```rust
.window(iced::window::Settings {
    size: iced::Size::new(1200.0, 700.0),  // Custom size
    decorations: false,
    ..Default::default()
})
```

### Add title bar back after loading
*Note: This requires more advanced iced window management*
Currently the app stays borderless throughout.

### Adjust colors
In `view_splash()`, modify the RGB values:
```rust
background: Some(Background::Color(Color::from_rgb(0.08, 0.08, 0.10))),
```

## Recommended Splash Image Specs

- **Format**: PNG (with transparency) or JPG
- **Size**: 400x400px to 600x600px
- **Style**: Logo, brand mark, or hero image
- **Colors**: Works best with dark theme (current bg: #141418)
