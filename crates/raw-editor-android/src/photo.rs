//! The open photo as the UI's top bar shows it: whether it is loading,
//! ready or failed, its name, and its camera, exposure and lens. Builds off
//! Android, so its tests run with the workspace's.

/// Where the open photo is.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
pub enum PhotoState {
    #[default]
    None,
    Loading,
    Ready,
    Failed,
}

/// The open photo, as the UI's top bar shows it.
#[derive(Default, Clone, PartialEq, Debug)]
pub struct Photo {
    pub state: PhotoState,
    /// The file's name.
    pub name: String,
    /// Make and model.
    pub camera: String,
    /// ISO, shutter, aperture, as the desktop's info panel has them.
    pub exposure: String,
    pub lens: String,
    /// Why it failed.
    pub error: String,
}

impl Photo {
    pub fn loading(name: String) -> Self {
        Photo { state: PhotoState::Loading, name, ..Default::default() }
    }

    pub fn failed(name: String, error: impl Into<String>) -> Self {
        Photo { state: PhotoState::Failed, name, error: error.into(), ..Default::default() }
    }

    /// For the UI, tab-separated: state (`none`, `loading`, `ready`,
    /// `failed`), name, camera, exposure, lens, error. Tabs inside a field
    /// become spaces.
    pub fn line(&self) -> String {
        let state = match self.state {
            PhotoState::None => "none",
            PhotoState::Loading => "loading",
            PhotoState::Ready => "ready",
            PhotoState::Failed => "failed",
        };
        [state, &self.name, &self.camera, &self.exposure, &self.lens, &self.error]
            .map(|f| f.replace(['\t', '\n'], " "))
            .join("\t")
    }

    /// Camera, exposure and lens from a decoded RAW's metadata. EXIF that
    /// wasn't there reads `---`; it is left out rather than shown.
    pub fn describe(&mut self, make: &str, model: &str, iso: &str, shutter: &str, aperture: &str, lens: &str) {
        let known = |s: &str| !s.trim().is_empty() && !s.contains("---");
        // Most makers repeat their name in the model: "NIKON CORPORATION",
        // "NIKON D3300".
        let model = model.trim();
        let maker = make.split_whitespace().next().unwrap_or("").to_lowercase();
        self.camera = if known(make) && !model.to_lowercase().starts_with(&maker) {
            format!("{} {model}", make.trim())
        } else {
            model.to_string()
        };
        let mut exposure = Vec::new();
        if known(shutter) {
            exposure.push(if shutter.contains('/') || shutter.ends_with('s') { shutter.to_string() } else { format!("{shutter} s") });
        }
        if known(aperture) {
            exposure.push(format!("f/{aperture}"));
        }
        if known(iso) {
            exposure.push(format!("ISO {iso}"));
        }
        self.exposure = exposure.join("  ");
        self.lens = if known(lens) { lens.trim().to_string() } else { String::new() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_has_every_field_in_order() {
        let mut p = Photo { state: PhotoState::Ready, name: "DSC_0001.NEF".into(), ..Default::default() };
        p.describe("NIKON CORPORATION", "NIKON D3300", "400", "1/250", "5.6", "18-55mm f/3.5-5.6");
        assert_eq!(p.line(), "ready\tDSC_0001.NEF\tNIKON D3300\t1/250  f/5.6  ISO 400\t18-55mm f/3.5-5.6\t");
    }

    #[test]
    fn the_make_is_added_only_when_the_model_lacks_it() {
        let mut p = Photo::default();
        p.describe("Canon", "Canon EOS R6", "100", "1/60", "4", "");
        assert_eq!(p.camera, "Canon EOS R6");
        p.describe("Panasonic", "DC-G9", "100", "1/60", "4", "");
        assert_eq!(p.camera, "Panasonic DC-G9");
    }

    #[test]
    fn missing_exif_is_left_out() {
        let mut p = Photo::default();
        p.describe("", "X100V", "---", "2", "---", "---");
        assert_eq!((p.exposure.as_str(), p.lens.as_str()), ("2 s", ""));
    }

    #[test]
    fn a_tab_in_a_field_cannot_shift_the_others() {
        let p = Photo::failed("a\tb.NEF".into(), "bad\nfile");
        assert_eq!(p.line().split('\t').count(), 6);
        assert_eq!(p.line(), "failed\ta b.NEF\t\t\t\tbad file");
        assert_eq!(Photo::loading("x".into()).line(), "loading\tx\t\t\t\t");
    }
}
