//! Port of libs/librepcb/core/workspace (workspace, workspace settings and
//! the workspace library database).
//!
//! - [`Workspace`]: opening/creating a workspace directory, its data
//!   directories and settings.
//! - [`WorkspaceSettings`]: the settings file `settings.lp`.
//! - [`LibraryDb`]: the SQLite index of all workspace libraries
//!   (`libraries/cache_v8.sqlite`, shared with upstream LibrePCB), with
//!   queries for elements, translations, categories, parts and
//!   organizations, and a facade for tools and agents
//!   ([`LibraryDb::search()`], [`LibraryDb::element_summary()`],
//!   [`LibraryDb::element_dir()`], [`LibraryDb::category_tree()`]).
//! - [`LibraryDbWriter`]: writes the database.
//! - [`LibraryScanner`]: fills the database by scanning all libraries,
//!   synchronously on a caller-provided thread.
//!
//! Not ported: UI themes and the color scheme logic (`uitheme`,
//! `colorrole`, `basecolorscheme`, `usercolorscheme`); color schemes are
//! kept as raw settings (see [`RawSettings`]).

mod element_kind;
mod error;
mod library_db;
mod library_db_writer;
mod library_scanner;
mod settings;
#[allow(clippy::module_inception)] // Upstream file name.
mod workspace;

pub use element_kind::ElementKind;
pub use error::{Error, Result};
pub use library_db::{
    CURRENT_DB_VERSION, CategoryInfo, CategoryTreeNode, DeviceInfo, ElementInfo, ElementSummary,
    LibraryDb, LibraryInfo, OrganizationInfo, OutputJobInfo, PcbDesignRulesInfo, SearchQuery,
    Translations,
};
pub use library_db_writer::{LibraryDbWriter, OutputJobKind};
pub use library_scanner::{LibraryScanner, ScanEvent, ScanOutcome};
pub use settings::{
    ApiEndpointSettings, KeyboardShortcuts, ListItem, OFFICIAL_API_URL, RawSettings, SettingsItem,
    SettingsValue, WorkspaceSettings,
};
pub use workspace::{
    DEFAULT_DATA_DIR_NAME, DataDirectoryChoice, Workspace, workspace_file_format_version,
};
