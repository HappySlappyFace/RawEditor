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

✅ **Auto-maximize** - Window maximizes after database loads
✅ **Centered on screen** - Opens in the center
✅ **1280x800 startup size** - Comfortable initial window size
✅ **Dark theme** - Professional appearance
✅ **Instant loading** - Shows immediately, database loads in background
✅ **Image fitting** - Two modes: Contain (no crop) or Cover (full bleed)

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

## Image Transparency & Blending

### For smooth blending with background:

1. **Use PNG with alpha channel**
   - Iced natively supports PNG transparency
   - Image will blend with dark background (#141418)

2. **Add gradient alpha in your editor** (Photoshop/GIMP/Figma)
   - Create a radial gradient mask
   - Center: 100% opacity
   - Edges: Fade to 0% opacity
   - Result: Image blends smoothly into background

3. **Choose image fit mode:**
   ```rust
   // Maintain aspect ratio, no cropping
   .content_fit(iced::ContentFit::Contain)
   
   // Fill entire space, may crop edges
   .content_fit(iced::ContentFit::Cover)
   ```

## Recommended Splash Image Specs

- **Format**: PNG (with alpha channel for transparency)
- **Size**: 800x800px or larger (will scale to fit)
- **Style**: Logo, brand mark, or hero image
- **Colors**: Works best with dark theme (current bg: #141418)
- **Alpha**: Add gradient fade at edges for smooth blending
