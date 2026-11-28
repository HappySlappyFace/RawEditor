//! Export, the app's side: the dialog, the folder picker and the settings
//! fields. The batch itself is the session's (`session::export`).

use crate::app::message::Message;
use crate::app::state::{Modal, RawEditor};
use crate::app::update::run;
use crate::session::ExportFormat;
use iced::Task;
use std::path::{Path, PathBuf};

pub fn handle_set_export_format(editor: &mut RawEditor, format: ExportFormat) -> Task<Message> {
    editor.session.export_settings.format = format;
    Task::none()
}

pub fn handle_set_export_color_space(
    editor: &mut RawEditor,
    space: crate::core::output_space::OutputSpace,
) -> Task<Message> {
    editor.session.export_settings.color_space = space;
    Task::none()
}

pub fn handle_set_export_quality(editor: &mut RawEditor, quality: u8) -> Task<Message> {
    editor.session.export_settings.quality = quality;
    Task::none()
}

pub fn handle_toggle_export_resize(editor: &mut RawEditor, resize: bool) -> Task<Message> {
    editor.session.export_settings.resize = resize;
    Task::none()
}

pub fn handle_toggle_export_ultra_hdr(editor: &mut RawEditor, on: bool) -> Task<Message> {
    editor.session.export_settings.ultra_hdr = on;
    Task::none()
}

pub fn handle_set_export_width(editor: &mut RawEditor, text: String) -> Task<Message> {
    editor.session.export_settings.set_max_width_text(text);
    Task::none()
}

pub fn handle_set_export_subfolder(editor: &mut RawEditor, subfolder: String) -> Task<Message> {
    editor.session.export_settings.subfolder = subfolder;
    Task::none()
}

pub fn handle_pick_export_base_path(editor: &mut RawEditor) -> Task<Message> {
    let current = editor.session.export_settings.base_path.clone();
    Task::perform(
        async move {
            rfd::AsyncFileDialog::new()
                .set_directory(current)
                .pick_folder()
                .await
                .map(|handle| handle.path().to_path_buf())
        },
        |opt| {
            if let Some(path) = opt {
                Message::SetExportBasePath(path)
            } else {
                Message::ModalNoOp
            }
        },
    )
}

pub fn handle_set_export_base_path(editor: &mut RawEditor, path: PathBuf) -> Task<Message> {
    if !path.as_os_str().is_empty() {
        editor.session.export_settings.base_path = path;
    }
    Task::none()
}

pub fn handle_open_export_modal(editor: &mut RawEditor) -> Task<Message> {
    editor.active_modal = Modal::Export;

    let default_dir = dirs::picture_dir().unwrap_or_else(|| PathBuf::from("."));
    if editor.session.export_settings.base_path == default_dir {
        if let Some(id) = editor.session.selected_image_id {
            if let Some(img) = editor.session.images.iter().find(|i| i.id == id) {
                if let Some(parent) = Path::new(&img.path).parent() {
                    editor.session.export_settings.base_path = parent.to_path_buf();
                }
            }
        }
    }

    Task::none()
}

pub fn handle_export_confirmed(editor: &mut RawEditor) -> Task<Message> {
    let Some(effects) = editor.session.start_export() else {
        return Task::none();
    };
    editor.active_modal = Modal::None;
    run(effects)
}

/// Cancel after the in-flight image finishes. The running GPU render is not
/// interruptible, so this drains the pending work rather than killing a task
/// mid-readback.
pub fn handle_cancel_export(editor: &mut RawEditor) -> Task<Message> {
    editor.session.cancel_export();
    Task::none()
}
