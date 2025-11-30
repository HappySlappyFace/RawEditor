use iced::widget::canvas::{self, Program};
use iced::mouse::Cursor;
use iced::{Rectangle, Renderer, Theme, Point, Size, Color};

use crate::Message;

/// Canvas-based JPEG preview renderer (Phase 32)
/// 
/// This renderer displays JPEG previews using Canvas instead of Image widget.
/// It uses the SAME coordinate system as GpuRenderer, ensuring zoom/pan state
/// transfers seamlessly when transitioning from preview to RAW.
pub struct PreviewRenderer {
    /// The JPEG image handle to display
    pub handle: iced::widget::image::Handle,
    /// Current zoom level (1.0 = 100%, 2.0 = 200%, etc.)
    pub zoom: f32,
    /// Pan offset in normalized coordinates
    pub offset: cgmath::Vector2<f32>,
    /// Phase 67: Interactive crop mode
    pub is_cropping: bool,
    pub crop: [f32; 4],
    pub image_width: u32,
    pub image_height: u32,
    /// Phase 67: Control rendering layers
    pub draw_image: bool,
}

/// Phase 67: Crop handles
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CropHandle {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    /// Phase 70: Dragging the crop area itself
    Body,
}

impl Program<Message> for PreviewRenderer {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        // The bounds give us the viewport size (canvas container size)
        let viewport_width = bounds.width;
        let viewport_height = bounds.height;

        // Use actual image aspect ratio
        let image_aspect = self.image_width as f32 / self.image_height as f32;
        let viewport_aspect = viewport_width / viewport_height;

        // Calculate fitted size (contain mode - image fits entirely within viewport)
        let (fitted_width, fitted_height) = if image_aspect > viewport_aspect {
            // Image is wider - fit to width
            let w = viewport_width;
            let h = w / image_aspect;
            (w, h)
        } else {
            // Image is taller - fit to height
            let h = viewport_height;
            let w = h * image_aspect;
            (w, h)
        };

        // Start with the image centered in the viewport
        let center_x = viewport_width / 2.0;
        let center_y = viewport_height / 2.0;

        // Apply zoom to the fitted size
        let zoomed_width = fitted_width * self.zoom;
        let zoomed_height = fitted_height * self.zoom;

        // Apply pan offset
        // CRITICAL: Pan offset must account for zoom!
        // GPU shader applies pan in texture space AFTER zoom division
        // In screen space, same texture pan = less screen movement when zoomed
        // So we divide by zoom to match the shader's coordinate system
        let pan_x = (self.offset.x * fitted_width) / self.zoom;
        let pan_y = (self.offset.y * fitted_height) / self.zoom;
        
        // Calculate final image bounds
        let image_x = center_x - (zoomed_width / 2.0) + pan_x;
        let image_y = center_y - (zoomed_height / 2.0) + pan_y;

        let image_bounds = Rectangle {
            x: image_x,
            y: image_y,
            width: zoomed_width,
            height: zoomed_height,
        };

        let mut geometries = Vec::new();

        if self.draw_image {
            let mut image_frame = canvas::Frame::new(renderer, bounds.size());
            
            // Draw the image
            let canvas_image = iced::widget::canvas::Image::new(self.handle.clone());
            image_frame.draw_image(image_bounds, canvas_image);
            
            geometries.push(image_frame.into_geometry());
        }

        // Phase 67: Draw Crop Overlay
        if self.is_cropping {
            let mut overlay_frame = canvas::Frame::new(renderer, bounds.size());
            
            // Calculate crop rect in screen coordinates
            // crop is [x, y, w, h] in normalized image coordinates
            
            let crop_x = image_bounds.x + (self.crop[0] * image_bounds.width);
            let crop_y = image_bounds.y + (self.crop[1] * image_bounds.height);
            let crop_w = self.crop[2] * image_bounds.width;
            let crop_h = self.crop[3] * image_bounds.height;

            // Draw Dimming Overlay (outside crop area)
            let dim_color = Color::from_rgba(0.0, 0.0, 0.0, 0.7); // Dark overlay
            
            // Top rect
            overlay_frame.fill_rectangle(
                Point::new(image_bounds.x, image_bounds.y),
                Size::new(image_bounds.width, crop_y - image_bounds.y),
                dim_color,
            );
            // Bottom rect
            overlay_frame.fill_rectangle(
                Point::new(image_bounds.x, crop_y + crop_h),
                Size::new(image_bounds.width, (image_bounds.y + image_bounds.height) - (crop_y + crop_h)),
                dim_color,
            );
            // Left rect
            overlay_frame.fill_rectangle(
                Point::new(image_bounds.x, crop_y),
                Size::new(crop_x - image_bounds.x, crop_h),
                dim_color,
            );
            // Right rect
            overlay_frame.fill_rectangle(
                Point::new(crop_x + crop_w, crop_y),
                Size::new((image_bounds.x + image_bounds.width) - (crop_x + crop_w), crop_h),
                dim_color,
            );

            // Draw Rule of Thirds grid
            let third_w = crop_w / 3.0;
            let third_h = crop_h / 3.0;
            
            let grid_stroke = canvas::Stroke::default()
                .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.8)) // Increased opacity
                .with_width(1.0);
                
            // Vertical lines
            overlay_frame.stroke(
                &canvas::Path::line(
                    Point::new(crop_x + third_w, crop_y),
                    Point::new(crop_x + third_w, crop_y + crop_h)
                ),
                grid_stroke.clone(),
            );
            overlay_frame.stroke(
                &canvas::Path::line(
                    Point::new(crop_x + third_w * 2.0, crop_y),
                    Point::new(crop_x + third_w * 2.0, crop_y + crop_h)
                ),
                grid_stroke.clone(),
            );
            
            // Horizontal lines
            overlay_frame.stroke(
                &canvas::Path::line(
                    Point::new(crop_x, crop_y + third_h),
                    Point::new(crop_x + crop_w, crop_y + third_h)
                ),
                grid_stroke.clone(),
            );
            overlay_frame.stroke(
                &canvas::Path::line(
                    Point::new(crop_x, crop_y + third_h * 2.0),
                    Point::new(crop_x + crop_w, crop_y + third_h * 2.0)
                ),
                grid_stroke.clone(),
            );
            
            // Draw white border
            let border_rect = Rectangle {
                x: crop_x,
                y: crop_y,
                width: crop_w,
                height: crop_h,
            };
            overlay_frame.stroke(
                &canvas::Path::rectangle(border_rect.position(), border_rect.size()),
                canvas::Stroke::default().with_color(Color::WHITE).with_width(2.0),
            );
            
            // Draw corner handles
            let handle_size = 12.0; // Slightly larger
            let handle_color = Color::WHITE;
            let handle_stroke = canvas::Stroke::default().with_color(Color::BLACK).with_width(1.0);
            
            let draw_handle = |frame: &mut canvas::Frame, x: f32, y: f32| {
                let pos = Point::new(x - handle_size/2.0, y - handle_size/2.0);
                let size = Size::new(handle_size, handle_size);
                // Fill
                frame.fill_rectangle(pos, size, handle_color);
                // Stroke (outline for visibility)
                frame.stroke(
                    &canvas::Path::rectangle(pos, size),
                    handle_stroke.clone(),
                );
            };
            
            // Top-Left
            draw_handle(&mut overlay_frame, crop_x, crop_y);
            // Top-Right
            draw_handle(&mut overlay_frame, crop_x + crop_w, crop_y);
            // Bottom-Left
            draw_handle(&mut overlay_frame, crop_x, crop_y + crop_h);
            // Bottom-Right
            draw_handle(&mut overlay_frame, crop_x + crop_w, crop_y + crop_h);
            
            geometries.push(overlay_frame.into_geometry());
        }

        geometries
    }

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        if !self.is_cropping {
            return (canvas::event::Status::Ignored, None);
        }

        let cursor_position = if let Some(pos) = cursor.position_in(bounds) {
            pos
        } else {
            return (canvas::event::Status::Ignored, None);
        };

        match event {
            canvas::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)) => {
                // Calculate image bounds (same logic as draw)
                let viewport_width = bounds.width;
                let viewport_height = bounds.height;
                let image_aspect = self.image_width as f32 / self.image_height as f32;
                let viewport_aspect = viewport_width / viewport_height;

                let (fitted_width, fitted_height) = if image_aspect > viewport_aspect {
                    let w = viewport_width;
                    let h = w / image_aspect;
                    (w, h)
                } else {
                    let h = viewport_height;
                    let w = h * image_aspect;
                    (w, h)
                };

                let center_x = viewport_width / 2.0;
                let center_y = viewport_height / 2.0;
                let zoomed_width = fitted_width * self.zoom;
                let zoomed_height = fitted_height * self.zoom;
                let pan_x = (self.offset.x * fitted_width) / self.zoom;
                let pan_y = (self.offset.y * fitted_height) / self.zoom;
                
                let image_x = center_x - (zoomed_width / 2.0) + pan_x;
                let image_y = center_y - (zoomed_height / 2.0) + pan_y;
                
                let image_bounds = Rectangle {
                    x: image_x,
                    y: image_y,
                    width: zoomed_width,
                    height: zoomed_height,
                };

                // Calculate handle positions
                let crop_x = image_bounds.x + (self.crop[0] * image_bounds.width);
                let crop_y = image_bounds.y + (self.crop[1] * image_bounds.height);
                let crop_w = self.crop[2] * image_bounds.width;
                let crop_h = self.crop[3] * image_bounds.height;
                
                let handle_radius = 15.0;
                let check_handle = |x, y| {
                    let dx = cursor_position.x - x;
                    let dy = cursor_position.y - y;
                    (dx*dx + dy*dy) < handle_radius * handle_radius
                };
                
                if check_handle(crop_x, crop_y) {
                    return (canvas::event::Status::Captured, Some(Message::CropHandleGrabbed(CropHandle::TopLeft, image_bounds)));
                }
                if check_handle(crop_x + crop_w, crop_y) {
                    return (canvas::event::Status::Captured, Some(Message::CropHandleGrabbed(CropHandle::TopRight, image_bounds)));
                }
                if check_handle(crop_x, crop_y + crop_h) {
                    return (canvas::event::Status::Captured, Some(Message::CropHandleGrabbed(CropHandle::BottomLeft, image_bounds)));
                }
                if check_handle(crop_x + crop_w, crop_y + crop_h) {
                    return (canvas::event::Status::Captured, Some(Message::CropHandleGrabbed(CropHandle::BottomRight, image_bounds)));
                }
                
                // Phase 70: Check if inside body
                if cursor_position.x >= crop_x && cursor_position.x <= crop_x + crop_w &&
                   cursor_position.y >= crop_y && cursor_position.y <= crop_y + crop_h {
                    return (canvas::event::Status::Captured, Some(Message::CropHandleGrabbed(CropHandle::Body, image_bounds)));
                }
            }
            _ => {}
        }

        (canvas::event::Status::Ignored, None)
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> iced::mouse::Interaction {
        if !self.is_cropping {
            return iced::mouse::Interaction::default();
        }

        let cursor_position = match cursor.position_in(bounds) {
            Some(p) => p,
            None => return iced::mouse::Interaction::default(),
        };

        // Re-calculate bounds (duplicate logic, but necessary for stateless Program)
        // In a real app, we might cache this layout, but for now it's cheap enough
        let viewport_width = bounds.width;
        let viewport_height = bounds.height;
        let image_aspect = self.image_width as f32 / self.image_height as f32;
        let viewport_aspect = viewport_width / viewport_height;

        let (fitted_width, fitted_height) = if image_aspect > viewport_aspect {
            let w = viewport_width;
            let h = w / image_aspect;
            (w, h)
        } else {
            let h = viewport_height;
            let w = h * image_aspect;
            (w, h)
        };

        let center_x = viewport_width / 2.0;
        let center_y = viewport_height / 2.0;
        let zoomed_width = fitted_width * self.zoom;
        let zoomed_height = fitted_height * self.zoom;
        let pan_x = (self.offset.x * fitted_width) / self.zoom;
        let pan_y = (self.offset.y * fitted_height) / self.zoom;
        
        let image_x = center_x - (zoomed_width / 2.0) + pan_x;
        let image_y = center_y - (zoomed_height / 2.0) + pan_y;
        
        let image_bounds = Rectangle {
            x: image_x,
            y: image_y,
            width: zoomed_width,
            height: zoomed_height,
        };

        let crop_x = image_bounds.x + (self.crop[0] * image_bounds.width);
        let crop_y = image_bounds.y + (self.crop[1] * image_bounds.height);
        let crop_w = self.crop[2] * image_bounds.width;
        let crop_h = self.crop[3] * image_bounds.height;
        
        let handle_radius = 15.0;
        let check_handle = |x, y| {
            let dx = cursor_position.x - x;
            let dy = cursor_position.y - y;
            (dx*dx + dy*dy) < handle_radius * handle_radius
        };
        
        if check_handle(crop_x, crop_y) { return iced::mouse::Interaction::ResizingDiagonallyUp; } // TopLeft (NW)
        if check_handle(crop_x + crop_w, crop_y) { return iced::mouse::Interaction::ResizingDiagonallyDown; } // TopRight (NE) - Wait, NE is Up?
        // Let's check standard cursors.
        // NWSE = ResizingDiagonallyUp (/) or Down (\)?
        // Usually:
        // NW-SE (\) = ResizingDiagonallyDown (if defined as top-left to bottom-right)
        // NE-SW (/) = ResizingDiagonallyUp
        
        // Iced 0.13:
        // ResizingDiagonallyUp = / (NE-SW)
        // ResizingDiagonallyDown = \ (NW-SE)
        
        if check_handle(crop_x, crop_y) { return iced::mouse::Interaction::ResizingDiagonallyDown; } // TopLeft (NW) -> SE
        if check_handle(crop_x + crop_w, crop_y) { return iced::mouse::Interaction::ResizingDiagonallyUp; } // TopRight (NE) -> SW
        if check_handle(crop_x, crop_y + crop_h) { return iced::mouse::Interaction::ResizingDiagonallyUp; } // BottomLeft (SW) -> NE
        if check_handle(crop_x + crop_w, crop_y + crop_h) { return iced::mouse::Interaction::ResizingDiagonallyDown; } // BottomRight (SE) -> NW
        
        // Check body
        if cursor_position.x >= crop_x && cursor_position.x <= crop_x + crop_w &&
           cursor_position.y >= crop_y && cursor_position.y <= crop_y + crop_h {
            return iced::mouse::Interaction::Grab;
        }

        iced::mouse::Interaction::default()
    }
}
