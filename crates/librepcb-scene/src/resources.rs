//! Application resources (upstream `Application::getResourcesDir()` with
//! the files of `share/librepcb`): the stroke fonts which new projects get
//! (`resources/fontobene/*.bene`) and which render texts without project
//! fonts.
//!
//! The fonts are looked up in a resources directory on disk (the
//! environment variable `LIBREPCB_SHARE`, else `../share/librepcb` next to
//! the executable), and are also embedded into the binary at build time
//! from the upstream checkout (`$LIBREPCB_UPSTREAM_DIR/share/librepcb`), so
//! an installed binary works on machines without the upstream resources.
//! A resources directory on disk wins over the embedded files.

use std::path::PathBuf;

/// Name of the default stroke font (upstream
/// `Application::getDefaultStrokeFontName()`).
pub const DEFAULT_STROKE_FONT: &str = "newstroke.bene";

/// The stroke fonts embedded at build time (upstream
/// `share/librepcb/fontobene`), as (file name, content).
pub const EMBEDDED_STROKE_FONTS: &[(&str, &[u8])] = &[(
    DEFAULT_STROKE_FONT,
    include_bytes!(concat!(
        env!("LIBREPCB_UPSTREAM_DIR"),
        "/share/librepcb/fontobene/newstroke.bene"
    )),
)];

/// Font files as (file name, content).
pub type FontFiles = Vec<(String, Vec<u8>)>;

/// Where the resources come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceSource {
    /// A resources directory on disk (`share/librepcb`).
    Directory(PathBuf),
    /// The files embedded into the binary.
    Embedded,
}

/// Returns the resources directory on disk (containing `fontobene/`):
/// `$LIBREPCB_SHARE`, else `../share/librepcb` relative to the executable.
/// `None` if neither exists (the embedded resources are used then).
pub fn resources_dir() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("LIBREPCB_SHARE") {
        dirs.push(dir.into());
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(bin) = exe.parent()
    {
        dirs.push(bin.join("../share/librepcb"));
    }
    dirs.into_iter()
        .filter(|dir| dir.join("fontobene").is_dir())
        .find_map(|dir| std::path::absolute(dir).ok())
}

/// Returns the stroke font files (`*.bene`, sorted by name) as (file name,
/// content) and where they come from: the resources directory on disk if
/// there is one (see [`resources_dir()`]), else the embedded fonts.
pub fn stroke_font_files() -> std::io::Result<(FontFiles, ResourceSource)> {
    if let Some(dir) = resources_dir() {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir.join("fontobene"))? {
            let path = entry?.path();
            if path.is_file() && path.extension().is_some_and(|e| e == "bene") {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                files.push((name, std::fs::read(&path)?));
            }
        }
        files.sort();
        return Ok((files, ResourceSource::Directory(dir)));
    }
    Ok((embedded_stroke_font_files(), ResourceSource::Embedded))
}

/// Returns the embedded stroke font files as (file name, content).
pub fn embedded_stroke_font_files() -> FontFiles {
    EMBEDDED_STROKE_FONTS
        .iter()
        .map(|(name, content)| ((*name).to_owned(), content.to_vec()))
        .collect()
}

/// Returns the content of the default stroke font ([`DEFAULT_STROKE_FONT`])
/// from the resources directory, else the embedded one.
pub fn default_stroke_font_data() -> Vec<u8> {
    resources_dir()
        .and_then(|dir| std::fs::read(dir.join("fontobene").join(DEFAULT_STROKE_FONT)).ok())
        .unwrap_or_else(|| {
            EMBEDDED_STROKE_FONTS
                .iter()
                .find(|(name, _)| *name == DEFAULT_STROKE_FONT)
                .map(|(_, content)| content.to_vec())
                .unwrap_or_default()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fonts() {
        let files = embedded_stroke_font_files();
        assert_eq!(files[0].0, DEFAULT_STROKE_FONT);
        let upstream = std::fs::read(
            std::path::Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
                .join("share/librepcb/fontobene/newstroke.bene"),
        )
        .unwrap();
        assert_eq!(files[0].1, upstream);
        let font = librepcb_core::font::StrokeFont::new(default_stroke_font_data());
        assert!(font.load_error().is_none());
    }
}
