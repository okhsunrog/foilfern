//! The "create library" tab.
//!
//! Port of libs/librepcb/editor/library/createlibrarytab.{h,cpp}: name,
//! description, author, version, URL, CC0 license and directory (below
//! `<workspace>/data/libraries/local/`) are validated while typing;
//! "Create" writes the new library (with upstream's default icon, README,
//! `.gitignore`, `.gitattributes` and optionally the CC0 license text) and
//! asks the application to open it.
//!
//! The template files are embedded from upstream's `share/librepcb` at
//! build time (like the stroke fonts of `librepcb-scene`).

use std::sync::Arc;

use chrono::Utc;
use librepcb_app_ui as ui;
use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::library::Library;
use librepcb_core::library::{BaseMetadata, LibraryBaseElement};
use librepcb_core::types::{ElementName, Uuid, Version};
use librepcb_i18n::tr;

use super::{TabId, TabRequest, TabUpdate};
use crate::validation;

macro_rules! share_file {
    ($path:literal) => {
        include_bytes!(concat!(
            env!("LIBREPCB_UPSTREAM_DIR"),
            "/share/librepcb/",
            $path
        ))
    };
}

/// Upstream `share/librepcb/library/default_image.png`.
pub const DEFAULT_LIBRARY_ICON: &[u8] = share_file!("library/default_image.png");
const README_TEMPLATE: &[u8] = share_file!("library/readme_template");
const GITIGNORE_TEMPLATE: &[u8] = share_file!("library/gitignore_template");
const GITATTRIBUTES_TEMPLATE: &[u8] = share_file!("library/gitattributes_template");
const CC0_LICENSE: &[u8] = share_file!("licenses/cc0-1.0.txt");

/// The suffix of library directories.
pub const LIBRARY_SUFFIX: &str = ".lplib";
/// Maximum length of library directory names (upstream: "some sane
/// maximum").
pub const MAX_DIRNAME_LENGTH: usize = 50;

/// The "create library" tab (upstream `CreateLibraryTab`).
pub struct CreateLibraryTab {
    id: TabId,
    local_libraries: FilePath,
    data: ui::CreateLibraryTabData,
    name: Option<ElementName>,
    version: Option<Version>,
    url: Option<librepcb_network::Url>,
    directory: Option<FilePath>,
}

impl CreateLibraryTab {
    /// A new tab creating libraries in `local_libraries` with the author
    /// `user_name` preset.
    pub fn new(local_libraries: FilePath, user_name: &str) -> Self {
        let mut tab = Self {
            id: TabId::new(),
            local_libraries,
            data: ui::CreateLibraryTabData {
                // Not translated by intention (like upstream).
                name: "My Library".into(),
                author: user_name.into(),
                version_default: "0.1".into(),
                ..Default::default()
            },
            name: None,
            version: None,
            url: None,
            directory: None,
        };
        tab.validate();
        tab
    }

    /// The unique tab identifier.
    pub fn id(&self) -> TabId {
        self.id
    }

    /// The `TabData`.
    pub fn ui_data(&self) -> ui::TabData {
        ui::TabData {
            r#type: ui::TabType::CreateLibrary,
            title: tr!("librepcb::editor::CreateLibraryTab", "New Library").into(),
            ..Default::default()
        }
    }

    /// The `CreateLibraryTabData`.
    pub fn derived_ui_data(&self) -> ui::CreateLibraryTabData {
        self.data.clone()
    }

    /// The directory the library will be created in (if valid).
    pub fn directory(&self) -> Option<&FilePath> {
        self.directory.as_ref()
    }

    /// Applies the data written by the UI.
    pub fn set_derived_ui_data(&mut self, data: &ui::CreateLibraryTabData) -> TabUpdate {
        self.data = data.clone();
        self.validate();
        TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        }
    }

    fn validate(&mut self) {
        let d = &mut self.data;
        let name = d.name.replace(LIBRARY_SUFFIX, "");
        let mut error = String::new();
        self.name = validation::element_name(&name, &mut error);
        d.name_error = error.as_str().into();

        let mut version = d.version.trim().to_owned();
        if version.is_empty() {
            version = d.version_default.to_string();
        }
        self.version = validation::version(&version, &mut error);
        d.version_error = error.as_str().into();

        self.url = validation::url(&d.url, &mut error, true);
        d.url_error = error.as_str().into();

        let (default, directory) =
            directory_for(&self.local_libraries, &name, &d.directory, &mut error);
        d.directory_default = default.into();
        self.directory = directory;
        d.directory_error = error.as_str().into();

        d.valid = self.name.is_some()
            && self.version.is_some()
            && d.url_error.is_empty()
            && self.directory.is_some();
    }

    /// Handles a tab action (upstream `CreateLibraryTab::trigger()`).
    pub fn trigger(&mut self, action: ui::TabAction) -> TabUpdate {
        match action {
            ui::TabAction::Cancel => TabUpdate {
                close: true,
                ..TabUpdate::default()
            },
            ui::TabAction::Accept => match self.create() {
                Ok(dir) => TabUpdate {
                    close: true,
                    requests: vec![TabRequest::OpenLibrary {
                        path: dir,
                        wizard: true,
                    }],
                    ..TabUpdate::default()
                },
                Err(e) => {
                    self.data.creation_error = e.into();
                    TabUpdate {
                        data_changed: true,
                        ..TabUpdate::default()
                    }
                }
            },
            _ => TabUpdate::default(),
        }
    }

    /// Creates the library; returns its directory.
    fn create(&self) -> Result<FilePath, String> {
        let (Some(name), Some(version), Some(dir)) = (
            self.name.clone(),
            self.version.clone(),
            self.directory.clone(),
        ) else {
            return Err(tr!("SlintHelpers", "Invalid"));
        };
        if dir.is_existing_dir() || dir.is_existing_file() {
            return Err(tr!("librepcb::editor::CreateLibraryTab", "Exists already"));
        }
        let fs = Arc::new(TransactionalFileSystem::open_rw(&dir).map_err(|e| e.to_string())?);
        let mut directory = TransactionalDirectory::new(Arc::clone(&fs), "");
        let metadata = BaseMetadata::new(
            Uuid::new_random(),
            version,
            self.data.author.trim(),
            Utc::now(),
            name.clone(),
            self.data.description.trim(),
            "",
        );
        let mut lib = Library::new(metadata).map_err(|e| e.to_string())?;
        if let Some(url) = &self.url {
            lib.set_url(url.to_string());
        }
        lib.set_icon(DEFAULT_LIBRARY_ICON.to_vec());
        lib.move_to(&mut directory).map_err(|e| e.to_string())?;

        // Additional files.
        let write = |path: &str, content: &[u8]| {
            if let Err(e) = fs.write(path, content) {
                log::error!("Failed to write {path}: {e}");
            }
        };
        if self.data.cc0 {
            write("LICENSE.txt", CC0_LICENSE);
        }
        let license = if self.data.cc0 {
            "Creative Commons (CC0-1.0). For the license text, see [LICENSE.txt](LICENSE.txt)."
        } else {
            "No license set."
        };
        let readme = String::from_utf8_lossy(README_TEMPLATE)
            .replace("{LIBRARY_NAME}", name.as_str())
            .replace("{LICENSE_TEXT}", license);
        write("README.md", readme.as_bytes());
        write(".gitignore", GITIGNORE_TEMPLATE);
        write(".gitattributes", GITATTRIBUTES_TEMPLATE);
        fs.save().map_err(|e| e.to_string())?;
        drop(lib);
        drop(directory);
        // Release the directory lock before the library is opened again.
        fs.release_lock().map_err(|e| e.to_string())?;
        Ok(dir)
    }
}

/// The default and the validated library directory for a name and the
/// user's directory input (upstream `validate()` of the create and download
/// library tabs): `(default directory name, directory or None, error)`.
pub fn directory_for(
    local_libraries: &FilePath,
    name: &str,
    input: &str,
    error: &mut String,
) -> (String, Option<FilePath>) {
    let mut default = FilePath::clean_file_name(
        name,
        validation::LIBRARY_DIR_OPTIONS,
        MAX_DIRNAME_LENGTH - LIBRARY_SUFFIX.len(),
    );
    if !default.is_empty() {
        default.push_str(LIBRARY_SUFFIX);
    }
    let mut dir = input.trim().to_owned();
    if dir.is_empty() {
        dir = default.clone();
    }
    let dir = validation::file_name(
        &dir,
        error,
        validation::LIBRARY_DIR_OPTIONS,
        MAX_DIRNAME_LENGTH,
        LIBRARY_SUFFIX,
    )
    .map(|d: String| local_libraries.path_to(&d));
    let dir = match dir {
        Some(d) if d.is_existing_file() || d.is_existing_dir() => {
            *error = tr!("librepcb::editor::CreateLibraryTab", "Exists already");
            None
        }
        other => other,
    };
    (default, dir)
}
