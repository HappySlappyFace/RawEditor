use iced::Task;
use std::sync::Arc;
use crate::app::state::{RawEditor, EditorReadiness};
use crate::app::message::Message;
use crate::raw;
use crate::gpu;

pub fn handle_raw_data_loaded(editor: &mut RawEditor, result: Result<raw::loader::RawDataResult, String>) -> Task<Message> {
    match result {
        Ok(raw) => {
            editor.current_metadata = Some(raw.clone());
            let image_id = editor.selected_image_id.unwrap_or(0);
            let params = editor.current_edit_params.clone();
            
            let xyz_to_cam = raw.color_matrix;
            let cam_to_srgb = crate::color::calculate_cam_to_srgb(xyz_to_cam);
            
            let context = editor.gpu_context.clone();
            
            Task::perform(
                async move {
                    let ctx = if let Some(c) = context {
                        c
                    } else {
                        match gpu::shared::SharedContext::new().await {
                            Ok(c) => Arc::new(c),
                            Err(e) => return Err(e),
                        }
                    };
                    
                    match gpu::shared::ImageResources::new(
                        &ctx,
                        image_id,
                        raw.data,
                        raw.width,
                        raw.height,
                        &params,
                        raw.wb_multipliers,
                        cam_to_srgb,
                        raw.cfa_pattern,
                        raw.black_levels,
                        raw.white_level
                    ) {
                        Ok(resources) => Ok((ctx, std::sync::Arc::new(resources))),
                        Err(e) => Err(e),
                    }
                },
                move |res| Message::ImageResourcesReady(image_id, res)
            )
        }
        Err(e) => { 
            editor.status = format!("Failed to load RAW: {}", e); 
            editor.editor_readiness = EditorReadiness::Failed(0, e); 
            Task::none()
        }
    }
}

pub fn handle_image_resources_ready(editor: &mut RawEditor, image_id: i64, result: Result<(Arc<gpu::shared::SharedContext>, Arc<gpu::shared::ImageResources>), String>) -> Task<Message> {
    match result {
        Ok((context, resources)) => {
            if editor.gpu_context.is_none() {
                editor.gpu_context = Some(context.clone());
            }
            
            if let Some(ctx) = &editor.gpu_context {
                resources.update_uniforms(ctx, &editor.current_edit_params);
            }
            
            editor.image_resources = Some(resources);
            editor.editor_readiness = EditorReadiness::Ready(image_id);
            editor.working_preview = None;
            editor.canvas_cache.clear();
            editor.histogram_cache.clear();

            // Kick off the first async render now that resources are ready
            editor.is_rendering_preview = true;
            crate::app::handlers::develop::trigger_async_render(editor)
        }
        Err(e) => { 
            editor.status = format!("GPU Init Failed: {}", e); 
            editor.editor_readiness = EditorReadiness::Failed(image_id, e); 
            Task::none()
        }
    }
}
