//! Port of libs/librepcb/core/workspace/workspacelibraryscanner.{h,cpp}.
//!
//! Scans all libraries of the workspace (`local` and `remote` subdirectories
//! of the libraries directory) and writes their elements into the library
//! database.
//!
//! Differences to upstream:
//! - The scanner does not own a thread (upstream: a `QThread` with a state
//!   machine). [`LibraryScanner::scan()`] runs synchronously on the calling
//!   thread; run it on a thread of your choice (the scanner is `Send`).
//!   Cancellation is an [`AtomicBool`] checked at the same points as
//!   upstream's `abortRequested()`; signals are replaced by a callback
//!   receiving [`ScanEvent`]s, and the result (upstream `scanSucceeded()` /
//!   `scanFailed()`) is the return value.
//! - Libraries are scanned in the order of their directory names (compared
//!   case insensitively, like `QDir`'s default sorting).
//! - Output jobs of organizations are read from their raw S-expressions
//!   (the `job` module is not ported yet).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::ElementKind;
use super::error::Result;
use super::library_db_writer::{LibraryDbWriter, OutputJobKind};
use crate::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem, file_utils};
use crate::library::cat::{ComponentCategory, PackageCategory};
use crate::library::cmp::Component;
use crate::library::dev::Device;
use crate::library::org::Organization;
use crate::library::pkg::Package;
use crate::library::sym::Symbol;
use crate::library::{BaseMetadata, Library, LibraryBaseElement, LibraryElement, read_file_format};
use crate::serialization::file_format_migrations;
use crate::sqlite_database::SqliteDatabase;

/// Progress events of a scan (upstream scanner signals).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanEvent {
    /// The scan started (upstream `scanStarted()`).
    Started,
    /// The libraries have been indexed (upstream
    /// `scanLibraryListUpdated()`).
    LibraryListUpdated {
        /// Number of libraries found.
        library_count: usize,
    },
    /// Progress in percent (upstream `scanProgressUpdate()`); 100 when the
    /// scan is finished (successfully, aborted or failed).
    Progress(u8),
}

/// Result of a completed scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanOutcome {
    /// The scan succeeded and the database was updated (upstream
    /// `scanSucceeded()`).
    Succeeded {
        /// Number of scanned library elements (excluding libraries).
        element_count: usize,
    },
    /// The scan was aborted; the elements in the database are unchanged
    /// (only the library list may have been updated).
    Aborted,
}

/// Scans the workspace libraries into the library database (see the
/// [module docs](self)).
#[derive(Debug, Clone)]
pub struct LibraryScanner {
    libraries_path: FilePath,
    db_file_path: FilePath,
}

static_assertions::assert_impl_all!(LibraryScanner: Send, Sync);

impl LibraryScanner {
    /// Creates a scanner for the libraries directory `libraries_path` and
    /// the database file `db_file_path`.
    pub fn new(libraries_path: &FilePath, db_file_path: &FilePath) -> Self {
        Self {
            libraries_path: libraries_path.clone(),
            db_file_path: db_file_path.clone(),
        }
    }

    /// Scans all libraries and updates the database (upstream `scan()`).
    ///
    /// Elements which cannot be opened are skipped with a warning.
    /// Libraries and elements in an outdated file format are upgraded on
    /// disk. Setting `abort` cancels the scan as soon as possible.
    pub fn scan(
        &self,
        abort: &AtomicBool,
        on_event: &mut dyn FnMut(ScanEvent),
    ) -> Result<ScanOutcome> {
        let start = std::time::Instant::now();
        on_event(ScanEvent::Started);
        on_event(ScanEvent::Progress(0));
        log::debug!("Start workspace library scan...");
        let result = self.scan_impl(abort, on_event);
        match &result {
            Ok(ScanOutcome::Succeeded { element_count }) => log::debug!(
                "Workspace library scan succeeded: {element_count} elements in {} ms.",
                start.elapsed().as_millis()
            ),
            Ok(ScanOutcome::Aborted) => log::debug!(
                "Workspace library scan aborted after {} ms.",
                start.elapsed().as_millis()
            ),
            Err(e) => log::debug!("Workspace library scan failed: {e}"),
        }
        on_event(ScanEvent::Progress(100));
        result
    }

    fn scan_impl(
        &self,
        abort: &AtomicBool,
        on_event: &mut dyn FnMut(ScanEvent),
    ) -> Result<ScanOutcome> {
        let aborted = || abort.load(Ordering::Relaxed);

        // Open SQLite database.
        let db = SqliteDatabase::open(self.db_file_path.as_path())?;
        let writer = LibraryDbWriter::new(&self.libraries_path, &db);

        // Update list of libraries.
        let mut libraries = self.libraries_of_directory("local");
        libraries.extend(self.libraries_of_directory("remote"));
        let lib_ids = self.update_libraries(&db, &writer, &libraries)?;
        on_event(ScanEvent::LibraryListUpdated {
            library_count: lib_ids.len(),
        });
        on_event(ScanEvent::Progress(1));

        // Begin database transaction.
        let transaction = db.transaction()?;

        // Clear all tables.
        for kind in [
            ElementKind::ComponentCategory,
            ElementKind::PackageCategory,
            ElementKind::Symbol,
            ElementKind::Package,
            ElementKind::Component,
            ElementKind::Device,
            ElementKind::Organization,
        ] {
            writer.remove_all_elements(kind)?;
        }

        // Scan all libraries.
        let mut count = 0;
        let mut percent = 1.0_f64;
        let fraction = (libraries.len() * 7) as f64;
        let mut step = |on_event: &mut dyn FnMut(ScanEvent)| {
            percent += 98.0 / fraction;
            // Truncation like upstream's qreal -> int conversion.
            on_event(ScanEvent::Progress(percent.clamp(0.0, 100.0) as u8));
        };
        'libs: for (fp, lib) in &libraries {
            let Some(&lib_id) = lib_ids.iter().find(|(p, _)| p == fp).map(|(_, id)| id) else {
                continue; // Cannot happen, all libraries have been added.
            };
            macro_rules! scan_kind {
                ($ty:ty) => {
                    if aborted() {
                        break 'libs;
                    }
                    count += self.add_elements_to_db::<$ty>(
                        &writer,
                        fp,
                        &lib.search_for_elements::<$ty>(),
                        lib_id,
                        abort,
                    );
                    step(on_event);
                };
            }
            scan_kind!(ComponentCategory);
            scan_kind!(PackageCategory);
            scan_kind!(Symbol);
            scan_kind!(Package);
            scan_kind!(Component);
            scan_kind!(Device);
            scan_kind!(Organization);
        }

        // Commit transaction.
        if aborted() {
            Ok(ScanOutcome::Aborted)
        } else {
            transaction.commit()?;
            Ok(ScanOutcome::Succeeded {
                element_count: count,
            })
        }
    }

    /// Opens all libraries in a subdirectory of the libraries directory
    /// (upstream `getLibrariesOfDirectory()`).
    fn libraries_of_directory(&self, root: &str) -> Vec<(FilePath, Library)> {
        let mut dirs = file_utils::find_directories(&self.libraries_path.path_to(root));
        dirs.sort_by(|a, b| {
            a.file_name()
                .to_lowercase()
                .cmp(&b.file_name().to_lowercase())
                .then_with(|| a.cmp(b))
        });
        let mut libs = Vec::new();
        for fp in dirs {
            if Library::is_valid_element_directory_path(&fp) {
                match open_and_migrate::<Library>(&fp) {
                    Ok(lib) => libs.push((fp, lib)),
                    Err(e) => {
                        log::error!("Could not open workspace library {}: {e}", fp.to_native())
                    }
                }
            } else if !fp.is_empty_dir() {
                log::warn!(
                    "Directory is not a valid library, ignoring it: {}",
                    fp.to_native()
                );
            }
        }
        libs
    }

    /// Updates the libraries table and the library translations, and
    /// returns the IDs of all libraries (upstream `updateLibraries()`).
    fn update_libraries(
        &self,
        db: &SqliteDatabase,
        writer: &LibraryDbWriter<'_>,
        libs: &[(FilePath, Library)],
    ) -> Result<Vec<(FilePath, i64)>> {
        let transaction = db.transaction()?;

        // Get IDs of existing libraries in DB.
        let mut db_lib_ids: Vec<(FilePath, i64)> = {
            let mut query = db.prepare("SELECT id, filepath FROM libraries", &[])?;
            let rows = query.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut ids = Vec::new();
            for row in rows {
                let (id, filepath) = row?;
                ids.push((self.libraries_path.path_to(&filepath), id));
            }
            ids
        };

        // Update existing and add new libraries to DB.
        for (fp, lib) in libs {
            let meta = lib.metadata();
            if db_lib_ids.iter().any(|(p, _)| p == fp) {
                writer.update_library(
                    fp,
                    meta.uuid(),
                    meta.version(),
                    meta.is_deprecated(),
                    lib.icon(),
                    lib.manufacturer().as_str(),
                )?;
            } else {
                let id = writer.add_library(
                    fp,
                    meta.uuid(),
                    meta.version(),
                    meta.is_deprecated(),
                    lib.icon(),
                    lib.manufacturer().as_str(),
                )?;
                db_lib_ids.push((fp.clone(), id));
            }
        }

        // Remove no longer existing libraries from DB.
        let mut removed = Vec::new();
        db_lib_ids.retain(|(p, _)| {
            let exists = libs.iter().any(|(fp, _)| fp == p);
            if !exists {
                removed.push(p.clone());
            }
            exists
        });
        for fp in &removed {
            writer.remove_element(ElementKind::Library, fp)?;
        }

        // Update all library translations.
        writer.remove_all_translations(ElementKind::Library)?;
        for (fp, lib) in libs {
            if let Some((_, id)) = db_lib_ids.iter().find(|(p, _)| p == fp) {
                add_translations(writer, ElementKind::Library, *id, lib.metadata())?;
            }
        }

        transaction.commit()?;
        Ok(db_lib_ids)
    }

    /// Adds all elements of a kind of a library, returns the number of
    /// added elements (upstream `addElementsToDb()`).
    fn add_elements_to_db<E: ScanElement>(
        &self,
        writer: &LibraryDbWriter<'_>,
        lib_path: &FilePath,
        dirs: &[String],
        lib_id: i64,
        abort: &AtomicBool,
    ) -> usize {
        let mut count = 0;
        for dir in dirs {
            if count % 20 == 19 && abort.load(Ordering::Relaxed) {
                break;
            }
            let fp = lib_path.path_to(dir);
            let result = open_and_migrate::<E>(&fp).and_then(|element| {
                let id = element.add_to_db(writer, lib_id, &fp)?;
                add_translations(writer, E::KIND, id, element.metadata())
            });
            match result {
                Ok(()) => count += 1,
                Err(e) => log::warn!(
                    "Failed to open library element during scan: {}: {e}",
                    fp.to_native()
                ),
            }
        }
        count
    }
}

/// Opens a library or library element (upstream `openAndMigrate()`): if a
/// file format migration is required, it is opened read/write and saved
/// after the upgrade to avoid the overhead the next time.
fn open_and_migrate<E: LibraryBaseElement>(fp: &FilePath) -> Result<E> {
    let fs = Arc::new(TransactionalFileSystem::open_ro(fp)?);
    let dir = TransactionalDirectory::new(fs, "");
    let version_file = format!(".librepcb-{}", E::SHORT_ELEMENT_NAME);
    let file_format = read_file_format(&dir, &version_file)?;
    if file_format_migrations(&file_format).is_empty() {
        return Ok(E::open(dir)?);
    }
    drop(dir);
    let fs = Arc::new(TransactionalFileSystem::open_rw(fp)?);
    let element = E::open(TransactionalDirectory::new(Arc::clone(&fs), ""))?;
    fs.save()?;
    fs.release_lock()?;
    Ok(element)
}

/// Adds the translations of an element (upstream `addTranslationsToDb()`).
fn add_translations(
    writer: &LibraryDbWriter<'_>,
    kind: ElementKind,
    element_id: i64,
    metadata: &BaseMetadata,
) -> Result<()> {
    for locale in metadata.all_available_locales() {
        writer.add_translation(
            kind,
            element_id,
            &locale,
            metadata.names().get(&locale).map(|n| n.as_str()),
            metadata.descriptions().get(&locale).map(String::as_str),
            metadata.keywords().get(&locale).map(String::as_str),
        )?;
    }
    Ok(())
}

/// Adds the categories of an element (upstream `addToCategories()`).
fn add_to_categories<E: LibraryElement>(
    writer: &LibraryDbWriter<'_>,
    kind: ElementKind,
    element_id: i64,
    element: &E,
) -> Result<()> {
    for category in element.element_metadata().categories() {
        writer.add_to_category(kind, element_id, *category)?;
    }
    Ok(())
}

/// Adds the resources of an element (upstream `addResourcesToDb()`).
fn add_resources<E: LibraryElement>(
    writer: &LibraryDbWriter<'_>,
    kind: ElementKind,
    element_id: i64,
    element: &E,
) -> Result<()> {
    for resource in element.element_metadata().resources() {
        writer.add_resource(
            kind,
            element_id,
            resource.name().as_str(),
            resource.media_type(),
            resource.url(),
        )?;
    }
    Ok(())
}

/// Adds a generic library element (symbol, package, component) with its
/// categories.
fn add_generic_element<E: LibraryElement>(
    writer: &LibraryDbWriter<'_>,
    kind: ElementKind,
    lib_id: i64,
    fp: &FilePath,
    element: &E,
) -> Result<i64> {
    let meta = element.metadata();
    let id = writer.add_element(
        kind,
        lib_id,
        fp,
        meta.uuid(),
        meta.version(),
        meta.is_deprecated(),
        element.element_metadata().generated_by(),
    )?;
    add_to_categories(writer, kind, id, element)?;
    Ok(id)
}

/// Element types which can be scanned (upstream `addElementToDb<T>()`
/// specializations).
trait ScanElement: LibraryBaseElement {
    const KIND: ElementKind;

    /// Adds the element (without translations) and returns its ID.
    fn add_to_db(&self, writer: &LibraryDbWriter<'_>, lib_id: i64, fp: &FilePath) -> Result<i64>;
}

impl ScanElement for ComponentCategory {
    const KIND: ElementKind = ElementKind::ComponentCategory;

    fn add_to_db(&self, writer: &LibraryDbWriter<'_>, lib_id: i64, fp: &FilePath) -> Result<i64> {
        let meta = self.metadata();
        writer.add_category(
            Self::KIND,
            lib_id,
            fp,
            meta.uuid(),
            meta.version(),
            meta.is_deprecated(),
            self.parent_uuid(),
        )
    }
}

impl ScanElement for PackageCategory {
    const KIND: ElementKind = ElementKind::PackageCategory;

    fn add_to_db(&self, writer: &LibraryDbWriter<'_>, lib_id: i64, fp: &FilePath) -> Result<i64> {
        let meta = self.metadata();
        writer.add_category(
            Self::KIND,
            lib_id,
            fp,
            meta.uuid(),
            meta.version(),
            meta.is_deprecated(),
            self.parent_uuid(),
        )
    }
}

impl ScanElement for Symbol {
    const KIND: ElementKind = ElementKind::Symbol;

    fn add_to_db(&self, writer: &LibraryDbWriter<'_>, lib_id: i64, fp: &FilePath) -> Result<i64> {
        add_generic_element(writer, Self::KIND, lib_id, fp, self)
    }
}

impl ScanElement for Package {
    const KIND: ElementKind = ElementKind::Package;

    fn add_to_db(&self, writer: &LibraryDbWriter<'_>, lib_id: i64, fp: &FilePath) -> Result<i64> {
        let id = add_generic_element(writer, Self::KIND, lib_id, fp, self)?;
        for name in self.alternative_names() {
            writer.add_alternative_name(id, name.name.as_str(), name.reference.as_str())?;
        }
        Ok(id)
    }
}

impl ScanElement for Component {
    const KIND: ElementKind = ElementKind::Component;

    fn add_to_db(&self, writer: &LibraryDbWriter<'_>, lib_id: i64, fp: &FilePath) -> Result<i64> {
        let id = add_generic_element(writer, Self::KIND, lib_id, fp, self)?;
        add_resources(writer, Self::KIND, id, self)?;
        Ok(id)
    }
}

impl ScanElement for Device {
    const KIND: ElementKind = ElementKind::Device;

    fn add_to_db(&self, writer: &LibraryDbWriter<'_>, lib_id: i64, fp: &FilePath) -> Result<i64> {
        let meta = self.metadata();
        let id = writer.add_device(
            lib_id,
            fp,
            meta.uuid(),
            meta.version(),
            meta.is_deprecated(),
            self.element_metadata().generated_by(),
            self.component_uuid(),
            self.package_uuid(),
        )?;
        add_to_categories(writer, Self::KIND, id, self)?;
        add_resources(writer, Self::KIND, id, self)?;
        for part in self.parts() {
            if !part.is_empty() {
                let part_id =
                    writer.add_part(id, part.mpn().as_str(), part.manufacturer().as_str())?;
                for attribute in part.attributes() {
                    writer.add_part_attribute(part_id, attribute)?;
                }
            }
        }
        Ok(id)
    }
}

impl ScanElement for Organization {
    const KIND: ElementKind = ElementKind::Organization;

    fn add_to_db(&self, writer: &LibraryDbWriter<'_>, lib_id: i64, fp: &FilePath) -> Result<i64> {
        let meta = self.metadata();
        let id = writer.add_organization(
            lib_id,
            fp,
            meta.uuid(),
            meta.version(),
            meta.is_deprecated(),
            self.logo_png(),
            self.url(),
            self.country(),
            self.fabs(),
            self.shipping(),
            self.is_sponsor(),
            self.priority(),
        )?;
        for rules in self.pcb_design_rules() {
            writer.add_organization_pcb_design_rules(
                id,
                rules.uuid(),
                rules.names().default_value().as_str(),
                rules.descriptions().default_value(),
                rules.url(),
                i64::from(rules.drc_settings(false).max_layer_count()),
            )?;
        }
        for (kind, jobs) in [
            (OutputJobKind::Pcb, self.pcb_output_jobs()),
            (OutputJobKind::Assembly, self.assembly_output_jobs()),
            (OutputJobKind::User, self.user_output_jobs()),
        ] {
            for job in jobs {
                writer.add_organization_output_job(
                    id,
                    kind,
                    job.uuid(),
                    job.type_name(),
                    job.name().as_str(),
                )?;
            }
        }
        Ok(id)
    }
}
