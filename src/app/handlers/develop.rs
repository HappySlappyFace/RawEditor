//! Develop, the app's side: modal guards, the copy dialog, the slider text
//! field, folded sections and pointer state. Editing, rendering and the
//! tools are the session's (`session::develop`).

use crate::app::message::{Message, SidebarSection};
use crate::app::state::{DragMode, Modal, RawEditor};
use crate::app::update::run;
use crate::core::types::EditParams;
use crate::session::{CopyCategory, CropHandle, EditSection, GtParam};
use iced::Task;

/// A slider's change: the edit, rendered (its release commits it).
fn edit(editor: &mut RawEditor, change: impl FnOnce(&mut EditParams)) -> Task<Message> {
    run(editor.session.edit(change))
}

pub fn handle_exposure_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.exposure = v)
}

pub fn handle_contrast_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.contrast = v)
}

pub fn handle_highlights_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.highlights = v)
}

pub fn handle_shadows_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.shadows = v)
}

pub fn handle_whites_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.whites = v)
}

pub fn handle_blacks_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.blacks = v)
}

pub fn handle_black_offset_changed(editor: &mut RawEditor, i: usize, v: f32) -> Task<Message> {
    if i < 4 {
        edit(editor, |p| p.black_offsets[i] = v)
    } else {
        Task::none()
    }
}

pub fn handle_black_phase_changed(editor: &mut RawEditor, y: bool, v: u32) -> Task<Message> {
    // A checkbox has no release: this click is the whole gesture.
    run(editor.session.edit_and_commit(|p| {
        if y {
            p.black_phase_y = v;
        } else {
            p.black_phase_x = v;
        }
    }))
}

pub fn handle_vibrance_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.vibrance = v)
}

pub fn handle_profile_curve_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.profile_curve = v)
}

pub fn handle_curve_changed(
    editor: &mut RawEditor,
    channel: crate::core::curves::CurveChannel,
    curve: crate::core::curves::Curve,
    commit: bool,
) -> Task<Message> {
    let change = |p: &mut EditParams| *p.curves.get_mut(channel) = curve;
    if commit {
        run(editor.session.edit_and_commit(change))
    } else {
        edit(editor, change)
    }
}

pub fn handle_tone_mapper_changed(editor: &mut RawEditor, mapper: crate::core::tonemap::ToneMapper) -> Task<Message> {
    run(editor.session.set_tone_mapper(mapper))
}

pub fn handle_gt_param_changed(editor: &mut RawEditor, param: GtParam, v: f32) -> Task<Message> {
    run(editor.session.set_gt_param(param, v))
}

pub fn handle_saturation_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.saturation = v)
}

pub fn handle_temperature_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.temperature = v)
}

pub fn handle_tint_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.tint = v)
}

pub fn handle_texture_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.texture = v)
}

pub fn handle_clarity_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.clarity = v)
}

pub fn handle_dehaze_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.dehaze = v)
}

// Noise reduction is held back on the main view while these move, and shown
// live in the detail loupe — see `session::loupe`.
pub fn handle_luma_noise_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    run(editor.session.noise_changed(|p| p.luma_noise = v))
}

pub fn handle_luma_detail_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    run(editor.session.noise_changed(|p| p.luma_detail = v))
}

pub fn handle_color_noise_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    run(editor.session.noise_changed(|p| p.color_noise = v))
}

pub fn handle_sharpening_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.sharpening = v)
}

pub fn handle_sharpen_masking_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.sharpen_masking = v)
}

pub fn handle_rotation_changed(editor: &mut RawEditor, v: f32) -> Task<Message> {
    edit(editor, |p| p.rotation = v)
}

pub fn handle_turn_image(editor: &mut RawEditor, clockwise: bool) -> Task<Message> {
    run(editor.session.turn_image(clockwise))
}

/// A click on a slider's value: a text field takes its place, focused with
/// the value selected, so typing replaces it.
pub fn handle_slider_entry_start(editor: &mut RawEditor, key: String, text: String) -> Task<Message> {
    editor.slider_entry = Some(crate::app::state::SliderEntry { key, text });
    let id = crate::app::views::develop::slider_entry_id();
    Task::batch([
        iced::widget::text_input::focus(id.clone()),
        iced::widget::text_input::select_all(id),
    ])
}

/// Run a tool switch, and follow the crop tool's state with the pointer's:
/// turning crop on makes a drag on the image a crop drag, turning it off
/// (directly, or by picking another tool) ends that.
pub(crate) fn switch_tool(editor: &mut RawEditor, switch: impl FnOnce(&mut crate::session::Session) -> crate::session::Effects) -> Task<Message> {
    let was_cropping = editor.session.is_cropping;
    let effects = switch(&mut editor.session);
    if editor.session.is_cropping != was_cropping {
        editor.drag_mode = if editor.session.is_cropping { DragMode::Crop } else { DragMode::None };
    }
    run(effects)
}

/// Arm/disarm the level tool. Develop-only and exclusive with every other
/// tool, like the WB picker: a drag on the image can only mean one thing.
pub fn handle_toggle_level_tool(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::None {
        return Task::none();
    }
    switch_tool(editor, |s| s.toggle_level_tool())
}

pub fn handle_level_line(editor: &mut RawEditor, dx: f32, dy: f32) -> Task<Message> {
    run(editor.session.level_line(dx, dy))
}

pub fn handle_set_crop(editor: &mut RawEditor, crop: [f32; 4]) -> Task<Message> {
    run(editor.session.edit_and_commit(|p| p.crop = crop))
}

pub fn handle_toggle_crop(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::None {
        return Task::none();
    }
    switch_tool(editor, |s| s.toggle_crop())
}

pub fn handle_crop_handle_grabbed(
    editor: &mut RawEditor,
    handle: CropHandle,
    _bounds: iced::Rectangle,
) -> Task<Message> {
    editor.drag_mode = DragMode::CropHandle(handle);
    editor.is_dragging = true;
    // See handle_mask_handle_grabbed: `_bounds` is the image rect, and
    // `viewport_size` must stay the canvas size. Overwriting it here made
    // the crop drag re-apply zoom to already-zoomed dims (zoom² scaling),
    // so crop handles drifted from the cursor at any zoom != 1.
    Task::none()
}

pub fn handle_commit_edit(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.commit_edit())
}

pub fn finish_stage_drag(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.finish_stage_drag())
}

pub fn handle_undo(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::None {
        return Task::none();
    }
    run(editor.session.undo())
}

pub fn handle_redo(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::None {
        return Task::none();
    }
    run(editor.session.redo())
}

/// Fold or open a Develop sidebar section, and remember it.
pub fn handle_toggle_section(editor: &mut RawEditor, section: SidebarSection) -> Task<Message> {
    if !editor.collapsed_sections.remove(&section) {
        editor.collapsed_sections.insert(section);
    }
    editor.save_preferences();
    // The loupe renders only while visible, so opening it (or the Detail
    // section around it) is what starts it.
    if matches!(section, SidebarSection::Detail | SidebarSection::DetailPreview) {
        let open = editor.loupe_panel_open();
        run(editor.session.set_loupe_panel_open(open))
    } else {
        Task::none()
    }
}

/// The edits a sidebar section owns (none for the loupe's own section).
pub fn edit_section(section: SidebarSection) -> Option<EditSection> {
    Some(match section {
        SidebarSection::Light => EditSection::Light,
        SidebarSection::ToneCurve => EditSection::ToneCurve,
        SidebarSection::ToneMapping => EditSection::ToneMapping,
        SidebarSection::Colour => EditSection::Colour,
        SidebarSection::Presence => EditSection::Presence,
        SidebarSection::Detail => EditSection::Detail,
        SidebarSection::Lens => EditSection::Lens,
        SidebarSection::Geometry => EditSection::Geometry,
        SidebarSection::Masks => EditSection::Masks,
        SidebarSection::DetailPreview => return None,
    })
}

/// Does this section hold any edits? Drives the header's reset button.
pub fn section_edited(section: SidebarSection, p: &EditParams) -> bool {
    edit_section(section).is_some_and(|s| s.edited(p))
}

pub fn handle_reset_section(editor: &mut RawEditor, section: SidebarSection) -> Task<Message> {
    let Some(section) = edit_section(section) else { return Task::none() };
    if editor.active_modal != Modal::None {
        return Task::none();
    }
    run(editor.session.reset_section(section))
}

pub fn handle_reset_edits(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::None {
        return Task::none();
    }
    run(editor.session.reset_edits())
}

/// Ctrl+C — open the category picker so the user chooses what to copy.
pub fn handle_copy_settings(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::None || !editor.session.prepare_copy() {
        return Task::none();
    }
    editor.active_modal = Modal::CopySettings;
    Task::none()
}

/// Ctrl+Shift+C — copy everything with no modal, so the common case keeps
/// zero friction.
pub fn handle_copy_all_settings(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::None {
        return Task::none();
    }
    editor.session.copy_settings(Some(crate::core::types::CopyCategories::all()));
    Task::none()
}

/// Confirm the picker (its Copy button, or Enter via `ModalConfirm`).
pub fn handle_copy_settings_confirmed(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::CopySettings {
        return Task::none();
    }
    editor.active_modal = Modal::None;
    editor.session.copy_settings(None);
    Task::none()
}

pub fn handle_toggle_copy_category(editor: &mut RawEditor, category: CopyCategory, value: bool) -> Task<Message> {
    editor.session.set_copy_category(category, value);
    editor.save_preferences();
    Task::none()
}

pub fn handle_set_all_copy_categories(editor: &mut RawEditor, all: bool) -> Task<Message> {
    editor.session.set_all_copy_categories(all);
    editor.save_preferences();
    Task::none()
}

/// Ctrl+V — merge the clipboard's categories onto the selection
/// (`Session::paste_settings`).
pub fn handle_paste_settings(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::None {
        return Task::none();
    }
    run(editor.session.paste_settings())
}

pub fn handle_toggle_wb_picker(editor: &mut RawEditor) -> Task<Message> {
    if editor.active_modal != Modal::None {
        return Task::none();
    }
    switch_tool(editor, |s| s.toggle_wb_picker())
}

pub fn handle_wb_picked(editor: &mut RawEditor, u: f32, v: f32) -> Task<Message> {
    run(editor.session.wb_picked(u, v))
}

pub(crate) fn update_pipeline(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.update_pipeline())
}

pub fn clear_before_peek(editor: &mut RawEditor) -> Task<Message> {
    run(editor.session.clear_before_peek())
}

/// Hold-to-peek at the unedited original (Space). Not under a dialog.
pub fn handle_set_before_peek(editor: &mut RawEditor, active: bool) -> Task<Message> {
    if active && editor.active_modal != Modal::None {
        return Task::none();
    }
    run(editor.session.set_before_peek(active))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every section with edits, one field each (and a mask).
    fn edited_everywhere() -> EditParams {
        let mut p = EditParams::default();
        p.exposure = 1.0;
        p.curves.master = crate::core::curves::Curve::from_points(&[[0.0, 0.1], [1.0, 1.0]]);
        p.profile_curve = 0.9;
        p.temperature = 0.2;
        p.clarity = 0.3;
        p.sharpening = 0.5;
        p.rotation = 2.0;
        p.lens.profile = !p.lens.profile;
        p.masks[0] = crate::core::types::MaskParams::new_radial(0.5, 0.5, 0.2, 0.2);
        p.mask_count = 1;
        p
    }

    #[test]
    fn resetting_a_section_clears_it_and_nothing_else() {
        let p = edited_everywhere();
        for s in SidebarSection::ALL {
            let expected = !matches!(s, SidebarSection::DetailPreview);
            assert_eq!(section_edited(s, &p), expected, "{s:?} should start edited");
        }
        for s in SidebarSection::ALL {
            if matches!(s, SidebarSection::Geometry | SidebarSection::Masks | SidebarSection::DetailPreview) {
                continue; // reset by the handler (Geometry) or not at all
            }
            let mut q = p;
            edit_section(s).expect("a section with edits").reset(&mut q);
            assert!(!section_edited(s, &q), "{s:?} still edited after its reset");
            for other in SidebarSection::ALL.into_iter().filter(|&o| o != s) {
                assert_eq!(section_edited(other, &q), section_edited(other, &p), "resetting {s:?} touched {other:?}");
            }
        }
    }

    #[test]
    fn folded_sections_survive_a_save_by_name() {
        for s in SidebarSection::ALL {
            assert_eq!(SidebarSection::from_name(s.name()), Some(s));
        }
        assert_eq!(SidebarSection::from_name("defringe"), None, "an old name is dropped, not misread");
    }
}
