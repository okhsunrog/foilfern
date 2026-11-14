//! Port of libs/librepcb/core/library/library.{h,cpp}.
//!
//! Differences to upstream:
//! - The URL is stored verbatim as string instead of a `QUrl` (see
//!   COMPAT.md).
//! - `getIconAsPixmap()` is not ported (UI); [`Library::icon()`] returns
//!   the PNG file content.

use std::collections::BTreeSet;

use super::error::{Error, Result};
use super::library_base_element::{BaseMetadata, LibraryBaseElement, save_element_files};
use super::library_base_element_check::run_base_element_checks;
use super::library_check_message::LibraryCheckMessage;
use crate::fileio::{FileSystem, TransactionalDirectory};
use crate::geometry::property;
use crate::serialization::{DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{SimpleString, Uuid};

/// File name of the library icon.
const ICON_FILE_NAME: &str = "library.png";

/// A library (`*.lplib` directory) containing library elements.
#[derive(Debug)]
pub struct Library {
    directory: TransactionalDirectory,
    metadata: BaseMetadata,
    url: String,
    dependencies: BTreeSet<Uuid>,
    icon: Vec<u8>,
    manufacturer: SimpleString,
}

static_assertions::assert_impl_all!(Library: Send, Sync);

impl Library {
    /// Creates a new library in a temporary directory; use
    /// [`move_to()`](LibraryBaseElement::move_to) or
    /// [`save_to()`](LibraryBaseElement::save_to) to store it.
    pub fn new(metadata: BaseMetadata) -> Result<Self> {
        Ok(Self {
            directory: TransactionalDirectory::new_temporary()?,
            metadata,
            url: String::new(),
            dependencies: BTreeSet::new(),
            icon: Vec::new(),
            manufacturer: SimpleString::default(),
        })
    }

    property!(
        /// Returns the URL of the library (e.g. its repository; may be empty).
        ref url: String, set_url
    );
    property!(
        /// Returns the UUIDs of the libraries this library depends on.
        ref dependencies: BTreeSet<Uuid>, set_dependencies
    );
    property!(
        /// Returns the icon (PNG file content, empty if there is no icon).
        ref icon: Vec<u8>, set_icon
    );
    property!(
        /// Returns the manufacturer (empty if the library is not about a
        /// specific manufacturer).
        ref manufacturer: SimpleString, set_manufacturer
    );

    /// Returns the name of the subdirectory containing elements of type `E`
    /// (e.g. `"sym"`).
    pub fn elements_directory_name<E: LibraryBaseElement>() -> &'static str {
        E::SHORT_ELEMENT_NAME
    }

    /// Returns the paths (relative to the library directory) of all element
    /// directories of type `E`.
    ///
    /// Non-empty directories which don't contain an element are ignored
    /// with a warning.
    pub fn search_for_elements<E: LibraryBaseElement>(&self) -> Vec<String> {
        let subdir = Self::elements_directory_name::<E>();
        let mut list = Vec::new();
        for dir_name in self.directory.dirs(subdir) {
            let dir_path = format!("{subdir}/{dir_name}");
            if E::is_valid_element_directory(&self.directory, &dir_path) {
                list.push(dir_path);
            } else if !self.directory.files(&dir_path).is_empty() {
                // Note: Do not warn about empty directories since this happens
                // often when switching branches, leading to annoying warnings.
                log::warn!(
                    "Directory is not a valid library element, ignoring it: {}",
                    self.directory
                        .abs_path(&dir_path)
                        .map(|p| p.to_native())
                        .unwrap_or(dir_path)
                );
            }
        }
        list
    }
}

impl LibraryBaseElement for Library {
    const SHORT_ELEMENT_NAME: &'static str = "lib";
    const LONG_ELEMENT_NAME: &'static str = "library";
    const DIRNAME_MUST_BE_UUID: bool = false;

    fn metadata(&self) -> &BaseMetadata {
        &self.metadata
    }
    fn metadata_mut(&mut self) -> &mut BaseMetadata {
        &mut self.metadata
    }
    fn directory(&self) -> &TransactionalDirectory {
        &self.directory
    }
    fn directory_mut(&mut self) -> &mut TransactionalDirectory {
        &mut self.directory
    }

    fn load(directory: TransactionalDirectory, root: &SExpression) -> Result<Self> {
        // Check directory suffix.
        let path = directory.abs_path("");
        if path.as_ref().is_none_or(|p| p.suffix() != "lplib") {
            return Err(Error::InvalidLibrarySuffix { path });
        }
        Ok(Self {
            metadata: BaseMetadata::deserialize(root)?,
            url: root.child_value("url/@0")?,
            dependencies: root
                .children_named("dependency")
                .map(|node| node.child_value("@0"))
                .collect::<crate::serialization::Result<_>>()?,
            icon: directory
                .read_if_exists(ICON_FILE_NAME)?
                .unwrap_or_default(),
            manufacturer: root.child_value("manufacturer/@0")?,
            directory,
        })
    }

    fn run_checks(&self) -> Result<Vec<LibraryCheckMessage>> {
        let mut msgs = Vec::new();
        run_base_element_checks(&self.metadata, &mut msgs);
        Ok(msgs)
    }

    fn save(&mut self) -> Result<()> {
        save_element_files(self)?;
        if self.icon.is_empty() {
            self.directory.remove_file(ICON_FILE_NAME)?;
        } else {
            self.directory.write(ICON_FILE_NAME, &self.icon)?;
        }
        Ok(())
    }

    fn move_to(&mut self, dest: &mut TransactionalDirectory) -> Result<()> {
        // Check directory suffix.
        if dest.abs_path("").is_none_or(|p| p.suffix() != "lplib") {
            log::debug!("Invalid library: {:?}", dest.abs_path(""));
            return Err(Error::InvalidLibraryDestination);
        }
        self.directory.move_to(dest)?;
        self.save()
    }
}

impl SerializeObject for Library {
    fn serialize(&self, root: &mut List) {
        self.metadata.serialize(root);
        root.ensure_line_break();
        root.append_child("url", &self.url);
        for uuid in &self.dependencies {
            root.ensure_line_break();
            root.append_child("dependency", uuid);
        }
        root.ensure_line_break();
        root.append_child("manufacturer", &self.manufacturer);
        self.metadata.serialize_message_approvals(root);
        root.ensure_line_break();
    }
}
