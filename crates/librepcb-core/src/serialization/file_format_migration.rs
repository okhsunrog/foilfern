//! Port of libs/librepcb/core/serialization/fileformatmigration.{h,cpp}.
//!
//! File format migrations upgrade libraries, library elements, projects and
//! workspace data written by older LibrePCB versions to the current file
//! format. They operate on the raw files of a [`TransactionalDirectory`]
//! (parsed as [`SExpression`] trees), so the deserializers only need to
//! handle the current file format. Like upstream, the migrations only
//! modify the tree; the caller re-serializes the loaded objects afterwards
//! to get correctly formatted files.
//!
//! One migration upgrades one file format step: [`V01Migration`] (0.1 → 1)
//! and [`V1Migration`] (1 → 2). [`UnstableMigration`] partially upgrades
//! files of an unstable (development) release of the current file format;
//! [`file_format_migrations()`] only returns it if the environment variable
//! `LIBREPCB_UPGRADE_UNSTABLE` is `1`.
//!
//! Differences to upstream:
//! - The migrations are a trait implemented by plain structs; upstream's
//!   protected virtual methods (overridden by the unstable migration) are
//!   the provided methods of the private `ProjectSteps` trait of each
//!   version step.
//! - Messages are logged with `log::info!` instead of `qInfo()`.

mod unstable;
mod v01;
mod v1;

use std::fmt;
use std::sync::LazyLock;

use librepcb_i18n::tr;

pub use unstable::UnstableMigration;
pub use v01::V01Migration;
pub use v1::V1Migration;

use super::error::ParseError;
use super::{Error, List, Mode, SExpression};
use crate::application;
use crate::fileio::{self, FilePath, FileSystem, TransactionalDirectory, VersionFile};
use crate::types::{Uuid, Version};

/// Result type of the file format migrations.
pub type MigrationResult<T> = std::result::Result<T, MigrationError>;

/// Suffix of upstream's `RuntimeError` messages in
/// `FileFormatMigrationV01::upgradeSchematic()`.
const CONTACT_US: &str = " If the project could be opened with older releases, this might be a \
                          bug - please contact us then.";

/// Error returned by a file format migration.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MigrationError {
    /// File access failed.
    #[error(transparent)]
    FileIo(#[from] fileio::Error),
    /// A file could not be parsed, or a value in it is invalid.
    #[error(transparent)]
    Serialization(#[from] Error),
    /// A version file does not contain the version the migration upgrades
    /// from (upstream `LogicError`).
    #[error(
        "Unexpected file format version:\nExpected v{expected}, found v{found}.\nFile: '{}'",
        .file.as_ref().map(FilePath::to_native).unwrap_or_default()
    )]
    UnexpectedFileFormat {
        /// The version the migration upgrades from.
        expected: Version,
        /// The version found in the file.
        found: Version,
        /// The version file.
        file: Option<FilePath>,
    },
    /// A schematic symbol refers to a component instance which does not
    /// exist.
    #[error("Failed to find component instance '{0}'.{CONTACT_US}")]
    ComponentInstanceNotFound(Uuid),
    /// A component instance refers to a library component which does not
    /// exist.
    #[error(
        "Failed to find component '{0}'. This looks like a bug, please contact us.{CONTACT_US}"
    )]
    ComponentNotFound(Uuid),
    /// A component instance refers to a symbol variant which does not exist.
    #[error("Failed to find component symbol variant '{0}'.{CONTACT_US}")]
    SymbolVariantNotFound(Uuid),
    /// A schematic symbol refers to a gate which does not exist.
    #[error("Failed to find gate '{0}'.{CONTACT_US}")]
    GateNotFound(Uuid),
    /// A gate refers to a symbol which does not exist.
    #[error("Failed to find symbol '{0}'.{CONTACT_US}")]
    SymbolNotFound(Uuid),
    /// The library element type is unknown to the migrations (a
    /// [`LibraryBaseElement`](crate::library::LibraryBaseElement)
    /// implementation outside of this crate).
    #[error("Unknown library element type: '{0}'")]
    UnknownElementType(&'static str),
}

/// Severity of a [`MigrationMessage`] (ordered by importance).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MigrationSeverity {
    /// Informational note.
    Note,
    /// Something the user should review.
    Warning,
    /// Something which may have broken the design.
    Critical,
}

impl MigrationSeverity {
    /// Returns the translated severity label (upstream
    /// `Message::getSeverityStrTr()`).
    pub fn to_tr_string(self) -> String {
        match self {
            Self::Note => tr!("librepcb::FileFormatMigration", "NOTE"),
            Self::Warning => tr!("librepcb::FileFormatMigration", "WARNING"),
            Self::Critical => tr!("librepcb::FileFormatMigration", "CRITICAL"),
        }
    }
}

/// A message emitted by a project migration, shown to the user in the
/// migration log (upstream `FileFormatMigration::Message`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationMessage {
    /// File format version the emitting migration upgrades from.
    pub from_version: Version,
    /// File format version the emitting migration upgrades to.
    pub to_version: Version,
    /// Severity.
    pub severity: MigrationSeverity,
    /// Number of affected items, if known.
    pub affected_items: Option<usize>,
    /// The (translated) message.
    pub message: String,
}

/// A file format migration from one file format version to the next
/// (upstream `FileFormatMigration`).
///
/// Every method upgrades the files in `dir` in place (in the transactional
/// file system); errors leave the directory partially upgraded.
pub trait FileFormatMigration: fmt::Debug + Send + Sync {
    /// Returns the file format version this migration upgrades from.
    #[allow(clippy::wrong_self_convention)] // A getter (upstream `getFromVersion()`).
    fn from_version(&self) -> &Version;
    /// Returns the file format version this migration upgrades to.
    fn to_version(&self) -> &Version;

    /// Upgrades a component category directory.
    fn upgrade_component_category(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()>;
    /// Upgrades a package category directory.
    fn upgrade_package_category(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()>;
    /// Upgrades a symbol directory.
    fn upgrade_symbol(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()>;
    /// Upgrades a package directory.
    fn upgrade_package(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()>;
    /// Upgrades a component directory.
    fn upgrade_component(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()>;
    /// Upgrades a device directory.
    fn upgrade_device(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()>;
    /// Upgrades an organization directory.
    fn upgrade_organization(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()>;
    /// Upgrades a library directory (only the library itself, not the
    /// elements it contains).
    fn upgrade_library(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()>;
    /// Upgrades a project directory (including its library), appending the
    /// messages for the user to `messages`.
    fn upgrade_project(
        &self,
        dir: &mut TransactionalDirectory,
        messages: &mut Vec<MigrationMessage>,
    ) -> MigrationResult<()>;
    /// Upgrades a workspace data directory.
    fn upgrade_workspace_data(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()>;
}

/// Whether previous unstable file format releases shall be (partially)
/// upgraded, i.e. whether the environment variable
/// `LIBREPCB_UPGRADE_UNSTABLE` is `1` (read once, like upstream).
static UPGRADE_UNSTABLE: LazyLock<bool> =
    LazyLock::new(|| std::env::var_os("LIBREPCB_UPGRADE_UNSTABLE").is_some_and(|v| v == "1"));

/// Returns the migrations needed to upgrade files in the file format
/// `file_format` to the current file format, in the order to apply them
/// (upstream `FileFormatMigration::getMigrations()`). Empty if no upgrade
/// is needed.
pub fn file_format_migrations(file_format: &Version) -> Vec<Box<dyn FileFormatMigration>> {
    let mut migrations: Vec<Box<dyn FileFormatMigration>> = Vec::new();
    if *file_format <= version("0.1") {
        migrations.push(Box::new(V01Migration::new()));
    }
    if *file_format <= version("1") {
        migrations.push(Box::new(V1Migration::new()));
    }
    // Allow partially upgrading previous unstable file format releases if the
    // LIBREPCB_UPGRADE_UNSTABLE environment variable is set to 1.
    if *UPGRADE_UNSTABLE && (*file_format == application::file_format_version()) {
        migrations.push(Box::new(UnstableMigration::new()));
    }
    migrations
}

/// Parses a version number literal.
fn version(s: &str) -> Version {
    // Invariant: only called with valid literals.
    s.parse().expect("valid version literal")
}

/// Builds a message and logs it (upstream `buildMessage()`).
fn build_message(
    migration: &dyn FileFormatMigration,
    severity: MigrationSeverity,
    message: String,
    affected_items: Option<usize>,
) -> MigrationMessage {
    let multiplier = match affected_items {
        Some(n) if n > 0 => format!(" ({n}x)"),
        _ => String::new(),
    };
    log::info!("UPGRADE {}{multiplier}: {message}", severity.to_tr_string());
    MigrationMessage {
        from_version: migration.from_version().clone(),
        to_version: migration.to_version().clone(),
        severity,
        affected_items,
        message,
    }
}

/// Checks the version file `file_name` for the version the migration
/// upgrades from and replaces it by the version it upgrades to (upstream
/// `upgradeVersionFile()`).
fn upgrade_version_file(
    migration: &dyn FileFormatMigration,
    dir: &mut TransactionalDirectory,
    file_name: &str,
) -> MigrationResult<()> {
    let current = VersionFile::from_bytes(&dir.read(file_name)?)?;
    if current.version() != migration.from_version() {
        return Err(MigrationError::UnexpectedFileFormat {
            expected: migration.from_version().clone(),
            found: current.version().clone(),
            file: dir.abs_path(file_name),
        });
    }
    dir.write(
        file_name,
        &VersionFile::new(migration.to_version().clone()).to_bytes(),
    )?;
    Ok(())
}

/// Parses the file `path` of `dir`.
fn read_file(dir: &TransactionalDirectory, path: &str) -> MigrationResult<SExpression> {
    let file_path = dir.abs_path(path);
    Ok(SExpression::parse(
        &dir.read(path)?,
        file_path.as_ref().map(FilePath::as_path),
        Mode::LibrePcb,
    )?)
}

/// Writes `root` into the file `path` of `dir`.
fn write_file(
    dir: &mut TransactionalDirectory,
    path: &str,
    root: &SExpression,
) -> MigrationResult<()> {
    dir.write(path, &root.to_byte_array(Mode::LibrePcb)?)?;
    Ok(())
}

/// Parses the file `path` of `dir`, lets `upgrade` modify it and writes it
/// back.
fn upgrade_file(
    dir: &mut TransactionalDirectory,
    path: &str,
    upgrade: impl FnOnce(&mut SExpression) -> MigrationResult<()>,
) -> MigrationResult<()> {
    let mut root = read_file(dir, path)?;
    upgrade(&mut root)?;
    write_file(dir, path, &root)
}

/// Returns the subdirectories of `path` in `dir` which contain the version
/// file `version_file` (the library elements of a project library).
fn element_dirs(
    dir: &mut TransactionalDirectory,
    path: &str,
    version_file: &str,
) -> Vec<TransactionalDirectory> {
    dir.dirs(path)
        .into_iter()
        .map(|name| dir.subdir(&format!("{path}/{name}")))
        .filter(|sub| sub.file_exists(version_file))
        .collect()
}

/// Returns the relative file paths listed in an index file (e.g.
/// `(schematic "schematics/main/schematic.lp")` in `schematics.lp`).
fn index_file_entries(
    dir: &TransactionalDirectory,
    path: &str,
    tag: &str,
) -> MigrationResult<Vec<String>> {
    let root = read_file(dir, path)?;
    root.children_named(tag)
        .map(|child| Ok(child.required_child("@0")?.value()?.to_owned()))
        .collect()
}

// --- Tree manipulation helpers (upstream `SExpression` API) ---------------

/// Returns the list of a node, or an error if it is not a list.
fn as_list(node: &mut SExpression) -> MigrationResult<&mut List> {
    node.as_list_mut()
        .ok_or_else(|| Error::parse(ParseError::NotAList, "").into())
}

/// Returns a (nested) child by path (upstream `getChild()` for
/// modification).
fn child_mut<'a>(node: &'a mut SExpression, path: &str) -> MigrationResult<&'a mut SExpression> {
    node.child_mut(path)
        .ok_or_else(|| Error::parse(ParseError::ChildNotFound(path.to_owned()), "").into())
}

/// Returns the value of the (nested) child `path` (upstream
/// `getChild(path).getValue()`).
fn child_str<'a>(node: &'a SExpression, path: &str) -> MigrationResult<&'a str> {
    Ok(node.required_child(path)?.value()?)
}

/// Returns all direct children which are lists named `name`, for
/// modification (upstream `getChildren(name)`).
fn children_mut<'a>(
    node: &'a mut SExpression,
    name: &'a str,
) -> impl Iterator<Item = &'a mut SExpression> + 'a {
    node.as_list_mut()
        .into_iter()
        .flat_map(|list| list.children_mut().iter_mut())
        .filter(move |c| c.as_list().is_some_and(|l| l.name() == name))
}

/// Returns the number of direct children which are lists named `name`.
fn count_children(node: &SExpression, name: &str) -> usize {
    node.children_named(name).count()
}

/// Removes the first direct child list named `name` (upstream
/// `removeChild(getChild(name))`), or returns an error if there is none.
fn remove_child(node: &mut SExpression, name: &str) -> MigrationResult<SExpression> {
    let list = as_list(node)?;
    let index = list
        .children()
        .iter()
        .position(|c| c.as_list().is_some_and(|l| l.name() == name))
        .ok_or_else(|| Error::parse(ParseError::ChildNotFound(name.to_owned()), ""))?;
    Ok(list.children_mut().remove(index))
}

/// Removes all direct child lists named `name`, returning how many were
/// removed.
fn remove_children(node: &mut SExpression, name: &str) -> usize {
    let Some(list) = node.as_list_mut() else {
        return 0;
    };
    let before = list.children().len();
    list.children_mut()
        .retain(|c| c.as_list().is_none_or(|l| l.name() != name));
    before - list.children().len()
}

/// Sets the name of the first direct child list named `name`.
fn rename_child(node: &mut SExpression, name: &str, new_name: &str) -> MigrationResult<()> {
    as_list(child_mut(node, name)?)?.set_name(new_name);
    Ok(())
}

/// Replaces the value of the (nested) child `path`.
fn set_child_value(node: &mut SExpression, path: &str, value: &str) -> MigrationResult<()> {
    Ok(child_mut(node, path)?.set_value(value)?)
}

/// Replaces the (nested) child `path` by another node (upstream
/// `getChild(path) = *node`).
fn replace_child(node: &mut SExpression, path: &str, new: SExpression) -> MigrationResult<()> {
    *child_mut(node, path)? = new;
    Ok(())
}

/// Appends `(name <token>)` (upstream
/// `appendChild(name, SExpression::createToken(token))`).
fn append_token(list: &mut List, name: &str, token: &str) {
    list.append_list(name).push(SExpression::token(token));
}

/// Appends `(name "<string>")` (upstream `appendChild(name, QString)`).
fn append_string(list: &mut List, name: &str, string: &str) {
    list.append_list(name).push(SExpression::string(string));
}

/// Renames all tokens `search` to `replace`, recursively.
fn replace_token(node: &mut SExpression, search: &str, replace: &str) {
    node.replace_recursive(&SExpression::token(search), &SExpression::token(replace));
}

/// Removes legacy files (like library caches) in the directory `libraries`
/// of the workspace data directory, whose base name (up to the first dot)
/// is one of `names`.
fn remove_legacy_workspace_files(
    dir: &mut TransactionalDirectory,
    names: &[&str],
) -> MigrationResult<()> {
    let mut libraries_dir = dir.subdir("libraries");
    for file_name in libraries_dir.files("") {
        let base_name = file_name.split('.').next().unwrap_or_default();
        if names.contains(&base_name) {
            log::info!(
                "Removing legacy file: {}",
                libraries_dir
                    .abs_path(&file_name)
                    .map(|p| p.to_native())
                    .unwrap_or_default()
            );
            libraries_dir.remove_file(&file_name)?;
        }
    }
    Ok(())
}
