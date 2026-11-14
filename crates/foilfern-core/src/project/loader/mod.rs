//! Port of libs/librepcb/core/project/projectloader.{h,cpp}.
//!
//! Loads a project in the same order as upstream: version check, file
//! format migrations, metadata, settings, output jobs, library, circuit,
//! ERC approvals, schematics (index file order), boards (index file
//! order), user settings. Loading goes through the same validating
//! insertion functions as editing, so the error messages are upstream's;
//! the change journal is cleared afterwards.
//! After a file format migration, the ERC is run and only the approvals of
//! messages which still occur are kept (like upstream).

mod board;
mod circuit;
mod migration_log;
mod schematic;

use std::collections::BTreeSet;

use chrono::Local;

pub use migration_log::MigrationLog;

use super::Project;
use super::error::{Error, Result};
use super::library::{LibraryElementKind, ProjectLibrary};
use super::ref_index::RefIndex;
use crate::application;
use crate::attribute::AttributeList;
use crate::fileio::{FilePath, FileSystem, TransactionalDirectory, VersionFile};
use crate::job::OutputJobList;
use crate::library::LibraryBaseElement;
use crate::serialization::{DeserializeObject, Mode, SExpression, file_format_migrations};
use crate::types::Version;

/// Loads projects, see the module documentation.
#[derive(Debug, Default)]
pub struct ProjectLoader {
    application_version: String,
    migration_log: Option<MigrationLog>,
    auto_assign_device_models: bool,
}

impl ProjectLoader {
    /// Creates a loader with default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether board devices without a valid 3D model get the default model
    /// of their footprint (upstream `setAutoAssignDeviceModels()`, used
    /// after upgrading the project library). Off by default.
    pub fn set_auto_assign_device_models(&mut self, auto_assign: bool) {
        self.auto_assign_device_models = auto_assign;
    }

    /// Sets the application version shown in the migration log (upstream
    /// `Application::getVersion()`; empty by default).
    pub fn set_application_version(&mut self, version: impl Into<String>) {
        self.application_version = version.into();
    }

    /// Returns the log of the file format migration performed by the last
    /// [`open()`](Self::open), or `None` if the project was already in the
    /// current file format.
    pub fn migration_log(&self) -> Option<&MigrationLog> {
        self.migration_log.as_ref()
    }

    /// Opens the project `file_name` (`*.lpp`) in `directory`.
    ///
    /// Projects in an older file format are upgraded (in the transactional
    /// file system, i.e. not on disk yet) and saved into `directory`,
    /// including a migration log in `logs/` (see
    /// [`migration_log()`](Self::migration_log)).
    pub fn open(&mut self, directory: TransactionalDirectory, file_name: &str) -> Result<Project> {
        self.migration_log = None;
        let file_path = || {
            directory
                .abs_path(file_name)
                .unwrap_or_else(|| FilePath::new("/").expect("valid path"))
        };
        if !directory.file_exists(file_name) {
            return Err(Error::ProjectFileNotFound(file_path()));
        }
        if !directory.file_exists(".librepcb-project") {
            return Err(Error::NotAProjectDirectory(directory.abs_path("")));
        }
        let file_format = VersionFile::from_bytes(&directory.read(".librepcb-project")?)?
            .version()
            .clone();
        log::debug!("Detected project file format: {file_format}");
        let current = application::file_format_version();
        if file_format > current {
            return Err(Error::NewerFileFormat {
                version: file_format,
                path: file_path(),
            });
        }
        let mut directory = directory;
        self.migration_log = self.upgrade_file_format(&mut directory, file_name, file_format)?;
        let mut p = Project::new(directory, file_name, crate::types::Uuid::new_random())?;
        load_metadata(&mut p)?;
        load_settings(&mut p)?;
        load_output_jobs(&mut p)?;
        load_library(&mut p)?;
        circuit::load_circuit(&mut p)?;
        load_erc(&mut p)?;
        schematic::load_schematics(&mut p)?;
        board::load_boards(&mut p, self.auto_assign_device_models)?;
        load_project_user_settings(&p);
        p.refs = RefIndex::build(&p);

        if self.migration_log.is_some() {
            // Clean up obsolete ERC message approvals.
            log::info!("Running ERC to clean up obsolete message approvals...");
            let messages = super::erc::run_erc(&p);
            let approvals = crate::rule_check::all_approvals(messages.iter().map(|m| m.message()));
            p.erc_approvals.retain(|a| approvals.contains(a));

            // Make sure the files are formatted correctly. Also handle
            // possible errors during serialization now instead of later.
            p.save()?;
        }
        p.journal.reset();
        log::debug!("Successfully opened project.");
        Ok(p)
    }

    /// Runs the file format migrations needed to upgrade the project in
    /// `directory` from `file_format` to the current file format, and
    /// writes the migration log. Returns the log, or `None` if no migration
    /// was needed.
    fn upgrade_file_format(
        &self,
        directory: &mut TransactionalDirectory,
        file_name: &str,
        file_format: Version,
    ) -> Result<Option<MigrationLog>> {
        let mut migration_log: Option<MigrationLog> = None;
        for migration in file_format_migrations(&file_format) {
            let log = migration_log.get_or_insert_with(|| MigrationLog {
                project_name: file_name.to_owned(),
                date_time: Local::now(),
                from_version: file_format.clone(),
                to_version: application::file_format_version(),
                messages: Vec::new(),
            });
            log::info!(
                "Project file format is outdated, upgrading from v{} to v{}...",
                migration.from_version(),
                migration.to_version()
            );
            migration.upgrade_project(directory, &mut log.messages)?;
        }

        // Sort & save migration messages.
        if let Some(log) = &mut migration_log {
            // Make sure to delete the temporary migration log (may not even
            // exist *now*, but may exist at the time the project gets saved).
            directory.remove_file(&log.relative_file_path(true))?;
            // Save final migration log to file system, which gets saved to
            // disk as soon as the project gets saved.
            log.sort_messages();
            directory.write(
                &log.relative_file_path(false),
                log.to_html(false, &self.application_version).as_bytes(),
            )?;
        }
        Ok(migration_log)
    }
}

impl Project {
    /// Opens the project `file_name` in `directory` with the default loader
    /// options.
    pub fn open(directory: TransactionalDirectory, file_name: &str) -> Result<Self> {
        ProjectLoader::new().open(directory, file_name)
    }
}

/// Parses the file `path` of the project directory.
pub(super) fn parse_file(p: &Project, path: &str) -> Result<SExpression> {
    let content = p.directory.read(path)?;
    let file_path = p.directory.abs_path(path);
    Ok(SExpression::parse(
        &content,
        file_path.as_ref().map(FilePath::as_path),
        Mode::LibrePcb,
    )?)
}

fn load_metadata(p: &mut Project) -> Result<()> {
    let root = parse_file(p, "project/metadata.lp")?;
    p.uuid = root.child_value("@0")?;
    p.metadata.name = root.child_value("name/@0")?;
    p.metadata.author = root.child_value("author/@0")?;
    p.metadata.version = root.child_value("version/@0")?;
    p.metadata.created = root.child_value("created/@0")?;
    p.metadata.attributes = AttributeList::deserialize(&root)?;
    Ok(())
}

fn load_settings(p: &mut Project) -> Result<()> {
    let root = parse_file(p, "project/settings.lp")?;
    let strings = |list: &str, item: &str| -> Result<Vec<String>> {
        Ok(root
            .required_child(list)?
            .children_named(item)
            .map(|n| n.child_value("@0"))
            .collect::<crate::serialization::Result<_>>()?)
    };
    p.settings.locale_order = strings("library_locale_order", "locale")?;
    p.settings.norm_order = strings("library_norm_order", "norm")?;
    p.settings.custom_bom_attributes = strings("custom_bom_attributes", "attribute")?;
    p.settings.default_lock_component_assembly =
        root.child_value("default_lock_component_assembly/@0")?;
    Ok(())
}

fn load_output_jobs(p: &mut Project) -> Result<()> {
    let root = parse_file(p, "project/jobs.lp")?;
    p.output_jobs = OutputJobList::deserialize(&root)?;
    Ok(())
}

fn load_library(p: &mut Project) -> Result<()> {
    load_library_elements(p, LibraryElementKind::Symbol, ProjectLibrary::add_symbol)?;
    load_library_elements(p, LibraryElementKind::Package, ProjectLibrary::add_package)?;
    load_library_elements(
        p,
        LibraryElementKind::Component,
        ProjectLibrary::add_component,
    )?;
    load_library_elements(p, LibraryElementKind::Device, ProjectLibrary::add_device)?;
    Ok(())
}

/// Loads all elements of one kind from `library/<kind>/*/` (upstream
/// `loadLibraryElements()`); invalid directories are skipped with a warning.
fn load_library_elements<E: LibraryBaseElement>(
    p: &mut Project,
    kind: LibraryElementKind,
    add: fn(&mut ProjectLibrary, E) -> Result<()>,
) -> Result<()> {
    let dirname = kind.directory_name();
    let mut count = 0;
    for sub in p.library.directory().dirs(dirname) {
        let dir = p
            .library
            .directory_mut()
            .subdir(&format!("{dirname}/{sub}"));
        if !E::is_valid_element_directory(&dir, "") {
            log::warn!(
                "Invalid directory in project library, ignoring it: {}",
                dir.abs_path("").map(|p| p.to_native()).unwrap_or_default()
            );
            continue;
        }
        add(&mut p.library, E::open(dir)?)?;
        count += 1;
    }
    log::debug!("Successfully loaded {count} {dirname} elements.");
    Ok(())
}

fn load_erc(p: &mut Project) -> Result<()> {
    let root = parse_file(p, "circuit/erc.lp")?;
    p.erc_approvals = root
        .children_named("approved")
        .cloned()
        .collect::<BTreeSet<_>>();
    Ok(())
}

fn load_project_user_settings(p: &Project) {
    // upstream: parses `project/settings.user.lp` (currently without
    // content) and uses defaults on error.
    if let Err(e) = parse_file(p, "project/settings.user.lp") {
        log::error!("Could not load project user settings, defaults will be used instead: {e}");
    }
}

/// Splits an index file entry like `schematics/main/schematic.lp` into the
/// directory (`schematics/main`) and the directory name (`main`), like
/// upstream's `FilePath::getParentDir()`/`getFilename()`.
pub(super) fn split_index_path(relative: &str) -> (String, String) {
    let dir = relative.rsplit_once('/').map_or("", |(d, _)| d);
    let name = dir.rsplit_once('/').map_or(dir, |(_, n)| n);
    (dir.to_owned(), name.to_owned())
}
