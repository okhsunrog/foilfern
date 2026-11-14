//! Port of libs/librepcb/core/workspace/workspacelibrarydb.{h,cpp}.
//!
//! The workspace library database (`data/libraries/cache_v8.sqlite`) is an
//! SQLite index of all libraries in the workspace (`data/libraries/local`
//! and `data/libraries/remote`), filled by the [`LibraryScanner`]. It is
//! shared with upstream LibrePCB, so the schema, the file name and the
//! database version are identical to upstream.
//!
//! Differences to upstream:
//! - The element type template parameters are an [`ElementKind`] argument.
//! - Getters with output pointer parameters and a `bool` return value
//!   return an `Option` of a plain struct instead ([`Translations`],
//!   [`ElementInfo`], [`LibraryInfo`], [`CategoryInfo`], [`DeviceInfo`]).
//! - The scan does not run in an owned background thread: call
//!   [`LibraryDb::rescan()`] on a thread of your choice, or move a
//!   [`LibraryScanner`] (from [`LibraryDb::scanner()`]) to one. There are no
//!   scan signals; progress is reported through a callback.
//! - Icons and logos are returned as PNG file content (no `QPixmap`).
//! - Results which are `QSet`s upstream are sorted sets (`BTreeSet`).
//! - Added a facade for tools and agents (MCP): [`LibraryDb::search()`],
//!   [`LibraryDb::element_summary()`], [`LibraryDb::element_dir()`] and
//!   [`LibraryDb::category_tree()`].

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard, PoisonError};

use rusqlite::types::ToSql;
use rusqlite::{Row, named_params};

use super::ElementKind;
use super::error::{Error, Result};
use super::library_db_writer::{LibraryDbWriter, OutputJobKind};
use super::library_scanner::{LibraryScanner, ScanEvent, ScanOutcome};
use crate::attribute::{Attribute, AttributeKey, AttributeList, AttributeType};
use crate::fileio::FilePath;
use crate::library::dev::Part;
use crate::library::{Resource, ResourceList};
use crate::sqlite_database::SqliteDatabase;
use crate::types::{ElementName, SimpleString, Uuid, Version};

/// Version of the database schema (upstream `sCurrentDbVersion`), also part
/// of the file name.
pub const CURRENT_DB_VERSION: i64 = 8;

/// Localized texts of an element (upstream `getTranslations()` output
/// parameters). Missing values are empty strings.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Translations {
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Keywords (comma separated).
    pub keywords: String,
}

/// Metadata of an element (upstream `getMetadata()` output parameters).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ElementInfo {
    /// UUID.
    pub uuid: Uuid,
    /// Version.
    pub version: Version,
    /// Whether the element is deprecated.
    pub deprecated: bool,
}

/// Additional metadata of a library (upstream `getLibraryMetadata()`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryInfo {
    /// Icon (PNG file content, empty if there is no icon).
    pub icon_png: Vec<u8>,
    /// Manufacturer (may be empty).
    pub manufacturer: String,
}

/// Additional metadata of a category (upstream `getCategoryMetadata()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CategoryInfo {
    /// Parent category (`None` for root categories).
    pub parent: Option<Uuid>,
}

/// Additional metadata of a device (upstream `getDeviceMetadata()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct DeviceInfo {
    /// UUID of the component.
    pub component_uuid: Uuid,
    /// UUID of the package.
    pub package_uuid: Uuid,
}

/// PCB design rules of an organization (upstream
/// `WorkspaceLibraryDb::PcbDesignRules`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PcbDesignRulesInfo {
    /// UUID.
    pub uuid: Uuid,
    /// Name (default locale).
    pub name: String,
    /// Description (default locale).
    pub description: String,
    /// URL (may be empty).
    pub url: String,
    /// Maximum number of copper layers (0 = no restriction).
    pub max_layer_count: i64,
}

/// Output job of an organization (upstream `WorkspaceLibraryDb::OutputJob`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OutputJobInfo {
    /// Kind of the job.
    pub kind: OutputJobKind,
    /// UUID.
    pub uuid: Uuid,
    /// Job type (e.g. `"gerber_excellon"`).
    pub job_type: String,
    /// Name.
    pub name: String,
}

/// An organization (upstream `WorkspaceLibraryDb::Organization`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrganizationInfo {
    /// Directory of the organization element.
    pub directory: FilePath,
    /// UUID.
    pub uuid: Uuid,
    /// Name (translated).
    pub name: String,
    /// Description (translated).
    pub description: String,
    /// Version.
    pub version: Version,
    /// Logo (PNG file content, may be empty).
    pub logo_png: Vec<u8>,
    /// Website URL (may be empty).
    pub url: String,
    /// Country code.
    pub country: String,
    /// Countries of the fabs.
    pub fabs: Vec<String>,
    /// Shipping destinations.
    pub shipping: Vec<String>,
    /// Whether the organization sponsors LibrePCB.
    pub is_sponsor: bool,
    /// Sort priority (higher first).
    pub priority: i64,
    /// PCB design rules (only if requested).
    pub pcb_design_rules: Vec<PcbDesignRulesInfo>,
    /// Output jobs (only if requested).
    pub output_jobs: Vec<OutputJobInfo>,
}

/// Summary of a library element (latest version), for tools and agents.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ElementSummary {
    /// Element kind.
    pub kind: ElementKind,
    /// UUID.
    pub uuid: Uuid,
    /// Directory of the element (latest version).
    #[serde(serialize_with = "serialize_file_path")]
    pub directory: FilePath,
    /// Version.
    pub version: Version,
    /// Whether the element is deprecated.
    pub deprecated: bool,
    /// Name (translated).
    pub name: String,
    /// Description (translated).
    pub description: String,
    /// Keywords (translated).
    pub keywords: String,
    /// Categories (empty for kinds without categories).
    pub categories: BTreeSet<Uuid>,
    /// Component and package of a device (`None` for other kinds).
    pub device: Option<DeviceInfo>,
}

fn serialize_file_path<S: serde::Serializer>(
    fp: &FilePath,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&fp.to_native())
}

/// Parameters of [`LibraryDb::search()`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SearchQuery {
    /// Keyword to search in names and keywords of all locales (and in
    /// alternative names of packages); an element UUID matches exactly.
    pub keyword: String,
    /// Kinds to search (empty: symbols, packages, components and devices).
    pub kinds: Vec<ElementKind>,
    /// Whether devices are also found by the MPN or manufacturer of their
    /// parts.
    pub include_parts: bool,
    /// Locale order for the returned texts (highest priority first).
    pub locale_order: Vec<String>,
    /// Maximum number of results (`None`: unlimited).
    pub limit: Option<usize>,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            keyword: String::new(),
            kinds: Vec::new(),
            include_parts: true,
            locale_order: Vec::new(),
            limit: None,
        }
    }
}

impl SearchQuery {
    /// Creates a query for `keyword` with default options.
    pub fn new(keyword: impl Into<String>) -> Self {
        Self {
            keyword: keyword.into(),
            ..Self::default()
        }
    }
}

/// A node of a category tree (see [`LibraryDb::category_tree()`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CategoryTreeNode {
    /// UUID of the category.
    pub uuid: Uuid,
    /// Name (translated; empty if the category does not exist but is
    /// referenced as parent).
    pub name: String,
    /// Child categories, sorted by name.
    pub children: Vec<CategoryTreeNode>,
}

/// The workspace library database (see the [module docs](self)).
///
/// Thread safe: the connection is protected by a mutex. The scanner uses
/// its own connection (SQLite WAL mode allows concurrent reads while a scan
/// writes).
#[derive(Debug)]
pub struct LibraryDb {
    libraries_path: FilePath,
    file_path: FilePath,
    // `None` only transiently while the database file is recreated.
    db: Mutex<Option<SqliteDatabase>>,
}

static_assertions::assert_impl_all!(LibraryDb: Send, Sync);

impl LibraryDb {
    /// Opens (or creates) the database of the libraries directory
    /// `libraries_path` (`<workspace>/data/libraries`). If the database has
    /// another version, it is reinitialized (empty).
    pub fn open(libraries_path: &FilePath) -> Result<Self> {
        log::debug!("Load workspace library database...");
        let file_path = libraries_path.path_to(&format!("cache_v{CURRENT_DB_VERSION}.sqlite"));
        let db = SqliteDatabase::open(file_path.as_path())?;
        let this = Self {
            libraries_path: libraries_path.clone(),
            file_path,
            db: Mutex::new(Some(db)),
        };

        // Check database version - actually it must match the version in the
        // filename, but if not (e.g. due to a mistake by us) we just remove
        // the whole database and create a new one.
        let db_version = this.db_version();
        if db_version != Some(CURRENT_DB_VERSION) {
            log::warn!(
                "Library database version {db_version:?} is outdated or not supported, \
                 reinitializing..."
            );
            this.reset()?;
        }
        log::debug!("Successfully loaded workspace library database.");
        Ok(this)
    }

    /// Returns the path of the SQLite database file.
    pub fn file_path(&self) -> &FilePath {
        &self.file_path
    }

    /// Returns the libraries directory.
    pub fn libraries_path(&self) -> &FilePath {
        &self.libraries_path
    }

    /// Returns a scanner for this database, e.g. to move it to another
    /// thread.
    pub fn scanner(&self) -> LibraryScanner {
        LibraryScanner::new(&self.libraries_path, &self.file_path)
    }

    /// Rescans all libraries on the calling thread (upstream
    /// `startLibraryRescan()`); see [`LibraryScanner::scan()`].
    pub fn rescan(
        &self,
        abort: &AtomicBool,
        on_event: &mut dyn FnMut(ScanEvent),
    ) -> Result<ScanOutcome> {
        self.scanner().scan(abort, on_event)
    }

    /// Reinitializes the database and rescans all libraries (upstream
    /// `resetAndRescan()`).
    pub fn reset_and_rescan(
        &self,
        abort: &AtomicBool,
        on_event: &mut dyn FnMut(ScanEvent),
    ) -> Result<ScanOutcome> {
        self.reset()?;
        self.rescan(abort, on_event)
    }

    fn lock(&self) -> MutexGuard<'_, Option<SqliteDatabase>> {
        self.db.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `f` with the database connection.
    fn with_db<T>(&self, f: impl FnOnce(&SqliteDatabase) -> Result<T>) -> Result<T> {
        let guard = self.lock();
        match guard.as_ref() {
            Some(db) => f(db),
            None => Err(Error::InvalidDatabaseValue(
                "the database is not open".to_owned(),
            )),
        }
    }

    /// Removes the database file and creates a new, empty database
    /// (upstream `reset()`).
    fn reset(&self) -> Result<()> {
        let mut guard = self.lock();
        *guard = None; // Close the connection.
        for suffix in ["", "-wal", "-shm"] {
            let path = format!("{}{suffix}", self.file_path.as_str());
            if let Err(e) = std::fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                log::warn!("Failed to remove {path}: {e}");
            }
        }
        let db = SqliteDatabase::open(self.file_path.as_path())?;
        let writer = LibraryDbWriter::new(&self.libraries_path, &db);
        writer.create_all_tables()?;
        writer.add_internal_data("version", CURRENT_DB_VERSION)?;
        *guard = Some(db);
        Ok(())
    }

    /// Returns the database version, `None` if it cannot be determined.
    fn db_version(&self) -> Option<i64> {
        self.with_db(|db| {
            let mut query =
                db.prepare("SELECT value_int FROM internal WHERE key = 'version'", &[])?;
            Ok(query.query_row([], |row| row.get::<_, i64>(0))?)
        })
        .ok()
    }

    fn relative(&self, fp: &FilePath) -> String {
        fp.to_relative(&self.libraries_path)
    }

    fn absolute(&self, relative: &str) -> FilePath {
        FilePath::from_relative(&self.libraries_path, relative)
    }

    // ---------------------------------------------------------------------
    // Getters
    // ---------------------------------------------------------------------

    /// Returns the version and directory of all elements of a kind,
    /// optionally only those with `uuid` and/or in the library `lib`
    /// (upstream `getAll()`).
    ///
    /// Sorted by version; elements with the same version are in reverse
    /// database order (like upstream's `QMultiMap`), so the last entry is
    /// the latest version (found first on equal versions).
    pub fn all(
        &self,
        kind: ElementKind,
        uuid: Option<Uuid>,
        lib: Option<&FilePath>,
    ) -> Result<Vec<(Version, FilePath)>> {
        if lib.is_some() && kind == ElementKind::Library {
            return Err(Error::LibraryFilterOnLibraries);
        }
        let mut conditions = Vec::new();
        if uuid.is_some() {
            conditions.push("%elements.uuid = :uuid");
        }
        if lib.is_some() {
            conditions.push("libraries.filepath = :filepath");
        }
        let mut sql = "SELECT %elements.version, %elements.filepath FROM %elements ".to_owned();
        if lib.is_some() {
            sql += "LEFT JOIN libraries ON %elements.library_id = libraries.id ";
        }
        if !conditions.is_empty() {
            sql += &format!("WHERE {} ", conditions.join(" AND "));
        }
        let uuid_str = uuid.map(|u| u.to_string());
        let lib_str = lib.map(|l| self.relative(l));
        let mut params: Vec<(&str, &dyn ToSql)> = Vec::new();
        if let Some(uuid) = &uuid_str {
            params.push((":uuid", uuid));
        }
        if let Some(lib) = &lib_str {
            params.push((":filepath", lib));
        }
        let mut elements = self.with_db(|db| {
            let mut query = db.prepare(&sql, &[("%elements", kind.table())])?;
            let rows = query.query_map(params.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut elements = Vec::new();
            for row in rows {
                let (version, filepath) = row?;
                elements.push((parse_version(&version)?, self.absolute(&filepath)));
            }
            Ok(elements)
        })?;
        elements.reverse();
        elements.sort_by(|a, b| a.0.cmp(&b.0)); // Stable.
        Ok(elements)
    }

    /// Returns the directory and UUID of all elements of a kind in the
    /// library `lib` (upstream `getAll(lib)`).
    pub fn all_in_library(
        &self,
        kind: ElementKind,
        lib: &FilePath,
    ) -> Result<HashMap<FilePath, Uuid>> {
        let lib_str = self.relative(lib);
        self.with_db(|db| {
            let mut query = db.prepare(
                "SELECT %elements.uuid, %elements.filepath FROM %elements \
                 LEFT JOIN libraries ON %elements.library_id = libraries.id \
                 WHERE libraries.filepath = :filepath",
                &[("%elements", kind.table())],
            )?;
            let rows = query.query_map(named_params! {":filepath": lib_str}, |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut elements = HashMap::new();
            for row in rows {
                let (uuid, filepath) = row?;
                elements.insert(self.absolute(&filepath), parse_uuid(&uuid)?);
            }
            Ok(elements)
        })
    }

    /// Returns the directory of the element with the highest version and
    /// the given UUID (upstream `getLatest()`).
    pub fn latest(&self, kind: ElementKind, uuid: Uuid) -> Result<Option<FilePath>> {
        Ok(self.all(kind, Some(uuid), None)?.pop().map(|(_, fp)| fp))
    }

    /// Same as [`latest()`](Self::latest) (facade name for tools).
    pub fn element_dir(&self, kind: ElementKind, uuid: Uuid) -> Result<Option<FilePath>> {
        self.latest(kind, uuid)
    }

    /// Finds elements by keyword (upstream `find()`): the keyword is
    /// searched (`LIKE`, i.e. case insensitive for ASCII) in the names and
    /// keywords of all locales (and in the alternative names of packages);
    /// descriptions are not searched. An exact UUID matches too.
    ///
    /// Returns the UUIDs, sorted by name, without duplicates.
    pub fn find(&self, kind: ElementKind, keyword: &str) -> Result<Vec<Uuid>> {
        // ATTENTION: Keep both queries in sync!
        let sql = if kind == ElementKind::Package {
            "SELECT packages.uuid FROM packages \
             LEFT JOIN packages_tr \
             ON packages.id = packages_tr.element_id \
             LEFT JOIN packages_alt \
             ON packages.id = packages_alt.package_id \
             WHERE packages_tr.name LIKE :escapedKeyword \
             OR packages_tr.keywords LIKE :escapedKeyword \
             OR packages_alt.name LIKE :escapedKeyword \
             OR packages.uuid = :keyword \
             GROUP BY packages.uuid \
             ORDER BY packages_tr.name ASC"
        } else {
            "SELECT %elements.uuid FROM %elements \
             LEFT JOIN %elements_tr \
             ON %elements.id = %elements_tr.element_id \
             WHERE %elements_tr.name LIKE :escapedKeyword \
             OR %elements_tr.keywords LIKE :escapedKeyword \
             OR %elements.uuid = :keyword \
             GROUP BY %elements.uuid \
             ORDER BY %elements_tr.name ASC"
        };
        let escaped = format!("%{keyword}%");
        self.with_db(|db| {
            let mut query = db.prepare(sql, &[("%elements", kind.table())])?;
            let rows = query.query_map(
                named_params! {":keyword": keyword, ":escapedKeyword": escaped},
                |row| row.get::<_, String>(0),
            )?;
            collect_uuids(rows)
        })
    }

    /// Finds devices by the MPN or manufacturer of their parts (upstream
    /// `findDevicesOfParts()`). Sorted by name, without duplicates.
    pub fn find_devices_of_parts(&self, keyword: &str) -> Result<Vec<Uuid>> {
        let escaped = format!("%{keyword}%");
        self.with_db(|db| {
            let mut query = db.prepare(
                "SELECT devices.uuid FROM devices \
                 LEFT JOIN parts \
                 ON devices.id = parts.device_id \
                 LEFT JOIN devices_tr \
                 ON devices.id = devices_tr.element_id \
                 WHERE parts.manufacturer LIKE :keyword \
                 OR parts.mpn LIKE :keyword \
                 GROUP BY devices.uuid \
                 ORDER BY devices_tr.name ASC",
                &[],
            )?;
            let rows = query.query_map(named_params! {":keyword": escaped}, |row| {
                row.get::<_, String>(0)
            })?;
            collect_uuids(rows)
        })
    }

    /// Finds the parts of a device whose MPN or manufacturer matches the
    /// keyword (upstream `findPartsOfDevice()`). Sorted, without
    /// duplicates.
    pub fn find_parts_of_device(&self, device: Uuid, keyword: &str) -> Result<Vec<Part>> {
        let escaped = format!("%{keyword}%");
        self.parts(
            "SELECT parts.id, mpn, manufacturer FROM parts \
             LEFT JOIN devices \
             ON devices.id = parts.device_id \
             WHERE devices.uuid = :device \
             AND (parts.mpn LIKE :keyword OR parts.manufacturer LIKE :keyword)",
            &[(":device", &device.to_string()), (":keyword", &escaped)],
        )
    }

    /// Returns the parts of a device (upstream `getDeviceParts()`). Sorted,
    /// without duplicates.
    pub fn device_parts(&self, device: Uuid) -> Result<Vec<Part>> {
        self.parts(
            "SELECT parts.id, mpn, manufacturer FROM parts \
             LEFT JOIN devices ON devices.id = parts.device_id \
             WHERE devices.uuid = :device",
            &[(":device", &device.to_string())],
        )
    }

    fn parts(&self, sql: &str, params: &[(&str, &dyn ToSql)]) -> Result<Vec<Part>> {
        let mut parts: Vec<Part> = self.with_db(|db| {
            // Atomic attributes query!
            let _transaction = db.transaction()?;
            let mut query = db.prepare(sql, &[])?;
            let rows = query.query_map(params, |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    opt_string(row, 1)?,
                    opt_string(row, 2)?,
                ))
            })?;
            let mut parts = Vec::new();
            for row in rows {
                let (id, mpn, manufacturer) = row?;
                parts.push(Part::new(
                    SimpleString::new(mpn)?,
                    SimpleString::new(manufacturer)?,
                    part_attributes(db, id)?,
                ));
            }
            Ok(parts)
        })?;
        // Remove duplicates (upstream: QSet).
        let mut unique: Vec<Part> = Vec::with_capacity(parts.len());
        for part in parts.drain(..) {
            if !unique.contains(&part) {
                unique.push(part);
            }
        }
        unique.sort_by(compare_parts);
        Ok(unique)
    }

    /// Returns the translated texts of an element (upstream
    /// `getTranslations()`), `None` if the element does not exist or has no
    /// translations at all.
    ///
    /// For each text, the first locale of `locale_order` which has the text
    /// is used, falling back to the default locale (`""`) and then to an
    /// empty string.
    pub fn translations<S: AsRef<str>>(
        &self,
        kind: ElementKind,
        elem_dir: &FilePath,
        locale_order: &[S],
    ) -> Result<Option<Translations>> {
        let filepath = self.relative(elem_dir);
        let rows = self.with_db(|db| {
            let mut query = db.prepare(
                "SELECT locale, name, description, keywords FROM %elements_tr \
                 INNER JOIN %elements \
                 ON %elements.id = %elements_tr.element_id \
                 WHERE %elements.filepath = :filepath",
                &[("%elements", kind.table())],
            )?;
            let rows = query.query_map(named_params! {":filepath": filepath}, |row| {
                Ok((
                    opt_string(row, 0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })?;
        if rows.is_empty() {
            return Ok(None);
        }
        let mut names = HashMap::new();
        let mut descriptions = HashMap::new();
        let mut keywords = HashMap::new();
        for (locale, name, description, kw) in rows {
            if let Some(name) = name {
                names.insert(locale.clone(), name);
            }
            if let Some(description) = description {
                descriptions.insert(locale.clone(), description);
            }
            if let Some(kw) = kw {
                keywords.insert(locale, kw);
            }
        }
        Ok(Some(Translations {
            name: localized_value(&names, locale_order),
            description: localized_value(&descriptions, locale_order),
            keywords: localized_value(&keywords, locale_order),
        }))
    }

    /// Returns the metadata of an element (upstream `getMetadata()`), `None`
    /// if the element does not exist.
    pub fn metadata(&self, kind: ElementKind, elem_dir: &FilePath) -> Result<Option<ElementInfo>> {
        let filepath = self.relative(elem_dir);
        let row = self.with_db(|db| {
            let mut query = db.prepare(
                "SELECT uuid, version, deprecated FROM %elements \
                 WHERE filepath = :filepath \
                 LIMIT 1",
                &[("%elements", kind.table())],
            )?;
            let mut rows = query.query_map(named_params! {":filepath": filepath}, |row| {
                Ok((
                    opt_string(row, 0)?,
                    opt_string(row, 1)?,
                    row.get::<_, Option<bool>>(2)?.unwrap_or(false),
                ))
            })?;
            Ok(rows.next().transpose()?)
        })?;
        let Some((uuid, version, deprecated)) = row else {
            log::warn!("Element not found in database: {elem_dir}");
            return Ok(None);
        };
        Ok(Some(ElementInfo {
            uuid: parse_uuid(&uuid)?,
            version: parse_version(&version)?,
            deprecated,
        }))
    }

    /// Returns additional metadata of a library (upstream
    /// `getLibraryMetadata()`), `None` if the library does not exist.
    pub fn library_metadata(&self, lib_dir: &FilePath) -> Result<Option<LibraryInfo>> {
        let filepath = self.relative(lib_dir);
        let info = self.with_db(|db| {
            let mut query = db.prepare(
                "SELECT icon_png, manufacturer FROM libraries \
                 WHERE filepath = :filepath \
                 LIMIT 1",
                &[],
            )?;
            let mut rows = query.query_map(named_params! {":filepath": filepath}, |row| {
                Ok(LibraryInfo {
                    icon_png: row.get::<_, Option<Vec<u8>>>(0)?.unwrap_or_default(),
                    manufacturer: opt_string(row, 1)?,
                })
            })?;
            Ok(rows.next().transpose()?)
        })?;
        if info.is_none() {
            log::warn!("Library not found in database: {lib_dir}");
        }
        Ok(info)
    }

    /// Returns additional metadata of a category (upstream
    /// `getCategoryMetadata()`), `None` if the category does not exist.
    pub fn category_metadata(
        &self,
        kind: ElementKind,
        cat_dir: &FilePath,
    ) -> Result<Option<CategoryInfo>> {
        if !kind.is_category() {
            return Err(Error::UnsupportedElementKind(kind));
        }
        let filepath = self.relative(cat_dir);
        let row = self.with_db(|db| {
            let mut query = db.prepare(
                "SELECT parent_uuid FROM %categories \
                 WHERE filepath = :filepath \
                 LIMIT 1",
                &[("%categories", kind.table())],
            )?;
            let mut rows = query.query_map(named_params! {":filepath": filepath}, |row| {
                opt_string(row, 0)
            })?;
            Ok(rows.next().transpose()?)
        })?;
        match row {
            Some(parent) => Ok(Some(CategoryInfo {
                parent: parent.parse().ok(),
            })),
            None => {
                log::warn!("Category not found in database: {cat_dir}");
                Ok(None)
            }
        }
    }

    /// Returns additional metadata of a device (upstream
    /// `getDeviceMetadata()`), `None` if the device does not exist.
    pub fn device_metadata(&self, dev_dir: &FilePath) -> Result<Option<DeviceInfo>> {
        let filepath = self.relative(dev_dir);
        let row = self.with_db(|db| {
            let mut query = db.prepare(
                "SELECT component_uuid, package_uuid FROM devices \
                 WHERE filepath = :filepath \
                 LIMIT 1",
                &[],
            )?;
            let mut rows = query.query_map(named_params! {":filepath": filepath}, |row| {
                Ok((opt_string(row, 0)?, opt_string(row, 1)?))
            })?;
            Ok(rows.next().transpose()?)
        })?;
        let Some((cmp, pkg)) = row else {
            log::warn!("Device not found in database: {dev_dir}");
            return Ok(None);
        };
        Ok(Some(DeviceInfo {
            component_uuid: parse_uuid(&cmp)?,
            package_uuid: parse_uuid(&pkg)?,
        }))
    }

    /// Returns the child categories of a category (upstream
    /// `getChildren()`). With `parent == None`, all root categories and
    /// categories with an inexistent parent are returned (so all elements
    /// are discoverable by [`by_category()`](Self::by_category)).
    pub fn children(&self, kind: ElementKind, parent: Option<Uuid>) -> Result<BTreeSet<Uuid>> {
        if !kind.is_category() {
            return Err(Error::UnsupportedElementKind(kind));
        }
        self.with_db(|db| match parent {
            Some(parent) => {
                let mut query = db.prepare(
                    "SELECT uuid FROM %categories \
                     WHERE parent_uuid = :category_uuid \
                     GROUP BY uuid",
                    &[("%categories", kind.table())],
                )?;
                let rows = query.query_map(
                    named_params! {":category_uuid": parent.to_string()},
                    |row| row.get::<_, String>(0),
                )?;
                collect_uuid_set(rows)
            }
            None => {
                let mut query = db.prepare(
                    "SELECT children.uuid FROM %categories AS children \
                     LEFT JOIN %categories AS parents \
                     ON children.parent_uuid = parents.uuid \
                     WHERE parents.uuid IS NULL \
                     GROUP BY children.uuid",
                    &[("%categories", kind.table())],
                )?;
                let rows = query.query_map([], |row| row.get::<_, String>(0))?;
                collect_uuid_set(rows)
            }
        })
    }

    /// Returns the elements of a category (upstream `getByCategory()`).
    /// With `category == None`, all elements without (existing) category
    /// are returned. `limit` limits the number of results (e.g. `Some(1)`
    /// to check whether a category contains any elements).
    pub fn by_category(
        &self,
        kind: ElementKind,
        category: Option<Uuid>,
        limit: Option<usize>,
    ) -> Result<BTreeSet<Uuid>> {
        let category_kind = kind
            .category_kind()
            .ok_or(Error::UnsupportedElementKind(kind))?;
        let limit = limit.map_or(-1, |l| i64::try_from(l).unwrap_or(i64::MAX));
        let replacements = [
            ("%elements", kind.table()),
            ("%categories", category_kind.table()),
        ];
        self.with_db(|db| match category {
            Some(category) => {
                // Find all elements assigned to the specified category.
                let mut query = db.prepare(
                    "SELECT %elements.uuid FROM %elements \
                     INNER JOIN %elements_cat \
                     ON %elements.id = %elements_cat.element_id \
                     WHERE category_uuid = :uuid \
                     GROUP BY uuid \
                     LIMIT :limit",
                    &replacements,
                )?;
                let rows = query.query_map(
                    named_params! {":uuid": category.to_string(), ":limit": limit},
                    |row| row.get::<_, String>(0),
                )?;
                collect_uuid_set(rows)
            }
            None => {
                // Find all elements with no (existent) category.
                let mut query = db.prepare(
                    "SELECT %elements.uuid FROM %elements \
                     LEFT JOIN %elements_cat \
                     ON %elements.id = %elements_cat.element_id \
                     LEFT JOIN %categories \
                     ON %elements_cat.category_uuid = %categories.uuid \
                     GROUP BY %elements.uuid \
                     HAVING COUNT(%categories.uuid) = 0 \
                     LIMIT :limit",
                    &replacements,
                )?;
                let rows = query.query_map(named_params! {":limit": limit}, |row| {
                    row.get::<_, String>(0)
                })?;
                collect_uuid_set(rows)
            }
        })
    }

    /// Returns the categories of an element (upstream `getCategoriesOf()`).
    pub fn categories_of(&self, kind: ElementKind, elem_dir: &FilePath) -> Result<BTreeSet<Uuid>> {
        let category_kind = kind
            .category_kind()
            .ok_or(Error::UnsupportedElementKind(kind))?;
        let filepath = self.relative(elem_dir);
        self.with_db(|db| {
            let mut query = db.prepare(
                "SELECT category_uuid FROM %elements_cat \
                 INNER JOIN %elements \
                 ON %elements_cat.element_id = %elements.id \
                 WHERE filepath = :filepath",
                &[
                    ("%elements", kind.table()),
                    ("%categories", category_kind.table()),
                ],
            )?;
            let rows = query.query_map(named_params! {":filepath": filepath}, |row| {
                row.get::<_, String>(0)
            })?;
            collect_uuid_set(rows)
        })
    }

    /// Returns the elements generated by `generated_by` (upstream
    /// `getGenerated()`); empty if `generated_by` is empty.
    pub fn generated(&self, kind: ElementKind, generated_by: &str) -> Result<BTreeSet<Uuid>> {
        if !kind.has_generated_by() {
            return Err(Error::UnsupportedElementKind(kind));
        }
        if generated_by.is_empty() {
            return Ok(BTreeSet::new());
        }
        self.with_db(|db| {
            let mut query = db.prepare(
                "SELECT uuid FROM %elements \
                 WHERE generated_by = :generated_by \
                 GROUP BY uuid",
                &[("%elements", kind.table())],
            )?;
            let rows = query.query_map(named_params! {":generated_by": generated_by}, |row| {
                row.get::<_, String>(0)
            })?;
            collect_uuid_set(rows)
        })
    }

    /// Returns the resources of a component or device (upstream
    /// `getResources()`); empty if the element does not exist.
    pub fn resources(&self, kind: ElementKind, elem_dir: &FilePath) -> Result<ResourceList> {
        if !kind.has_resources() {
            return Err(Error::UnsupportedElementKind(kind));
        }
        let filepath = self.relative(elem_dir);
        self.with_db(|db| {
            let mut query = db.prepare(
                "SELECT name, media_type, url FROM %elements_res \
                 LEFT JOIN %elements ON %elements.id = %elements_res.element_id \
                 WHERE %elements.filepath = :filepath",
                &[("%elements", kind.table())],
            )?;
            let rows = query.query_map(named_params! {":filepath": filepath}, |row| {
                Ok((
                    opt_string(row, 0)?,
                    opt_string(row, 1)?,
                    opt_string(row, 2)?,
                ))
            })?;
            let mut resources = ResourceList::new();
            for row in rows {
                let (name, media_type, url) = row?;
                resources.push(Resource::new(ElementName::new(name)?, media_type, url));
            }
            Ok(resources)
        })
    }

    /// Returns the devices of a component (upstream
    /// `getComponentDevices()`).
    pub fn component_devices(&self, component: Uuid) -> Result<BTreeSet<Uuid>> {
        self.with_db(|db| {
            let mut query = db.prepare(
                "SELECT uuid FROM devices \
                 WHERE component_uuid = :uuid \
                 GROUP BY uuid",
                &[],
            )?;
            let rows = query.query_map(named_params! {":uuid": component.to_string()}, |row| {
                row.get::<_, String>(0)
            })?;
            collect_uuid_set(rows)
        })
    }

    /// Returns the devices using a package (no upstream counterpart).
    pub fn package_devices(&self, package: Uuid) -> Result<BTreeSet<Uuid>> {
        self.with_db(|db| {
            let mut query = db.prepare(
                "SELECT uuid FROM devices \
                 WHERE package_uuid = :uuid \
                 GROUP BY uuid",
                &[],
            )?;
            let rows = query.query_map(named_params! {":uuid": package.to_string()}, |row| {
                row.get::<_, String>(0)
            })?;
            collect_uuid_set(rows)
        })
    }

    /// Returns all organizations, only the latest version of each (upstream
    /// `getAllLatestOrganizations()`), sorted by priority (highest first)
    /// and name.
    pub fn all_latest_organizations<S: AsRef<str>>(
        &self,
        locale_order: &[S],
        pcb_design_rules: bool,
        output_jobs: bool,
    ) -> Result<Vec<OrganizationInfo>> {
        // Get all organizations.
        let rows = self.with_db(|db| {
            let mut query = db.prepare(
                "SELECT id, filepath, uuid, version, logo_png, url, country, fabs, \
                 shipping, sponsor, priority FROM organizations",
                &[],
            )?;
            let rows = query.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    opt_string(row, 1)?,
                    opt_string(row, 2)?,
                    opt_string(row, 3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?.unwrap_or_default(),
                    opt_string(row, 5)?,
                    opt_string(row, 6)?,
                    opt_string(row, 7)?,
                    opt_string(row, 8)?,
                    row.get::<_, Option<bool>>(9)?.unwrap_or(false),
                    row.get::<_, Option<i64>>(10)?.unwrap_or(0),
                ))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })?;
        let mut entries: HashMap<Uuid, Vec<(i64, OrganizationInfo)>> = HashMap::new();
        for (id, filepath, uuid, version, logo, url, country, fabs, shipping, sponsor, priority) in
            rows
        {
            let org = OrganizationInfo {
                directory: self.absolute(&filepath),
                uuid: parse_uuid(&uuid)?,
                name: String::new(),
                description: String::new(),
                version: parse_version(&version)?,
                logo_png: logo,
                url,
                country,
                fabs: fabs.split(',').map(str::to_owned).collect(),
                shipping: shipping.split(',').map(str::to_owned).collect(),
                is_sponsor: sponsor,
                priority,
                pcb_design_rules: Vec::new(),
                output_jobs: Vec::new(),
            };
            entries.entry(org.uuid).or_default().push((id, org));
        }

        // Take only the newest version of each organization (local library
        // elements first on equal versions).
        let mut result = Vec::new();
        for mut entry in entries.into_values() {
            entry.sort_by(|(_, a), (_, b)| {
                b.version
                    .cmp(&a.version)
                    .then_with(|| a.directory.cmp(&b.directory))
            });
            if let Some(first) = entry.into_iter().next() {
                result.push(first);
            }
        }

        // Fill in remaining data.
        for (id, org) in &mut result {
            if let Some(tr) =
                self.translations(ElementKind::Organization, &org.directory, locale_order)?
            {
                org.name = tr.name;
                org.description = tr.description;
            }
            if pcb_design_rules {
                org.pcb_design_rules = self.with_db(|db| {
                    let mut query = db.prepare(
                        "SELECT uuid, name, description, url, max_layers \
                         FROM organization_pcb_design_rules \
                         WHERE organization_id = :organization_id",
                        &[],
                    )?;
                    let rows = query.query_map(named_params! {":organization_id": *id}, |row| {
                        Ok((
                            opt_string(row, 0)?,
                            opt_string(row, 1)?,
                            opt_string(row, 2)?,
                            opt_string(row, 3)?,
                            row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                        ))
                    })?;
                    let mut rules = Vec::new();
                    for row in rows {
                        let (uuid, name, description, url, max_layer_count) = row?;
                        rules.push(PcbDesignRulesInfo {
                            uuid: parse_uuid(&uuid)?,
                            name,
                            description,
                            url,
                            max_layer_count,
                        });
                    }
                    Ok(rules)
                })?;
            }
            if output_jobs {
                org.output_jobs = self.with_db(|db| {
                    let mut query = db.prepare(
                        "SELECT kind, uuid, type, name FROM organization_output_jobs \
                         WHERE organization_id = :organization_id",
                        &[],
                    )?;
                    let rows = query.query_map(named_params! {":organization_id": *id}, |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            opt_string(row, 1)?,
                            opt_string(row, 2)?,
                            opt_string(row, 3)?,
                        ))
                    })?;
                    let mut jobs = Vec::new();
                    for row in rows {
                        let (kind, uuid, job_type, name) = row?;
                        let kind = OutputJobKind::from_db(kind).ok_or_else(|| {
                            Error::InvalidDatabaseValue(format!("output job kind {kind}"))
                        })?;
                        jobs.push(OutputJobInfo {
                            kind,
                            uuid: parse_uuid(&uuid)?,
                            job_type,
                            name,
                        });
                    }
                    Ok(jobs)
                })?;
            }
        }

        // Pre-sort results in a way suitable for most use-cases.
        let mut result: Vec<OrganizationInfo> = result.into_iter().map(|(_, org)| org).collect();
        result.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(result)
    }

    // ---------------------------------------------------------------------
    // Facade for tools and agents
    // ---------------------------------------------------------------------

    /// Returns the summary of the latest version of an element, `None` if
    /// there is no element of this kind with this UUID.
    pub fn element_summary<S: AsRef<str>>(
        &self,
        kind: ElementKind,
        uuid: Uuid,
        locale_order: &[S],
    ) -> Result<Option<ElementSummary>> {
        let Some((version, directory)) = self.all(kind, Some(uuid), None)?.pop() else {
            return Ok(None);
        };
        let deprecated = self
            .metadata(kind, &directory)?
            .is_some_and(|info| info.deprecated);
        let translations = self
            .translations(kind, &directory, locale_order)?
            .unwrap_or_default();
        let categories = if kind.category_kind().is_some() {
            self.categories_of(kind, &directory)?
        } else {
            BTreeSet::new()
        };
        let device = if kind == ElementKind::Device {
            self.device_metadata(&directory)?
        } else {
            None
        };
        Ok(Some(ElementSummary {
            kind,
            uuid,
            directory,
            version,
            deprecated,
            name: translations.name,
            description: translations.description,
            keywords: translations.keywords,
            categories,
            device,
        }))
    }

    /// Searches elements (see [`SearchQuery`]) and returns the summaries of
    /// their latest versions, grouped by kind (in the order of
    /// `query.kinds`) and sorted by name within each kind.
    pub fn search(&self, query: &SearchQuery) -> Result<Vec<ElementSummary>> {
        let kinds: &[ElementKind] = if query.kinds.is_empty() {
            &ElementKind::ELEMENTS
        } else {
            &query.kinds
        };
        let limit = query.limit.unwrap_or(usize::MAX);
        let mut result = Vec::new();
        for &kind in kinds {
            let mut uuids = self.find(kind, &query.keyword)?;
            if kind == ElementKind::Device && query.include_parts {
                let mut seen: HashSet<Uuid> = uuids.iter().copied().collect();
                for uuid in self.find_devices_of_parts(&query.keyword)? {
                    if seen.insert(uuid) {
                        uuids.push(uuid);
                    }
                }
            }
            let mut summaries = Vec::new();
            for uuid in uuids {
                if let Some(summary) = self.element_summary(kind, uuid, &query.locale_order)? {
                    summaries.push(summary);
                }
            }
            summaries.sort_by(|a, b| {
                crate::library::natural_cmp_case_insensitive(&a.name, &b.name)
                    .then_with(|| a.uuid.cmp(&b.uuid))
            });
            for summary in summaries {
                if result.len() >= limit {
                    return Ok(result);
                }
                result.push(summary);
            }
        }
        Ok(result)
    }

    /// Finds elements where every whitespace separated token of `keyword`
    /// occurs (case insensitive) in the element's texts: names, keywords
    /// and descriptions of all locales, alternative names of packages,
    /// and for devices also the names of their component and package and
    /// the MPNs/manufacturers of their parts. No upstream counterpart (the
    /// upstream [`find()`](Self::find) matches the whole keyword); meant
    /// for multi-word queries of tools and agents ("resistor 0805").
    ///
    /// Returns the UUIDs ranked by the number of tokens found in the
    /// element's own names (most first), then by name.
    pub fn find_all_tokens(&self, kind: ElementKind, keyword: &str) -> Result<Vec<Uuid>> {
        let tokens: Vec<String> = keyword.split_whitespace().map(str::to_lowercase).collect();
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        #[derive(Default)]
        struct Texts {
            name: String,
            names: String,
            other: String,
        }
        let mut elements: HashMap<Uuid, Texts> = HashMap::new();
        let table = kind.table();
        self.with_db(|db| {
            // (query, whether column 1 is an own name of the element)
            let mut queries = vec![(
                "SELECT %elements.uuid, %elements_tr.name, %elements_tr.keywords, \
                 %elements_tr.description FROM %elements \
                 LEFT JOIN %elements_tr ON %elements.id = %elements_tr.element_id",
                true,
            )];
            if kind == ElementKind::Package {
                queries.push((
                    "SELECT packages.uuid, packages_alt.name, NULL, NULL FROM packages \
                     INNER JOIN packages_alt ON packages.id = packages_alt.package_id",
                    true,
                ));
            }
            if kind == ElementKind::Device {
                queries.push((
                    "SELECT devices.uuid, components_tr.name, NULL, NULL FROM devices \
                     INNER JOIN components ON components.uuid = devices.component_uuid \
                     INNER JOIN components_tr ON components.id = components_tr.element_id",
                    false,
                ));
                queries.push((
                    "SELECT devices.uuid, packages_tr.name, NULL, NULL FROM devices \
                     INNER JOIN packages ON packages.uuid = devices.package_uuid \
                     INNER JOIN packages_tr ON packages.id = packages_tr.element_id",
                    false,
                ));
                queries.push((
                    "SELECT devices.uuid, packages_alt.name, NULL, NULL FROM devices \
                     INNER JOIN packages ON packages.uuid = devices.package_uuid \
                     INNER JOIN packages_alt ON packages.id = packages_alt.package_id",
                    false,
                ));
                queries.push((
                    "SELECT devices.uuid, parts.mpn, parts.manufacturer, NULL FROM devices \
                     INNER JOIN parts ON devices.id = parts.device_id",
                    false,
                ));
            }
            for (sql, own_name) in queries {
                let mut query = db.prepare(sql, &[("%elements", table)])?;
                let rows = query.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        opt_string(row, 1)?,
                        opt_string(row, 2)?,
                        opt_string(row, 3)?,
                    ))
                })?;
                for row in rows {
                    let (uuid, name, texts1, texts2) = row?;
                    let texts = elements.entry(parse_uuid(&uuid)?).or_default();
                    if own_name {
                        if texts.name.is_empty() {
                            texts.name = name.clone();
                        }
                        texts.names.push('\n');
                        texts.names.push_str(&name.to_lowercase());
                    } else {
                        texts.other.push('\n');
                        texts.other.push_str(&name.to_lowercase());
                    }
                    for t in [texts1, texts2] {
                        texts.other.push('\n');
                        texts.other.push_str(&t.to_lowercase());
                    }
                }
            }
            Ok(())
        })?;
        let mut found: Vec<(usize, String, Uuid)> = elements
            .into_iter()
            .filter(|(_, t)| {
                tokens
                    .iter()
                    .all(|tok| t.names.contains(tok.as_str()) || t.other.contains(tok.as_str()))
            })
            .map(|(uuid, t)| {
                let in_names = tokens
                    .iter()
                    .filter(|tok| t.names.contains(tok.as_str()))
                    .count();
                (in_names, t.name, uuid)
            })
            .collect();
        found.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| crate::library::natural_cmp_case_insensitive(&a.1, &b.1))
                .then_with(|| a.2.cmp(&b.2))
        });
        Ok(found.into_iter().map(|(_, _, uuid)| uuid).collect())
    }

    /// Like [`search()`](Self::search), but matching every whitespace
    /// separated token of the keyword (see
    /// [`find_all_tokens()`](Self::find_all_tokens)); results are grouped
    /// by kind in the order of `query.kinds`, ranked within each kind.
    pub fn search_all_tokens(&self, query: &SearchQuery) -> Result<Vec<ElementSummary>> {
        let kinds: &[ElementKind] = if query.kinds.is_empty() {
            &ElementKind::ELEMENTS
        } else {
            &query.kinds
        };
        let limit = query.limit.unwrap_or(usize::MAX);
        let mut result = Vec::new();
        for &kind in kinds {
            for uuid in self.find_all_tokens(kind, &query.keyword)? {
                if result.len() >= limit {
                    return Ok(result);
                }
                if let Some(summary) = self.element_summary(kind, uuid, &query.locale_order)? {
                    result.push(summary);
                }
            }
        }
        Ok(result)
    }

    /// Returns the category tree of a category kind (component or package
    /// categories): the root categories (including those with an
    /// inexistent parent) with their children, sorted by name. Cycles are
    /// broken (a category appears at most once).
    pub fn category_tree<S: AsRef<str>>(
        &self,
        kind: ElementKind,
        locale_order: &[S],
    ) -> Result<Vec<CategoryTreeNode>> {
        let mut visited = HashSet::new();
        self.category_subtree(kind, None, locale_order, &mut visited)
    }

    fn category_subtree<S: AsRef<str>>(
        &self,
        kind: ElementKind,
        parent: Option<Uuid>,
        locale_order: &[S],
        visited: &mut HashSet<Uuid>,
    ) -> Result<Vec<CategoryTreeNode>> {
        let mut nodes = Vec::new();
        for uuid in self.children(kind, parent)? {
            if !visited.insert(uuid) {
                continue;
            }
            let name = match self.latest(kind, uuid)? {
                Some(dir) => self
                    .translations(kind, &dir, locale_order)?
                    .map(|tr| tr.name)
                    .unwrap_or_default(),
                None => String::new(),
            };
            let children = self.category_subtree(kind, Some(uuid), locale_order, visited)?;
            nodes.push(CategoryTreeNode {
                uuid,
                name,
                children,
            });
        }
        nodes.sort_by(|a, b| {
            crate::library::natural_cmp_case_insensitive(&a.name, &b.name)
                .then_with(|| a.uuid.cmp(&b.uuid))
        });
        Ok(nodes)
    }
}

/// Returns the value for the first locale of `locale_order` which exists,
/// falling back to the default locale `""` and then to an empty string
/// (upstream `LocalizedDescriptionMap::value()` on a map with an empty
/// default value).
fn localized_value<S: AsRef<str>>(map: &HashMap<String, String>, locale_order: &[S]) -> String {
    locale_order
        .iter()
        .find_map(|locale| map.get(locale.as_ref()))
        .or_else(|| map.get(""))
        .cloned()
        .unwrap_or_default()
}

/// Reads a text column, `NULL` as empty string (upstream
/// `QVariant::toString()`).
fn opt_string(row: &Row<'_>, index: usize) -> rusqlite::Result<String> {
    Ok(row.get::<_, Option<String>>(index)?.unwrap_or_default())
}

fn parse_uuid(s: &str) -> Result<Uuid> {
    s.parse()
        .map_err(|_| Error::InvalidDatabaseValue(format!("invalid UUID \"{s}\"")))
}

fn parse_version(s: &str) -> Result<Version> {
    s.parse()
        .map_err(|_| Error::InvalidDatabaseValue(format!("invalid version \"{s}\"")))
}

fn collect_uuids(rows: impl Iterator<Item = rusqlite::Result<String>>) -> Result<Vec<Uuid>> {
    rows.map(|row| parse_uuid(&row?)).collect()
}

fn collect_uuid_set(
    rows: impl Iterator<Item = rusqlite::Result<String>>,
) -> Result<BTreeSet<Uuid>> {
    rows.map(|row| parse_uuid(&row?)).collect()
}

/// Reads the attributes of a part (upstream `getPartAttributes()`).
fn part_attributes(db: &SqliteDatabase, part_id: i64) -> Result<AttributeList> {
    let mut query = db.prepare(
        "SELECT key, type, value, unit FROM parts_attr \
         WHERE part_id = :part_id",
        &[],
    )?;
    let rows = query.query_map(named_params! {":part_id": part_id}, |row| {
        Ok((
            opt_string(row, 0)?,
            opt_string(row, 1)?,
            opt_string(row, 2)?,
            opt_string(row, 3)?,
        ))
    })?;
    let mut attributes = AttributeList::new();
    for row in rows {
        let (key, attribute_type, value, unit) = row?;
        let attribute_type: AttributeType = attribute_type.parse()?;
        let unit = attribute_type.unit_from_string(&unit)?;
        attributes.push(Attribute::new(
            AttributeKey::new(key)?,
            attribute_type,
            value,
            unit,
        )?);
    }
    Ok(attributes)
}

/// Sort order of parts (upstream `WorkspaceLibraryDb::Part::operator<`):
/// parts without MPN first, then by MPN, manufacturer and attributes
/// (key, type, value compared naturally).
fn compare_parts(a: &Part, b: &Part) -> Ordering {
    let (a_mpn, b_mpn) = (a.mpn().as_str(), b.mpn().as_str());
    if a_mpn.is_empty() != b_mpn.is_empty() {
        // Parts without MPN first.
        return if a_mpn.is_empty() {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    a_mpn
        .cmp(b_mpn)
        .then_with(|| a.manufacturer().as_str().cmp(b.manufacturer().as_str()))
        .then_with(|| {
            let (a_attrs, b_attrs) = (a.attributes(), b.attributes());
            for i in 0..a_attrs.len().max(b_attrs.len()) {
                let ordering = match (a_attrs.get(i), b_attrs.get(i)) {
                    (Some(_), None) => Ordering::Greater,
                    (None, Some(_)) => Ordering::Less,
                    (Some(x), Some(y)) => x
                        .key()
                        .as_str()
                        .cmp(y.key().as_str())
                        .then_with(|| x.attribute_type().name().cmp(y.attribute_type().name()))
                        .then_with(|| {
                            crate::library::natural_cmp_case_insensitive(
                                &x.value_tr(true),
                                &y.value_tr(true),
                            )
                        }),
                    (None, None) => Ordering::Equal,
                };
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            Ordering::Equal
        })
}
