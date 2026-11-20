//! The desktop entry that gives the app its icon on Linux.
//!
//! On X11 and Windows the window carries its own icon (`main`). A Wayland
//! window has none: the taskbar shows the icon of the `.desktop` file named
//! after the window's app id ([`APP_ID`]), and without one a generic icon.
//! So at startup the app makes sure the user's applications folder has an
//! entry pointing at wherever this binary runs from, with the icon kept in
//! the app's data folder. Both are written only when they differ: moving or
//! updating the binary repoints the entry, an unchanged launch touches
//! nothing. `TryExec` makes launchers hide the entry once the binary is gone.

use std::path::Path;

/// The window's Wayland app id / X11 class, and the entry's file name.
pub const APP_ID: &str = "raw-editor";

/// Write the entry and its icon if they are missing or stale. `Ok(true)`
/// when anything was written. Blocking (small files): run it off the UI
/// thread.
pub fn install() -> Result<bool, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate the executable: {e}"))?;
    // Replaced under a running process (a rebuild), the link reads
    // "… (deleted)": the next launch will know the real path.
    if !exe.is_file() {
        return Ok(false);
    }
    let apps = dirs::data_dir().ok_or("no data folder")?.join("applications");
    let icon = crate::core::paths::data_dir().join("icon.png");
    let entry = entry(&exe, &icon).ok_or("the path cannot be written in a desktop entry")?;
    let wrote_icon = write_if_changed(&icon, crate::APP_ICON_PNG)?;
    let wrote_entry = write_if_changed(&apps.join(format!("{APP_ID}.desktop")), entry.as_bytes())?;
    Ok(wrote_icon || wrote_entry)
}

fn entry(exe: &Path, icon: &Path) -> Option<String> {
    let (exe, icon) = (exe.to_str()?, icon.to_str()?);
    if [exe, icon].iter().any(|s| s.contains(['\n', '\r'])) {
        return None;
    }
    Some(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=RawEditor\n\
         GenericName=RAW Photo Editor\n\
         Comment=Edit and organise RAW photos\n\
         Exec={exec}\n\
         TryExec={try_exec}\n\
         Icon={icon}\n\
         Terminal=false\n\
         Categories=Graphics;Photography;\n\
         StartupWMClass={APP_ID}\n",
        exec = exec_quote(exe),
        try_exec = escape(exe),
        icon = escape(icon),
    ))
}

/// A `string` value: backslash and tab are the escapes a path can need
/// (line breaks are refused, and paths never start with a space).
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t")
}

/// `Exec`'s own quoting, under the string escape: one double-quoted
/// argument (the spec reserves space among others, so always quote) with
/// `"`, `` ` ``, `$` and `\` backslash-escaped, and `%` doubled.
fn exec_quote(path: &str) -> String {
    let mut quoted = String::from("\"");
    for c in path.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    escape(&quoted)
}

/// Replace `path` with `bytes` unless it already holds them. Via a rename,
/// so the desktop's folder watcher never reads half a file.
fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(false);
    }
    let dir = path.parent().ok_or("no parent folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let part = path.with_extension("part");
    std::fs::write(&part, bytes).map_err(|e| format!("cannot write {}: {e}", part.display()))?;
    std::fs::rename(&part, path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_paths_are_quoted_per_the_spec() {
        assert_eq!(exec_quote("/opt/raw-editor"), "\"/opt/raw-editor\"");
        assert_eq!(exec_quote("/home/a b/raw editor"), "\"/home/a b/raw editor\"");
        assert_eq!(exec_quote("/x/100%/$y"), "\"/x/100%%/\\\\$y\"");
        // A literal backslash takes four: Exec's escape, then the string's.
        assert_eq!(exec_quote("/x\\y"), "\"/x\\\\\\\\y\"");
    }

    #[test]
    fn the_entry_names_the_window_class_and_icon() {
        let e = entry(Path::new("/opt/My Apps/raw-editor"), Path::new("/home/u/.local/share/raw-editor/icon.png"))
            .unwrap();
        assert!(e.starts_with("[Desktop Entry]\n"));
        assert!(e.contains("\nExec=\"/opt/My Apps/raw-editor\"\n"));
        assert!(e.contains("\nTryExec=/opt/My Apps/raw-editor\n"));
        assert!(e.contains("\nIcon=/home/u/.local/share/raw-editor/icon.png\n"));
        assert!(e.contains(&format!("\nStartupWMClass={APP_ID}\n")));
        assert_eq!(entry(Path::new("/a\nb"), Path::new("/i.png")), None);
    }

    #[test]
    fn unchanged_files_are_left_alone() {
        let dir = std::env::temp_dir().join(format!("raw-editor-desktop-{}", std::process::id()));
        let path = dir.join("applications").join("raw-editor.desktop");
        assert_eq!(write_if_changed(&path, b"one"), Ok(true));
        assert_eq!(write_if_changed(&path, b"one"), Ok(false));
        assert_eq!(write_if_changed(&path, b"two"), Ok(true));
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert!(!path.with_extension("part").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The bundled icon decodes as the square PNG the window wants.
    #[test]
    fn the_app_icon_is_a_square_png() {
        let icon = image::load_from_memory(crate::APP_ICON_PNG).unwrap();
        assert_eq!(icon.width(), icon.height());
        assert!(icon.width() >= 128);
        let r = iced::window::icon::from_file_data(crate::APP_ICON_PNG, None);
        assert!(r.is_ok(), "{:?}", r.err());
    }
}
