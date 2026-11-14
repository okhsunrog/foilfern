//! Port of libs/librepcb/core/workspace/workspacelibrarydbwriter.{h,cpp}.
//!
//! Writes the workspace library database (used by the library scanner and
//! by tests). The schema is identical to upstream (the database file is
//! shared with upstream LibrePCB).
//!
//! Differences to upstream:
//! - The element type template parameters are an [`ElementKind`] argument;
//!   unsupported kinds return [`Error::UnsupportedElementKind`] (upstream
//!   `static_assert`).
//! - Values are bound like the Qt SQLite driver does: empty strings where
//!   upstream calls `nonNull()`, `NULL` where it calls `nonEmptyOrNull()` or
//!   passes a null `QVariant`, booleans as integers, byte arrays as blobs
//!   (`NULL` if empty). URLs are stored verbatim (upstream: `QUrl::toString()`).

use rusqlite::named_params;

use super::ElementKind;
use super::error::{Error, Result};
use crate::attribute::Attribute;
use crate::fileio::FilePath;
use crate::sqlite_database::SqliteDatabase;
use crate::types::{Uuid, Version};

/// Kind of output jobs provided by an organization (upstream
/// `WorkspaceLibraryDb::OutputJobKind`; the values are stored in the
/// database).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum OutputJobKind {
    /// PCB fabrication output jobs.
    Pcb,
    /// Assembly output jobs.
    Assembly,
    /// Other output jobs.
    User,
}

impl OutputJobKind {
    /// Returns the value stored in the database.
    pub fn to_db(self) -> i64 {
        match self {
            Self::Pcb => 1,
            Self::Assembly => 2,
            Self::User => 99,
        }
    }

    /// Converts a value stored in the database.
    pub fn from_db(value: i64) -> Option<Self> {
        match value {
            1 => Some(Self::Pcb),
            2 => Some(Self::Assembly),
            99 => Some(Self::User),
            _ => None,
        }
    }
}

/// Table definitions (upstream `createAllTables()`).
const CREATE_TABLES: &[&str] = &[
    // internal
    "CREATE TABLE IF NOT EXISTS internal (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `key` TEXT UNIQUE NOT NULL, \
     `value_text` TEXT, \
     `value_int` INTEGER, \
     `value_real` REAL, \
     `value_blob` BLOB \
     )",
    // libraries
    "CREATE TABLE IF NOT EXISTS libraries (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `filepath` TEXT UNIQUE NOT NULL, \
     `uuid` TEXT NOT NULL, \
     `version` TEXT NOT NULL, \
     `deprecated` BOOLEAN NOT NULL, \
     `icon_png` BLOB, \
     `manufacturer` TEXT NOT NULL\
     )",
    "CREATE TABLE IF NOT EXISTS libraries_tr (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `element_id` INTEGER \
     REFERENCES libraries(id) ON DELETE CASCADE NOT NULL, \
     `locale` TEXT NOT NULL, \
     `name` TEXT, \
     `description` TEXT, \
     `keywords` TEXT, \
     UNIQUE(element_id, locale)\
     )",
    // component categories
    "CREATE TABLE IF NOT EXISTS component_categories (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `library_id` INTEGER NOT NULL, \
     `filepath` TEXT UNIQUE NOT NULL, \
     `uuid` TEXT NOT NULL, \
     `version` TEXT NOT NULL, \
     `deprecated` BOOLEAN NOT NULL, \
     `parent_uuid` TEXT\
     )",
    "CREATE TABLE IF NOT EXISTS component_categories_tr (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `element_id` INTEGER \
     REFERENCES component_categories(id) ON DELETE CASCADE NOT NULL, \
     `locale` TEXT NOT NULL, \
     `name` TEXT, \
     `description` TEXT, \
     `keywords` TEXT, \
     UNIQUE(element_id, locale)\
     )",
    // package categories
    "CREATE TABLE IF NOT EXISTS package_categories (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `library_id` INTEGER NOT NULL, \
     `filepath` TEXT UNIQUE NOT NULL, \
     `uuid` TEXT NOT NULL, \
     `version` TEXT NOT NULL, \
     `deprecated` BOOLEAN NOT NULL, \
     `parent_uuid` TEXT\
     )",
    "CREATE TABLE IF NOT EXISTS package_categories_tr (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `element_id` INTEGER \
     REFERENCES package_categories(id) ON DELETE CASCADE NOT NULL, \
     `locale` TEXT NOT NULL, \
     `name` TEXT, \
     `description` TEXT, \
     `keywords` TEXT, \
     UNIQUE(element_id, locale)\
     )",
    // symbols
    "CREATE TABLE IF NOT EXISTS symbols (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `library_id` INTEGER NOT NULL, \
     `filepath` TEXT UNIQUE NOT NULL, \
     `uuid` TEXT NOT NULL, \
     `version` TEXT NOT NULL, \
     `deprecated` BOOLEAN NOT NULL, \
     `generated_by` TEXT\
     )",
    "CREATE TABLE IF NOT EXISTS symbols_tr (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `element_id` INTEGER \
     REFERENCES symbols(id) ON DELETE CASCADE NOT NULL, \
     `locale` TEXT NOT NULL, \
     `name` TEXT, \
     `description` TEXT, \
     `keywords` TEXT, \
     UNIQUE(element_id, locale)\
     )",
    "CREATE TABLE IF NOT EXISTS symbols_cat (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `element_id` INTEGER \
     REFERENCES symbols(id) ON DELETE CASCADE NOT NULL, \
     `category_uuid` TEXT NOT NULL, \
     UNIQUE(element_id, category_uuid)\
     )",
    // packages
    "CREATE TABLE IF NOT EXISTS packages (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `library_id` INTEGER NOT NULL, \
     `filepath` TEXT UNIQUE NOT NULL, \
     `uuid` TEXT NOT NULL, \
     `version` TEXT NOT NULL, \
     `deprecated` BOOLEAN NOT NULL, \
     `generated_by` TEXT\
     )",
    "CREATE TABLE IF NOT EXISTS packages_tr (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `element_id` INTEGER \
     REFERENCES packages(id) ON DELETE CASCADE NOT NULL, \
     `locale` TEXT NOT NULL, \
     `name` TEXT, \
     `description` TEXT, \
     `keywords` TEXT, \
     UNIQUE(element_id, locale)\
     )",
    "CREATE TABLE IF NOT EXISTS packages_cat (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `element_id` INTEGER \
     REFERENCES packages(id) ON DELETE CASCADE NOT NULL, \
     `category_uuid` TEXT NOT NULL, \
     UNIQUE(element_id, category_uuid)\
     )",
    "CREATE TABLE IF NOT EXISTS packages_alt (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `package_id` INTEGER \
     REFERENCES packages(id) ON DELETE CASCADE NOT NULL, \
     `name` TEXT NOT NULL, \
     `reference` TEXT NOT NULL\
     )",
    // components
    "CREATE TABLE IF NOT EXISTS components (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `library_id` INTEGER NOT NULL, \
     `filepath` TEXT UNIQUE NOT NULL, \
     `uuid` TEXT NOT NULL, \
     `version` TEXT NOT NULL, \
     `deprecated` BOOLEAN NOT NULL, \
     `generated_by` TEXT\
     )",
    "CREATE TABLE IF NOT EXISTS components_tr (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `element_id` INTEGER \
     REFERENCES components(id) ON DELETE CASCADE NOT NULL, \
     `locale` TEXT NOT NULL, \
     `name` TEXT, \
     `description` TEXT, \
     `keywords` TEXT, \
     UNIQUE(element_id, locale)\
     )",
    "CREATE TABLE IF NOT EXISTS components_cat (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `element_id` INTEGER \
     REFERENCES components(id) ON DELETE CASCADE NOT NULL, \
     `category_uuid` TEXT NOT NULL, \
     UNIQUE(element_id, category_uuid)\
     )",
    "CREATE TABLE IF NOT EXISTS components_res (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `element_id` INTEGER \
     REFERENCES components(id) ON DELETE CASCADE NOT NULL, \
     `name` TEXT NOT NULL, \
     `media_type` TEXT NOT NULL, \
     `url` TEXT\
     )",
    // devices
    "CREATE TABLE IF NOT EXISTS devices (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `library_id` INTEGER NOT NULL, \
     `filepath` TEXT UNIQUE NOT NULL, \
     `uuid` TEXT NOT NULL, \
     `version` TEXT NOT NULL, \
     `deprecated` BOOLEAN NOT NULL, \
     `component_uuid` TEXT NOT NULL, \
     `package_uuid` TEXT NOT NULL, \
     `generated_by` TEXT\
     )",
    "CREATE TABLE IF NOT EXISTS devices_tr (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `element_id` INTEGER \
     REFERENCES devices(id) ON DELETE CASCADE NOT NULL, \
     `locale` TEXT NOT NULL, \
     `name` TEXT, \
     `description` TEXT, \
     `keywords` TEXT, \
     UNIQUE(element_id, locale)\
     )",
    "CREATE TABLE IF NOT EXISTS devices_cat (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `element_id` INTEGER \
     REFERENCES devices(id) ON DELETE CASCADE NOT NULL, \
     `category_uuid` TEXT NOT NULL, \
     UNIQUE(element_id, category_uuid)\
     )",
    "CREATE TABLE IF NOT EXISTS devices_res (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `element_id` INTEGER \
     REFERENCES devices(id) ON DELETE CASCADE NOT NULL, \
     `name` TEXT NOT NULL, \
     `media_type` TEXT NOT NULL, \
     `url` TEXT\
     )",
    // parts
    "CREATE TABLE IF NOT EXISTS parts (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `device_id` INTEGER REFERENCES devices(id) ON DELETE CASCADE NOT NULL, \
     `mpn` TEXT NOT NULL, \
     `manufacturer` TEXT NOT NULL \
     )",
    "CREATE TABLE IF NOT EXISTS parts_attr (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `part_id` INTEGER REFERENCES parts(id) ON DELETE CASCADE NOT NULL, \
     `key` TEXT NOT NULL, \
     `type` TEXT NOT NULL, \
     `value` TEXT NOT NULL, \
     `unit` TEXT\
     )",
    // organizations
    "CREATE TABLE IF NOT EXISTS organizations (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `library_id` INTEGER NOT NULL, \
     `filepath` TEXT UNIQUE NOT NULL, \
     `uuid` TEXT NOT NULL, \
     `version` TEXT NOT NULL, \
     `deprecated` BOOLEAN NOT NULL, \
     `logo_png` BLOB, \
     `url` TEXT, \
     `country` TEXT NOT NULL, \
     `fabs` TEXT NOT NULL, \
     `shipping` TEXT NOT NULL, \
     `sponsor` BOOL NOT NULL, \
     `priority` INTEGER NOT NULL \
     )",
    "CREATE TABLE IF NOT EXISTS organizations_tr (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `element_id` INTEGER \
     REFERENCES organizations(id) ON DELETE CASCADE NOT NULL, \
     `locale` TEXT NOT NULL, \
     `name` TEXT, \
     `description` TEXT, \
     `keywords` TEXT, \
     UNIQUE(element_id, locale)\
     )",
    "CREATE TABLE IF NOT EXISTS organization_pcb_design_rules (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `organization_id` INTEGER REFERENCES organizations(id) \
     ON DELETE CASCADE NOT NULL, \
     `uuid` TEXT NOT NULL, \
     `name` TEXT NOT NULL, \
     `description` TEXT NOT NULL, \
     `url` TEXT, \
     `max_layers` INTEGER NOT NULL \
     )",
    "CREATE TABLE IF NOT EXISTS organization_output_jobs (\
     `id` INTEGER PRIMARY KEY NOT NULL, \
     `organization_id` INTEGER REFERENCES organizations(id) \
     ON DELETE CASCADE NOT NULL, \
     `kind` INTEGER NOT NULL, \
     `uuid` TEXT NOT NULL, \
     `type` TEXT NOT NULL, \
     `name` TEXT NOT NULL \
     )",
];

/// Returns `None` for empty strings (upstream `nonEmptyOrNull()`).
fn non_empty_or_null(s: &str) -> Option<&str> {
    (!s.is_empty()).then_some(s)
}

/// Returns `None` for empty byte arrays (the Qt SQLite driver binds a null
/// `QByteArray` as `NULL`).
fn null_if_empty(data: &[u8]) -> Option<&[u8]> {
    (!data.is_empty()).then_some(data)
}

/// Writer of the workspace library database (see the [module docs](self)).
///
/// File paths are stored relative to the libraries root directory.
pub struct LibraryDbWriter<'a> {
    libraries_root: FilePath,
    db: &'a SqliteDatabase,
}

impl<'a> LibraryDbWriter<'a> {
    /// Creates a writer for `db`, storing paths relative to
    /// `libraries_root`.
    pub fn new(libraries_root: &FilePath, db: &'a SqliteDatabase) -> Self {
        Self {
            libraries_root: libraries_root.clone(),
            db,
        }
    }

    fn file_path_to_string(&self, fp: &FilePath) -> String {
        fp.to_relative(&self.libraries_root)
    }

    /// Creates all tables (only needed once after creating a new database).
    pub fn create_all_tables(&self) -> Result<()> {
        for query in CREATE_TABLES {
            self.db.exec(query)?;
        }
        Ok(())
    }

    /// Adds an integer to the `internal` table.
    pub fn add_internal_data(&self, key: &str, value: i64) -> Result<()> {
        let mut query = self.db.prepare(
            "INSERT INTO internal (key, value_int) VALUES (:key, :version)",
            &[],
        )?;
        self.db
            .insert(&mut query, named_params! {":key": key, ":version": value})?;
        Ok(())
    }

    /// Adds a library and returns its ID.
    pub fn add_library(
        &self,
        fp: &FilePath,
        uuid: Uuid,
        version: &Version,
        deprecated: bool,
        icon_png: &[u8],
        manufacturer: &str,
    ) -> Result<i64> {
        let mut query = self.db.prepare(
            "INSERT INTO libraries \
             (filepath, uuid, version, deprecated, icon_png, manufacturer) VALUES \
             (:filepath, :uuid, :version, :deprecated, :icon_png, :manufacturer)",
            &[],
        )?;
        Ok(self.db.insert(
            &mut query,
            named_params! {
                ":filepath": self.file_path_to_string(fp),
                ":uuid": uuid.to_string(),
                ":version": version.to_string(),
                ":deprecated": deprecated,
                ":icon_png": null_if_empty(icon_png),
                ":manufacturer": manufacturer,
            },
        )?)
    }

    /// Updates the metadata of a library (identified by its path).
    pub fn update_library(
        &self,
        fp: &FilePath,
        uuid: Uuid,
        version: &Version,
        deprecated: bool,
        icon_png: &[u8],
        manufacturer: &str,
    ) -> Result<()> {
        let mut query = self.db.prepare(
            "UPDATE libraries \
             SET uuid = :uuid, version = :version, deprecated = :deprecated, \
             icon_png = :icon_png, manufacturer = :manufacturer \
             WHERE filepath = :filepath",
            &[],
        )?;
        query.execute(named_params! {
            ":filepath": self.file_path_to_string(fp),
            ":uuid": uuid.to_string(),
            ":version": version.to_string(),
            ":deprecated": deprecated,
            ":icon_png": null_if_empty(icon_png),
            ":manufacturer": manufacturer,
        })?;
        Ok(())
    }

    /// Adds a symbol, package or component and returns its ID.
    #[allow(clippy::too_many_arguments)]
    pub fn add_element(
        &self,
        kind: ElementKind,
        lib_id: i64,
        fp: &FilePath,
        uuid: Uuid,
        version: &Version,
        deprecated: bool,
        generated_by: &str,
    ) -> Result<i64> {
        if !matches!(
            kind,
            ElementKind::Symbol | ElementKind::Package | ElementKind::Component
        ) {
            return Err(Error::UnsupportedElementKind(kind));
        }
        let mut query = self.db.prepare(
            "INSERT INTO %elements \
             (library_id, filepath, uuid, version, deprecated, generated_by) VALUES \
             (:library_id, :filepath, :uuid, :version, :deprecated, :generated_by)",
            &[("%elements", kind.table())],
        )?;
        Ok(self.db.insert(
            &mut query,
            named_params! {
                ":library_id": lib_id,
                ":filepath": self.file_path_to_string(fp),
                ":uuid": uuid.to_string(),
                ":version": version.to_string(),
                ":deprecated": deprecated,
                ":generated_by": non_empty_or_null(generated_by),
            },
        )?)
    }

    /// Adds a component or package category and returns its ID.
    #[allow(clippy::too_many_arguments)]
    pub fn add_category(
        &self,
        kind: ElementKind,
        lib_id: i64,
        fp: &FilePath,
        uuid: Uuid,
        version: &Version,
        deprecated: bool,
        parent: Option<Uuid>,
    ) -> Result<i64> {
        if !kind.is_category() {
            return Err(Error::UnsupportedElementKind(kind));
        }
        let mut query = self.db.prepare(
            "INSERT INTO %categories \
             (library_id, filepath, uuid, version, deprecated, parent_uuid) VALUES \
             (:library_id, :filepath, :uuid, :version, :deprecated, :parent_uuid)",
            &[("%categories", kind.table())],
        )?;
        Ok(self.db.insert(
            &mut query,
            named_params! {
                ":library_id": lib_id,
                ":filepath": self.file_path_to_string(fp),
                ":uuid": uuid.to_string(),
                ":version": version.to_string(),
                ":deprecated": deprecated,
                ":parent_uuid": parent.map(|p| p.to_string()),
            },
        )?)
    }

    /// Adds a device and returns its ID.
    #[allow(clippy::too_many_arguments)]
    pub fn add_device(
        &self,
        lib_id: i64,
        fp: &FilePath,
        uuid: Uuid,
        version: &Version,
        deprecated: bool,
        generated_by: &str,
        component: Uuid,
        package: Uuid,
    ) -> Result<i64> {
        let mut query = self.db.prepare(
            "INSERT INTO devices \
             (library_id, filepath, uuid, version, deprecated, generated_by, \
             component_uuid, package_uuid) VALUES \
             (:library_id, :filepath, :uuid, :version, :deprecated, :generated_by, \
             :component_uuid, :package_uuid)",
            &[],
        )?;
        Ok(self.db.insert(
            &mut query,
            named_params! {
                ":library_id": lib_id,
                ":filepath": self.file_path_to_string(fp),
                ":uuid": uuid.to_string(),
                ":version": version.to_string(),
                ":deprecated": deprecated,
                ":generated_by": non_empty_or_null(generated_by),
                ":component_uuid": component.to_string(),
                ":package_uuid": package.to_string(),
            },
        )?)
    }

    /// Adds a part of a device and returns its ID.
    pub fn add_part(&self, dev_id: i64, mpn: &str, manufacturer: &str) -> Result<i64> {
        let mut query = self.db.prepare(
            "INSERT INTO parts \
             (device_id, mpn, manufacturer) VALUES \
             (:device_id, :mpn, :manufacturer)",
            &[],
        )?;
        Ok(self.db.insert(
            &mut query,
            named_params! {
                ":device_id": dev_id,
                ":mpn": mpn,
                ":manufacturer": manufacturer,
            },
        )?)
    }

    /// Adds an attribute of a part and returns its ID.
    pub fn add_part_attribute(&self, part_id: i64, attribute: &Attribute) -> Result<i64> {
        let mut query = self.db.prepare(
            "INSERT INTO parts_attr \
             (part_id, key, type, value, unit) VALUES \
             (:part_id, :key, :type, :value, :unit)",
            &[],
        )?;
        Ok(self.db.insert(
            &mut query,
            named_params! {
                ":part_id": part_id,
                ":key": attribute.key().as_str(),
                ":type": attribute.attribute_type().name(),
                ":value": attribute.value(),
                ":unit": attribute.unit().map(|u| u.name()),
            },
        )?)
    }

    /// Adds an organization and returns its ID.
    #[allow(clippy::too_many_arguments)]
    pub fn add_organization(
        &self,
        lib_id: i64,
        fp: &FilePath,
        uuid: Uuid,
        version: &Version,
        deprecated: bool,
        logo_png: &[u8],
        url: &str,
        country: &str,
        fabs: &[String],
        shipping: &[String],
        is_sponsor: bool,
        priority: i32,
    ) -> Result<i64> {
        let mut query = self.db.prepare(
            "INSERT INTO organizations \
             (library_id, filepath, uuid, version, deprecated, logo_png, url, \
             country, fabs, shipping, sponsor, priority) VALUES \
             (:library_id, :filepath, :uuid, :version, :deprecated, :logo_png, \
             :url, :country, :fabs, :shipping, :sponsor, :priority)",
            &[],
        )?;
        Ok(self.db.insert(
            &mut query,
            named_params! {
                ":library_id": lib_id,
                ":filepath": self.file_path_to_string(fp),
                ":uuid": uuid.to_string(),
                ":version": version.to_string(),
                ":deprecated": deprecated,
                ":logo_png": null_if_empty(logo_png),
                ":url": url,
                ":country": country,
                ":fabs": fabs.join(","),
                ":shipping": shipping.join(","),
                ":sponsor": is_sponsor,
                ":priority": priority,
            },
        )?)
    }

    /// Adds PCB design rules of an organization and returns their ID.
    pub fn add_organization_pcb_design_rules(
        &self,
        org_id: i64,
        uuid: Uuid,
        name: &str,
        description: &str,
        url: &str,
        max_layers: i64,
    ) -> Result<i64> {
        let mut query = self.db.prepare(
            "INSERT INTO organization_pcb_design_rules \
             (organization_id, uuid, name, description, url, max_layers) VALUES \
             (:organization_id, :uuid, :name, :description, :url, :max_layers)",
            &[],
        )?;
        Ok(self.db.insert(
            &mut query,
            named_params! {
                ":organization_id": org_id,
                ":uuid": uuid.to_string(),
                ":name": name,
                ":description": description,
                ":url": url,
                ":max_layers": max_layers,
            },
        )?)
    }

    /// Adds an output job of an organization and returns its ID.
    pub fn add_organization_output_job(
        &self,
        org_id: i64,
        kind: OutputJobKind,
        uuid: Uuid,
        job_type: &str,
        name: &str,
    ) -> Result<i64> {
        let mut query = self.db.prepare(
            "INSERT INTO organization_output_jobs \
             (organization_id, kind, uuid, type, name) VALUES \
             (:organization_id, :kind, :uuid, :type, :name)",
            &[],
        )?;
        Ok(self.db.insert(
            &mut query,
            named_params! {
                ":organization_id": org_id,
                ":kind": kind.to_db(),
                ":uuid": uuid.to_string(),
                ":type": job_type,
                ":name": name,
            },
        )?)
    }

    /// Removes an element (and its translations etc.).
    pub fn remove_element(&self, kind: ElementKind, fp: &FilePath) -> Result<()> {
        let mut query = self.db.prepare(
            "DELETE FROM %elements WHERE filepath = :filepath",
            &[("%elements", kind.table())],
        )?;
        query.execute(named_params! {":filepath": self.file_path_to_string(fp)})?;
        Ok(())
    }

    /// Removes all elements of a kind (and their translations etc.).
    pub fn remove_all_elements(&self, kind: ElementKind) -> Result<()> {
        Ok(self.db.clear_table(kind.table())?)
    }

    /// Adds a translation of an element and returns its ID.
    pub fn add_translation(
        &self,
        kind: ElementKind,
        element_id: i64,
        locale: &str,
        name: Option<&str>,
        description: Option<&str>,
        keywords: Option<&str>,
    ) -> Result<i64> {
        let mut query = self.db.prepare(
            "INSERT INTO %elements_tr \
             (element_id, locale, name, description, keywords) VALUES \
             (:element_id, :locale, :name, :description, :keywords)",
            &[("%elements", kind.table())],
        )?;
        Ok(self.db.insert(
            &mut query,
            named_params! {
                ":element_id": element_id,
                ":locale": locale,
                ":name": name,
                ":description": description,
                ":keywords": keywords,
            },
        )?)
    }

    /// Removes all translations of a kind.
    pub fn remove_all_translations(&self, kind: ElementKind) -> Result<()> {
        Ok(self.db.clear_table(&format!("{}_tr", kind.table()))?)
    }

    /// Assigns an element (symbol, package, component or device) to a
    /// category and returns the ID of the assignment.
    pub fn add_to_category(
        &self,
        kind: ElementKind,
        element_id: i64,
        category: Uuid,
    ) -> Result<i64> {
        if kind.category_kind().is_none() {
            return Err(Error::UnsupportedElementKind(kind));
        }
        let mut query = self.db.prepare(
            "INSERT INTO %elements_cat \
             (element_id, category_uuid) VALUES \
             (:element_id, :category_uuid)",
            &[("%elements", kind.table())],
        )?;
        Ok(self.db.insert(
            &mut query,
            named_params! {
                ":element_id": element_id,
                ":category_uuid": category.to_string(),
            },
        )?)
    }

    /// Adds a resource of a component or device and returns its ID.
    pub fn add_resource(
        &self,
        kind: ElementKind,
        element_id: i64,
        name: &str,
        media_type: &str,
        url: &str,
    ) -> Result<i64> {
        if !kind.has_resources() {
            return Err(Error::UnsupportedElementKind(kind));
        }
        let mut query = self.db.prepare(
            "INSERT INTO %elements_res \
             (element_id, name, media_type, url) VALUES \
             (:element_id, :name, :media_type, :url)",
            &[("%elements", kind.table())],
        )?;
        Ok(self.db.insert(
            &mut query,
            named_params! {
                ":element_id": element_id,
                ":name": name,
                ":media_type": media_type,
                ":url": url,
            },
        )?)
    }

    /// Adds an alternative name of a package and returns its ID.
    pub fn add_alternative_name(&self, pkg_id: i64, name: &str, reference: &str) -> Result<i64> {
        let mut query = self.db.prepare(
            "INSERT INTO packages_alt \
             (package_id, name, reference) VALUES \
             (:package_id, :name, :reference)",
            &[],
        )?;
        Ok(self.db.insert(
            &mut query,
            named_params! {
                ":package_id": pkg_id,
                ":name": name,
                ":reference": reference,
            },
        )?)
    }
}
