//! Port of libs/librepcb/core/library/librarybaseelement.{h,cpp}.
//!
//! Upstream `LibraryBaseElement` is the base class of all library elements
//! (and of the library itself). Here it is split into:
//!
//! - [`BaseMetadata`]: the common attributes (UUID, version, author, names,
//!   ...), a plain data struct embedded in every element;
//! - [`LibraryBaseElement`]: a trait implemented by every element type,
//!   providing the element specific constants and hooks (loading,
//!   serialization, checks) and the shared behavior (opening, saving,
//!   moving) as provided methods.
//!
//! Differences to upstream:
//! - Loading goes through [`LibraryBaseElement::open()`], which also checks
//!   the directory name (upstream does it in the constructor).
//! - Elements in an older file format are upgraded by the file format
//!   migrations in [`LibraryBaseElement::open()`] for all element types
//!   (upstream: in each element's static `open()`).
//! - The `abortBeforeMigration` flag of `open()` is not ported.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use super::error::{Error, Result};
use super::library_check_message::LibraryCheckMessage;
use crate::application;
use crate::fileio::{FilePath, FileSystem, TransactionalDirectory, VersionFile};
use crate::geometry::property;
use crate::rule_check::all_approvals;
use crate::serialization::List;
use crate::serialization::{
    self, DeserializeObject, FileFormatMigration, LocalizedDescriptionMap, LocalizedKeywordsMap,
    LocalizedNameMap, MigrationError, Mode, SExpression, SerializeObject, file_format_migrations,
};
use crate::types::{ElementName, Uuid, Version};

/// Attributes common to all library elements and libraries (upstream
/// `LibraryBaseElement` members).
#[derive(Debug, Clone, PartialEq)]
pub struct BaseMetadata {
    uuid: Uuid,
    version: Version,
    author: String,
    created: DateTime<Utc>,
    is_deprecated: bool,
    names: LocalizedNameMap,
    descriptions: LocalizedDescriptionMap,
    keywords: LocalizedKeywordsMap,
    message_approvals: BTreeSet<SExpression>,
}

impl BaseMetadata {
    /// Creates the metadata of a new element (not deprecated, without
    /// translations and message approvals).
    pub fn new(
        uuid: Uuid,
        version: Version,
        author: impl Into<String>,
        created: DateTime<Utc>,
        name_en_us: ElementName,
        description_en_us: impl Into<String>,
        keywords_en_us: impl Into<String>,
    ) -> Self {
        Self {
            uuid,
            version,
            author: author.into(),
            created,
            is_deprecated: false,
            names: LocalizedNameMap::new(name_en_us),
            descriptions: LocalizedDescriptionMap::new(description_en_us.into()),
            keywords: LocalizedKeywordsMap::new(keywords_en_us.into()),
            message_approvals: BTreeSet::new(),
        }
    }

    /// Returns the UUID.
    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    property!(
        /// Returns the version.
        ref version: Version, set_version
    );
    property!(
        /// Returns the author.
        ref author: String, set_author
    );
    property!(
        /// Returns the creation date/time.
        copy created: DateTime<Utc>, set_created
    );
    property!(
        /// Returns whether the element is deprecated.
        copy is_deprecated: bool, set_deprecated
    );
    property!(
        /// Returns the localized names.
        ref names: LocalizedNameMap, set_names
    );
    property!(
        /// Returns the localized descriptions.
        ref descriptions: LocalizedDescriptionMap, set_descriptions
    );
    property!(
        /// Returns the localized keywords.
        ref keywords: LocalizedKeywordsMap, set_keywords
    );
    property!(
        /// Returns the approved check messages (their approval nodes).
        ref message_approvals: BTreeSet<SExpression>, set_message_approvals
    );

    /// Returns the default (en_US) name.
    pub fn name(&self) -> &ElementName {
        self.names.default_value()
    }

    /// Returns all locales for which names, descriptions or keywords exist
    /// (sorted, without duplicates, including the default locale `""`).
    pub fn all_available_locales(&self) -> Vec<String> {
        self.names
            .keys()
            .chain(self.descriptions.keys())
            .chain(self.keywords.keys())
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Approves or unapproves a check message. Returns whether the
    /// approvals were modified.
    pub fn set_message_approved(&mut self, approval: &SExpression, approved: bool) -> bool {
        if approved {
            self.message_approvals.insert(approval.clone())
        } else {
            self.message_approvals.remove(approval)
        }
    }

    /// Copies all attributes except the UUID from `other` (upstream part of
    /// `duplicateFrom()`; the creation date should be overwritten
    /// afterwards).
    pub fn duplicate_from(&mut self, other: &BaseMetadata) {
        *self = Self {
            uuid: self.uuid,
            ..other.clone()
        };
    }

    /// Appends the message approvals (sorted) to `root` (upstream
    /// `serializeMessageApprovals()`).
    pub fn serialize_message_approvals(&self, root: &mut List) {
        for node in &self.message_approvals {
            root.ensure_line_break();
            root.push(node.clone());
        }
        root.ensure_line_break();
    }

    /// Removes all approvals which do not match any of `messages`.
    pub fn retain_message_approvals<'a>(
        &mut self,
        messages: impl IntoIterator<Item = &'a LibraryCheckMessage>,
    ) {
        let messages: Vec<_> = messages.into_iter().map(|m| m.to_message()).collect();
        let approvals = all_approvals(&messages);
        self.message_approvals.retain(|a| approvals.contains(a));
    }
}

impl SerializeObject for BaseMetadata {
    /// Serializes the attributes, but not the message approvals (they are
    /// serialized at the end of the element with
    /// [`serialize_message_approvals()`](Self::serialize_message_approvals)).
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        self.names.serialize(root);
        root.ensure_line_break();
        self.descriptions.serialize(root);
        root.ensure_line_break();
        self.keywords.serialize(root);
        root.ensure_line_break();
        root.append_child("author", &self.author);
        root.ensure_line_break();
        root.append_child("version", &self.version);
        root.ensure_line_break();
        root.append_child("created", &self.created);
        root.ensure_line_break();
        root.append_child("deprecated", &self.is_deprecated);
        root.ensure_line_break();
    }
}

impl DeserializeObject for BaseMetadata {
    /// Loads the attributes and message approvals from the root node of an
    /// element file.
    fn deserialize(root: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: root.child_value("@0")?,
            version: root.child_value("version/@0")?,
            author: root.child_value("author/@0")?,
            created: root.child_value("created/@0")?,
            is_deprecated: root.child_value("deprecated/@0")?,
            names: LocalizedNameMap::deserialize(root)?,
            descriptions: LocalizedDescriptionMap::deserialize(root)?,
            keywords: LocalizedKeywordsMap::deserialize(root)?,
            message_approvals: root.children_named("approved").cloned().collect(),
        })
    }
}

/// Behavior shared by all library elements and the library itself (upstream
/// virtual methods of `LibraryBaseElement`).
///
/// An element type provides its constants, access to its [`BaseMetadata`]
/// and directory, loading ([`load()`](Self::load)), serialization of its
/// file content ([`SerializeObject`]) and its checks; opening, saving and
/// moving are provided.
pub trait LibraryBaseElement: SerializeObject + Sized + Send + Sync {
    /// Short element name, used for directory names and the version file
    /// (e.g. `"sym"` for `.librepcb-sym`).
    const SHORT_ELEMENT_NAME: &'static str;
    /// Long element name, used for the file name and the root node (e.g.
    /// `"symbol"` for `symbol.lp` containing `(librepcb_symbol ...)`).
    const LONG_ELEMENT_NAME: &'static str;
    /// Whether the directory name must be the element's UUID (all elements
    /// except libraries).
    const DIRNAME_MUST_BE_UUID: bool = true;

    /// Returns the common attributes.
    fn metadata(&self) -> &BaseMetadata;
    /// Returns the common attributes for modification.
    fn metadata_mut(&mut self) -> &mut BaseMetadata;
    /// Returns the directory of the element.
    fn directory(&self) -> &TransactionalDirectory;
    /// Returns the directory of the element for modification.
    fn directory_mut(&mut self) -> &mut TransactionalDirectory;

    /// Constructs the element from its (current format) file content `root`
    /// and its `directory` (upstream private constructor). Use
    /// [`open()`](Self::open) to load an element.
    fn load(directory: TransactionalDirectory, root: &SExpression) -> Result<Self>;

    /// Runs the library element check (upstream `runChecks()`).
    fn run_checks(&self) -> Result<Vec<LibraryCheckMessage>>;

    /// Opens the element stored in `directory` (upstream static `open()`).
    fn open(mut directory: TransactionalDirectory) -> Result<Self> {
        // Upgrade file format, if needed.
        let version_file = format!(".librepcb-{}", Self::SHORT_ELEMENT_NAME);
        let file_format = read_file_format(&directory, &version_file)?;
        let migrations = file_format_migrations(&file_format);
        for migration in &migrations {
            upgrade_element::<Self>(migration.as_ref(), &mut directory)?;
        }

        // Load element.
        let file_name = format!("{}.lp", Self::LONG_ELEMENT_NAME);
        let content = directory.read(&file_name)?;
        let file_path = directory.abs_path(&file_name);
        let root = SExpression::parse(
            &content,
            file_path.as_ref().map(FilePath::as_path),
            Mode::LibrePcb,
        )?;
        let mut element = Self::load(directory, &root)?;

        // Check directory name.
        if Self::DIRNAME_MUST_BE_UUID {
            let path = element.directory().abs_path("");
            let dir_name = path.as_ref().map(FilePath::file_name).unwrap_or_default();
            let uuid = element.metadata().uuid().to_string();
            if dir_name != uuid {
                return Err(Error::DirectoryNameUuidMismatch {
                    dir_name: dir_name.to_owned(),
                    uuid,
                    path,
                });
            }
        }
        if !migrations.is_empty() {
            element.remove_obsolete_message_approvals()?;
            element.save()?; // Format all files correctly as the migration doesn't!
        }
        Ok(element)
    }

    /// Returns the complete file content as S-expression (root node
    /// `librepcb_<long element name>`).
    fn to_sexpression(&self) -> SExpression {
        let mut root = List::new(format!("librepcb_{}", Self::LONG_ELEMENT_NAME));
        self.serialize(&mut root);
        SExpression::List(root)
    }

    /// Writes the element file and the version file into the directory
    /// (upstream `save()`). Note that the underlying file system still needs
    /// to be saved to write the files to disk.
    fn save(&mut self) -> Result<()> {
        save_element_files(self)
    }

    /// Copies all files into `dest` and makes it the directory of this
    /// element, then saves ("save as").
    fn save_to(&mut self, dest: &mut TransactionalDirectory) -> Result<()> {
        self.directory_mut().save_to(dest)?;
        self.save()
    }

    /// Moves all files into `dest` and makes it the directory of this
    /// element, then saves.
    fn move_to(&mut self, dest: &mut TransactionalDirectory) -> Result<()> {
        self.directory_mut().move_to(dest)?;
        self.save()
    }

    /// Like [`save_to()`](Self::save_to), into the subdirectory named by
    /// the UUID of `dest`.
    fn save_into_parent_directory(&mut self, dest: &mut TransactionalDirectory) -> Result<()> {
        let mut dir = dest.subdir(&self.metadata().uuid().to_string());
        self.save_to(&mut dir)
    }

    /// Like [`move_to()`](Self::move_to), into the subdirectory named by
    /// the UUID of `dest`.
    fn move_into_parent_directory(&mut self, dest: &mut TransactionalDirectory) -> Result<()> {
        let mut dir = dest.subdir(&self.metadata().uuid().to_string());
        self.move_to(&mut dir)
    }

    /// Removes all message approvals which don't match any message of the
    /// library element check anymore.
    fn remove_obsolete_message_approvals(&mut self) -> Result<()> {
        let messages = self.run_checks()?;
        self.metadata_mut().retain_message_approvals(&messages);
        Ok(())
    }

    /// Returns whether `path` in `fs` contains an element of this type,
    /// i.e. its version file (upstream `isValidElementDirectory()`).
    fn is_valid_element_directory(fs: &impl FileSystem, path: &str) -> bool {
        let file = format!(".librepcb-{}", Self::SHORT_ELEMENT_NAME);
        if path.is_empty() {
            fs.file_exists(&file)
        } else {
            fs.file_exists(&format!("{path}/{file}"))
        }
    }

    /// Returns whether the directory `dir` on disk contains an element of
    /// this type (upstream `isValidElementDirectory(FilePath)`).
    fn is_valid_element_directory_path(dir: &FilePath) -> bool {
        dir.path_to(&format!(".librepcb-{}", Self::SHORT_ELEMENT_NAME))
            .is_existing_file()
    }
}

/// Writes the element file and the version file of `element` (default
/// implementation of [`LibraryBaseElement::save()`], for use by overriding
/// implementations).
pub fn save_element_files<E: LibraryBaseElement>(element: &mut E) -> Result<()> {
    let content = element
        .to_sexpression()
        .to_byte_array(Mode::LibrePcb)
        .map_err(Error::from)?;
    let dir = element.directory_mut();
    dir.write(&format!("{}.lp", E::LONG_ELEMENT_NAME), &content)?;
    dir.write(
        &format!(".librepcb-{}", E::SHORT_ELEMENT_NAME),
        &VersionFile::new(application::file_format_version()).to_bytes(),
    )?;
    Ok(())
}

/// Runs the upgrade of `migration` for elements of type `E` (upstream: each
/// element's `open()` calls its `upgrade*()` method).
fn upgrade_element<E: LibraryBaseElement>(
    migration: &dyn FileFormatMigration,
    dir: &mut TransactionalDirectory,
) -> Result<(), MigrationError> {
    // Dispatch on the element type by its short name, which also names its
    // version file (`.librepcb-<short name>`).
    match E::SHORT_ELEMENT_NAME {
        "cmpcat" => migration.upgrade_component_category(dir),
        "pkgcat" => migration.upgrade_package_category(dir),
        "sym" => migration.upgrade_symbol(dir),
        "pkg" => migration.upgrade_package(dir),
        "cmp" => migration.upgrade_component(dir),
        "dev" => migration.upgrade_device(dir),
        "org" => migration.upgrade_organization(dir),
        "lib" => migration.upgrade_library(dir),
        other => Err(MigrationError::UnknownElementType(other)),
    }
}

/// Reads the file format version from the version file `file_name` and
/// checks that it is not newer than the current file format (upstream
/// `readFileFormat()`).
pub fn read_file_format(directory: &TransactionalDirectory, file_name: &str) -> Result<Version> {
    let version = VersionFile::from_bytes(&directory.read(file_name)?)?
        .version()
        .clone();
    if version > application::file_format_version() {
        return Err(Error::NewerFileFormat {
            version,
            path: directory.abs_path(""),
        });
    }
    Ok(version)
}
