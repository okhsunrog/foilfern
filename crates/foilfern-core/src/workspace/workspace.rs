//! Port of libs/librepcb/core/workspace/workspace.{h,cpp}.
//!
//! A workspace is a directory with the version file `.librepcb-workspace`,
//! a `projects` directory and one or more data directories: `data` (the
//! default) and `v<version>` (backups or data of other file format
//! versions). A data directory contains:
//!
//! - `.librepcb-data`: the file format version of the data directory;
//! - `settings.lp`: the [`WorkspaceSettings`];
//! - `libraries/local/*.lplib`: libraries created or copied by the user;
//! - `libraries/remote/<uuid>.lplib`: libraries installed from the API
//!   server;
//! - `libraries/cache_v8.sqlite`: the [`LibraryDb`] (library index).
//!
//! While a workspace is open, its data directory is locked (upstream
//! compatible `.lock` file), so the workspace can't be opened by upstream
//! LibrePCB (or another instance) at the same time.
//!
//! Differences to upstream:
//! - The most recently used workspace path (stored in `QSettings`) is not
//!   ported; it belongs to the application.
//! - [`Workspace::check_compatibility()`] returns a `Result` instead of a
//!   `bool` plus error message.
//! - Added [`Workspace::open_or_create()`], which performs the steps of the
//!   upstream workspace initialization wizard without user interaction.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::error::{Error, Result};
use super::library_db::LibraryDb;
use super::settings::WorkspaceSettings;
use crate::application;
use crate::fileio::{
    FilePath, LockHandler, RestoreMode, TransactionalDirectory, TransactionalFileSystem,
    VersionFile, file_utils,
};
use crate::serialization::{Mode, SExpression, file_format_migrations};
use crate::types::Version;

/// Name of the version file in the workspace root.
const VERSION_FILE_NAME: &str = ".librepcb-workspace";

/// Name of the version file in a data directory.
const DATA_VERSION_FILE_NAME: &str = ".librepcb-data";

/// Name of the settings file in a data directory.
const SETTINGS_FILE_NAME: &str = "settings.lp";

/// Name of the default data directory.
pub const DEFAULT_DATA_DIR_NAME: &str = "data";

/// Returns the workspace file format version (upstream
/// `Workspace::FILE_FORMAT_VERSION()`, constant `0.1`).
pub fn workspace_file_format_version() -> Version {
    // Invariant: "0.1" is a valid version number.
    "0.1".parse().expect("valid version")
}

/// The data directory to open and whether it has to be created by copying
/// another data directory first (upstream `determineDataDirectory()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDirectoryChoice {
    /// Name of the data directory to open or create.
    pub dir: String,
    /// If the file format needs to be upgraded (or imported from an older
    /// data directory): copy the directory `.0` to `.1` before opening.
    pub copy: Option<(String, String)>,
}

/// An open workspace (see the [module docs](self)).
pub struct Workspace {
    path: FilePath,
    projects_path: FilePath,
    data_path: FilePath,
    libraries_path: FilePath,
    file_system: Arc<TransactionalFileSystem>,
    settings: WorkspaceSettings,
    library_db: Arc<LibraryDb>,
}

static_assertions::assert_impl_all!(Workspace: Send, Sync);

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("path", &self.path)
            .field("data_path", &self.data_path)
            .finish_non_exhaustive()
    }
}

impl Workspace {
    /// Opens an existing workspace with the data directory `data_dir_name`
    /// (e.g. `"data"` or `"v2"`, created if it does not exist).
    ///
    /// The data directory is locked; `lock_handler` decides what to do if it
    /// is locked already (`None`: fail). An outdated data directory is
    /// upgraded to the current file format.
    pub fn open(
        ws_path: &FilePath,
        data_dir_name: &str,
        lock_handler: Option<LockHandler<'_>>,
    ) -> Result<Self> {
        let path = ws_path.clone();
        let projects_path = path.path_to("projects");
        let data_path = path.path_to(data_dir_name);
        let libraries_path = data_path.path_to("libraries");
        log::debug!("Open workspace data directory {}...", data_path.to_native());

        // Fail if the path is not a valid workspace directory.
        Self::check_compatibility(&path)?;

        // Ensure that the projects directory exists since several features
        // depend on it.
        file_utils::make_path(&projects_path)?;

        // Access the data directory with TransactionalFileSystem to ensure a
        // failsafe file access and forbid concurrent access by a lock.
        let file_system = Arc::new(TransactionalFileSystem::open(
            &data_path,
            true,
            RestoreMode::Yes,
            lock_handler,
        )?);

        // Check file format of data directory.
        let mut loaded_file_format = workspace_file_format_version();
        if file_system.file_exists(DATA_VERSION_FILE_NAME) {
            let raw = file_system.read(DATA_VERSION_FILE_NAME)?;
            loaded_file_format = VersionFile::from_bytes(&raw)?.version().clone();
            if loaded_file_format > application::file_format_version() {
                return Err(Error::DataDirectoryTooNew(loaded_file_format));
            }
        }

        // Upgrade file format, if needed.
        let mut data_dir = TransactionalDirectory::new(Arc::clone(&file_system), "");
        for migration in file_format_migrations(&loaded_file_format) {
            log::info!(
                "Workspace data file format is outdated, upgrading from v{} to v{}...",
                migration.from_version(),
                migration.to_version()
            );
            migration.upgrade_workspace_data(&mut data_dir)?;
        }

        // Load workspace settings.
        let mut settings = WorkspaceSettings::new();
        if file_system.file_exists(SETTINGS_FILE_NAME) {
            log::debug!("Load workspace settings...");
            let content = file_system.read(SETTINGS_FILE_NAME)?;
            let settings_path = file_system.abs_path(SETTINGS_FILE_NAME);
            let root = SExpression::parse(
                &content,
                settings_path.as_ref().map(FilePath::as_path),
                Mode::LibrePcb,
            )?;
            settings.load(&root, &loaded_file_format);
            log::debug!("Successfully loaded workspace settings.");
        } else {
            log::info!("Workspace settings file not found, default settings will be used.");
        }

        // Write files to disk if an upgrade was performed.
        if loaded_file_format != application::file_format_version() {
            file_system.write(SETTINGS_FILE_NAME, &settings.to_bytes()?)?;
            file_system.save()?;
        }

        // Load library database.
        file_utils::make_path(&libraries_path)?;
        let library_db = Arc::new(LibraryDb::open(&libraries_path)?);

        log::debug!("Successfully opened workspace.");
        Ok(Self {
            path,
            projects_path,
            data_path,
            libraries_path,
            file_system,
            settings,
            library_db,
        })
    }

    /// Opens the workspace at `path`, creating it if the directory does not
    /// exist or is empty, and choosing (and if needed creating or upgrading
    /// by copy) the data directory like the upstream workspace
    /// initialization wizard.
    pub fn open_or_create(path: &FilePath, lock_handler: Option<LockHandler<'_>>) -> Result<Self> {
        if Self::check_compatibility(path).is_err() {
            if path.is_existing_file() || (path.is_existing_dir() && !path.is_empty_dir()) {
                return Err(Error::InvalidWorkspace(path.clone()));
            }
            Self::create(path)?;
        }
        let dirs = Self::find_data_directories(path)?;
        let choice = Self::determine_data_directory(&dirs);
        if let Some((from, to)) = &choice.copy {
            log::info!("Copy workspace data directory {from} to {to}...");
            file_utils::copy_dir_recursively(&path.path_to(from), &path.path_to(to))?;
        }
        Self::open(path, &choice.dir, lock_handler)
    }

    /// Returns the workspace directory.
    pub fn path(&self) -> &FilePath {
        &self.path
    }

    /// Returns the `projects` directory.
    pub fn projects_path(&self) -> &FilePath {
        &self.projects_path
    }

    /// Returns the data directory.
    pub fn data_path(&self) -> &FilePath {
        &self.data_path
    }

    /// Returns the `libraries` directory in the data directory.
    pub fn libraries_path(&self) -> &FilePath {
        &self.libraries_path
    }

    /// Returns the `libraries/local` directory.
    pub fn local_libraries_path(&self) -> FilePath {
        self.libraries_path.path_to("local")
    }

    /// Returns the `libraries/remote` directory.
    pub fn remote_libraries_path(&self) -> FilePath {
        self.libraries_path.path_to("remote")
    }

    /// Returns the settings.
    pub fn settings(&self) -> &WorkspaceSettings {
        &self.settings
    }

    /// Returns the settings for modification (call
    /// [`save_settings()`](Self::save_settings) afterwards).
    pub fn settings_mut(&mut self) -> &mut WorkspaceSettings {
        &mut self.settings
    }

    /// Returns the library database.
    pub fn library_db(&self) -> &LibraryDb {
        &self.library_db
    }

    /// Returns a shared handle of the library database (e.g. for an editor
    /// which copies library elements into a project while the workspace
    /// stays open).
    pub fn shared_library_db(&self) -> Arc<LibraryDb> {
        Arc::clone(&self.library_db)
    }

    /// Saves the (modified) settings to disk.
    pub fn save_settings(&mut self) -> Result<()> {
        log::debug!("Save workspace settings...");
        self.file_system
            .write(SETTINGS_FILE_NAME, &self.settings.to_bytes()?)?;
        self.file_system.save()?;
        Ok(())
    }

    /// Checks the existence and compatibility of a workspace directory
    /// (upstream `checkCompatibility()`).
    pub fn check_compatibility(ws_root: &FilePath) -> Result<()> {
        // Check existence of version file.
        let version_fp = ws_root.path_to(VERSION_FILE_NAME);
        if !version_fp.is_existing_file() {
            return Err(Error::InvalidWorkspace(ws_root.clone()));
        }

        // Check workspace file format.
        let version_file = VersionFile::from_bytes(&file_utils::read_file(&version_fp)?)?;
        if *version_file.version() != workspace_file_format_version() {
            return Err(Error::IncompatibleWorkspace {
                path: ws_root.clone(),
                version: version_file.version().clone(),
            });
        }
        Ok(())
    }

    /// Returns all data directories of a workspace and their (intended)
    /// file format version (upstream `findDataDirectories()`).
    ///
    /// For `v<version>` directories, the version of the directory name is
    /// returned (the directory may be silently upgraded up to that version);
    /// for `data`, the version of its content.
    pub fn find_data_directories(ws_root: &FilePath) -> Result<BTreeMap<String, Version>> {
        let mut result = BTreeMap::new();
        for dir in file_utils::find_directories(ws_root) {
            let name = dir.file_name().to_owned();
            if name.starts_with('.') {
                continue; // Upstream: hidden directories are not listed.
            }
            if name == DEFAULT_DATA_DIR_NAME {
                let fs = TransactionalFileSystem::open(&dir, false, RestoreMode::Yes, None)?;
                let version = if fs.file_exists(DATA_VERSION_FILE_NAME) {
                    VersionFile::from_bytes(&fs.read(DATA_VERSION_FILE_NAME)?)?
                        .version()
                        .clone()
                } else {
                    // File format 0.1 didn't have a version file.
                    workspace_file_format_version()
                };
                result.insert(name, version);
            } else if let Some(version) = name
                .strip_prefix('v')
                .and_then(|v| v.parse::<Version>().ok())
            {
                result.insert(name, version);
            }
        }
        Ok(result)
    }

    /// Decides which data directory to open, and how (upstream
    /// `determineDataDirectory()`).
    pub fn determine_data_directory(data_dirs: &BTreeMap<String, Version>) -> DataDirectoryChoice {
        let file_format = application::file_format_version();
        let versioned_dir_name = format!("v{file_format}");

        // If there's a specific data directory for the current file format,
        // use it.
        if data_dirs.contains_key(&versioned_dir_name) {
            return DataDirectoryChoice {
                dir: versioned_dir_name,
                copy: None,
            };
        }

        // If the default data directory file format can be loaded, use it.
        if let Some(version) = data_dirs.get(DEFAULT_DATA_DIR_NAME)
            && *version <= file_format
        {
            // If the file format needs to be upgraded, a backup should be
            // created. But only if it doesn't exist yet, otherwise we can just
            // do the upgrade.
            let backup_dir = format!("v{version}");
            let copy = (*version < file_format && !data_dirs.contains_key(&backup_dir))
                .then(|| (DEFAULT_DATA_DIR_NAME.to_owned(), backup_dir));
            return DataDirectoryChoice {
                dir: DEFAULT_DATA_DIR_NAME.to_owned(),
                copy,
            };
        }

        // There's no data directory to open, so we have to create a new one.
        let dir = if data_dirs.contains_key(DEFAULT_DATA_DIR_NAME) {
            versioned_dir_name
        } else {
            DEFAULT_DATA_DIR_NAME.to_owned()
        };

        // If there are older file formats available, the latest one should
        // be imported.
        let mut to_import: Option<(&String, &Version)> = None;
        for (name, version) in data_dirs {
            if name != DEFAULT_DATA_DIR_NAME
                && *version < file_format
                && to_import.is_none_or(|(_, v)| version > v)
            {
                to_import = Some((name, version));
            }
        }
        let copy = to_import.map(|(name, _)| (name.clone(), dir.clone()));
        DataDirectoryChoice { dir, copy }
    }

    /// Creates a new workspace (upstream `createNewWorkspace()`): writes the
    /// version file into `path` (created if needed).
    pub fn create(path: &FilePath) -> Result<()> {
        file_utils::write_file(
            &path.path_to(VERSION_FILE_NAME),
            &VersionFile::new(workspace_file_format_version()).to_bytes(),
        )?;
        Ok(())
    }
}
