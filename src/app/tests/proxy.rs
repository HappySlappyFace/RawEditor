//! Edited proxies and the render that lands after the user left an image
//! (`session::proxy`, through `handlers::develop::handle_render_finished`).

use std::sync::Arc;

use crate::app::state::RawEditor;
use crate::core::types::EditParams;
use crate::session::edit_key;

fn edited() -> EditParams {
    EditParams { exposure: -1.0, ..EditParams::default() }
}

fn late_render(id: i64, edit: EditParams) -> crate::app::message::RenderOutput {
    crate::app::message::RenderOutput {
        image_id: id,
        display: Arc::from(Vec::new()),
        dims: (1, 1),
        view_rect: crate::gpu::params::FULL_VIEW_RECT,
        overview: Arc::from(vec![9u8; 8]),
        overview_dims: (2, 1),
        histogram: [[0; 256]; 3],
        edit: Some(edit),
        upload_ms: 0.0,
        render_ms: 0.0,
        update_ms: 0.0,
    }
}

/// Released a slider and moved straight on: the final render lands after
/// the user left, and must still become the image's thumbnail.
#[test]
fn the_awaited_final_render_becomes_the_proxy() {
    let mut e = RawEditor::new().0;
    e.session.proxy_awaiting.insert(5, edit_key(&edited()));
    let _ = crate::app::handlers::develop::handle_render_finished(&mut e, late_render(5, edited()));
    assert!(e.session.edited_proxies.peek(&5).is_some_and(|p| p.edited));
    assert!(e.session.proxy_awaiting.is_empty());
}

/// ...but a render of an older step of the edit must not.
#[test]
fn an_awaited_render_of_an_older_edit_is_dropped() {
    let mut e = RawEditor::new().0;
    e.session.proxy_awaiting.insert(5, edit_key(&edited()));
    let older = EditParams { exposure: -0.5, ..EditParams::default() };
    let _ = crate::app::handlers::develop::handle_render_finished(&mut e, late_render(5, older));
    assert!(e.session.edited_proxies.peek(&5).is_none());
    assert!(e.session.proxy_awaiting.is_empty());
}

/// Without anyone waiting on it, a render of an image no longer loaded
/// is ignored entirely.
#[test]
fn a_stray_render_of_another_image_is_ignored() {
    let mut e = RawEditor::new().0;
    let _ = crate::app::handlers::develop::handle_render_finished(&mut e, late_render(5, edited()));
    assert!(e.session.edited_proxies.peek(&5).is_none());
}
