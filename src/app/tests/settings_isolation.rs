//! The app's tests must never read or write the user's settings file. The
//! guard lives in the engine (`AppSettings::{load_or_default, save}`), which
//! these tests build without `cfg(test)`: only the `test-support` feature
//! reaches it. Without that, `RawEditor::new` loaded the developer's real
//! preferences and every `save_preferences` wrote over them.

use crate::app::message::{CopyCategory, Message};
use crate::app::testkit::*;
use crate::core::settings::AppSettings;

#[test]
fn saving_preferences_never_writes_the_settings_file() {
    // Pointed at a temp folder first, so a broken guard writes there, not
    // over the user's file. Left behind: other tests in this process may
    // resolve paths under it meanwhile.
    let root = std::env::temp_dir().join(format!("raw-editor-settings-isolation-{}", std::process::id()));
    crate::core::paths::redirect_for_tests(&root);
    let path = AppSettings::settings_path();
    assert!(path.starts_with(&root), "{} is not redirected", path.display());

    let mut e = editor_with_images(1);
    send(&mut e, Message::SetAllCopyCategories(false));
    send(&mut e, Message::ToggleCopyCategory(CopyCategory::Tone, true));
    send(&mut e, Message::ToggleAutoAdvance);
    e.save_preferences();
    assert!(!path.exists(), "a test wrote {}", path.display());
}

