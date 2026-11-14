//! Native file dialogs (upstream `FileDialog`, `QFileDialog`) through
//! `rfd`.
//!
//! For automated UI sessions without a desktop (headless screenshots,
//! Slint's MCP server), the environment variable
//! `LIBREPCB_FILE_DIALOG_RESULT` answers every file dialog with the given
//! path instead of showing a native dialog (an empty value cancels).

use std::path::{Path, PathBuf};

/// The environment variable which answers file dialogs (see the module
/// documentation).
pub const RESULT_ENV: &str = "LIBREPCB_FILE_DIALOG_RESULT";

/// A filter of a file dialog: its (translated) name and the extensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    /// The name, e.g. "Images (*.png *.jpg)".
    pub name: String,
    /// The extensions without dot.
    pub extensions: Vec<&'static str>,
}

/// Asks for an existing file to open; `None` if canceled.
pub fn open_file(title: &str, filters: &[Filter], directory: Option<&Path>) -> Option<PathBuf> {
    if let Some(result) = std::env::var_os(RESULT_ENV) {
        return (!result.is_empty()).then(|| PathBuf::from(result));
    }
    let mut dialog = rfd::FileDialog::new().set_title(title);
    for f in filters {
        dialog = dialog.add_filter(&f.name, &f.extensions);
    }
    if let Some(dir) = directory {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_file()
}

/// Asks for a directory; `None` if canceled.
pub fn choose_directory(title: &str, directory: Option<&Path>) -> Option<PathBuf> {
    if let Some(result) = std::env::var_os(RESULT_ENV) {
        return (!result.is_empty()).then(|| PathBuf::from(result));
    }
    let mut dialog = rfd::FileDialog::new().set_title(title);
    if let Some(dir) = directory {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_folder()
}
