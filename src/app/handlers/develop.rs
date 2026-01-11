use iced::Task;
use crate::app::state::{RawEditor, DragMode};
use crate::app::message::Message;
use crate::ui::preview_renderer::CropHandle;

pub fn handle_exposure_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.exposure = v;
    update_pipeline(editor);
    Task::none()
}

pub fn handle_contrast_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.contrast = v;
    update_pipeline(editor);
    Task::none()
}

pub fn handle_highlights_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.highlights = v;
    update_pipeline(editor);
    Task::none()
}

pub fn handle_shadows_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.shadows = v;
    update_pipeline(editor);
    Task::none()
}

pub fn handle_whites_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.whites = v;
    update_pipeline(editor);
    Task::none()
}

pub fn handle_blacks_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.blacks = v;
    update_pipeline(editor);
    Task::none()
}

pub fn handle_black_offset_changed(editor: &mut RawEditor, i: usize, v: f32) -> Task<Message> {
    if i < 4 {
        editor.current_edit_params.black_offsets[i] = v;
        update_pipeline(editor);
    }
    Task::none()
}

pub fn handle_black_phase_changed(editor: &mut RawEditor, y: bool, v: u32) -> Task<Message> {
    if y {
        editor.current_edit_params.black_phase_y = v;
    } else {
        editor.current_edit_params.black_phase_x = v;
    }
    update_pipeline(editor);
    Task::none()
}

pub fn handle_vibrance_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.vibrance = v;
    update_pipeline(editor);
    Task::none()
}

pub fn handle_saturation_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.saturation = v;
    update_pipeline(editor);
    Task::none()
}

pub fn handle_temperature_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.temperature = v;
    update_pipeline(editor);
    Task::none()
}

pub fn handle_tint_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.tint = v;
    update_pipeline(editor);
    Task::none()
}

pub fn handle_noise_reduction_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.noise_reduction = v;
    update_pipeline(editor);
    Task::none()
}

pub fn handle_sharpening_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.sharpening = v;
    update_pipeline(editor);
    Task::none()
}

pub fn handle_sharpen_masking_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.sharpen_masking = v;
    update_pipeline(editor);
    Task::none()
}

pub fn handle_rotation_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    editor.current_edit_params.rotation = v;
    update_pipeline(editor);
    Task::none()
}

pub fn handle_set_crop(editor: &mut RawEditor, crop: [f32; 4]) -> Task<Message> {
    editor.current_edit_params.crop = crop;
    update_pipeline(editor);
    editor.commit_current_state();
    Task::none()
}

pub fn handle_toggle_crop(editor: &mut RawEditor) -> Task<Message> {
    editor.is_cropping = !editor.is_cropping;
    if editor.is_cropping {
        editor.drag_mode = DragMode::Crop;
    } else {
        editor.drag_mode = DragMode::None;
    }
    Task::none()
}

pub fn handle_crop_handle_grabbed(editor: &mut RawEditor, handle: CropHandle, bounds: iced::Rectangle) -> Task<Message> {
    editor.drag_mode = DragMode::CropHandle(handle);
    editor.is_dragging = true;
    editor.viewport_size = (bounds.width, bounds.height);
    Task::none()
}

pub fn handle_commit_edit(editor: &mut RawEditor) -> Task<Message> {
    editor.commit_current_state();
    Task::none()
}

pub fn handle_undo(editor: &mut RawEditor) -> Task<Message> {
    if let Some((stack, index)) = editor.get_current_history() {
        if *index > 0 {
            *index -= 1;
            editor.current_edit_params = stack[*index].clone();
            update_pipeline(editor);
        }
    }
    Task::none()
}

pub fn handle_redo(editor: &mut RawEditor) -> Task<Message> {
    if let Some((stack, index)) = editor.get_current_history() {
        if *index < stack.len() - 1 {
            *index += 1;
            editor.current_edit_params = stack[*index].clone();
            update_pipeline(editor);
        }
    }
    Task::none()
}

pub fn handle_reset_edits(editor: &mut RawEditor) -> Task<Message> {
    editor.current_edit_params.reset();
    if let Some(lib) = &editor.library { 
        if let Some(id) = editor.selected_image_id { 
            let _ = lib.delete_edits(id); 
        } 
    }
    update_pipeline(editor);
    editor.commit_current_state();
    Task::none()
}

pub fn handle_copy_settings(editor: &mut RawEditor) -> Task<Message> {
    editor.edit_clipboard = Some(editor.current_edit_params);
    editor.status = "Settings copied!".to_string();
    Task::none()
}

pub fn handle_paste_settings(editor: &mut RawEditor) -> Task<Message> {
    if let Some(cb) = editor.edit_clipboard {
        editor.current_edit_params = cb;
        if let Some(lib) = &editor.library {
            for &id in &editor.multi_selection {
                if Some(id) != editor.selected_image_id { 
                    let _ = lib.save_edit_params(id, &editor.current_edit_params); 
                }
            }
            editor.status = "Settings pasted to selection".to_string();
        }
        update_pipeline(editor);
        editor.commit_current_state();
    }
    Task::none()
}

// Helper
fn update_pipeline(editor: &mut RawEditor) {
    editor.save_current_edits();
    if let (Some(ctx), Some(resources)) = (&editor.gpu_context, &editor.image_resources) {
        resources.update_uniforms(ctx, &editor.current_edit_params);
        editor.canvas_cache.clear();
    }
}
