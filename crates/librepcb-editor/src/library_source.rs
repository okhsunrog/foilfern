//! Where library elements come from when they are added to a project
//! library (upstream: `WorkspaceLibraryDb::getLatest<T>()`).
//!
//! [`LibraryElementSource`] is implemented by the workspace library
//! database ([`LibraryDb`]) and by [`DirectoryLibrarySource`], a simple
//! implementation over a list of element directories or library
//! (`*.lplib`) directories, e.g. for tests and the standalone MCP server.

use std::collections::BTreeMap;

use librepcb_core::fileio::FilePath;
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::cmp::Component;
use librepcb_core::library::dev::Device;
use librepcb_core::library::pkg::Package;
use librepcb_core::library::sym::Symbol;
use librepcb_core::project::LibraryElementKind;
use librepcb_core::serialization::{Mode, SExpression};
use librepcb_core::types::{Uuid, Version};
use librepcb_core::workspace::{ElementKind, LibraryDb};

use crate::error::{Error, Result};

/// Resolves library element UUIDs to the directory of their latest version
/// (upstream `WorkspaceLibraryDb::getLatest<ElementType>()`).
pub trait LibraryElementSource: Send + Sync {
    /// Returns the directory of the latest version of the element, or
    /// `None` if it is unknown.
    fn element_directory(&self, kind: LibraryElementKind, uuid: &Uuid) -> Option<FilePath>;

    /// Returns the devices of a component (upstream
    /// `WorkspaceLibraryDb::getComponentDevices()`), e.g. for the "change
    /// device" menu. The default implementation knows none.
    fn component_devices(&self, component: &Uuid) -> Vec<Uuid> {
        let _ = component;
        Vec::new()
    }
}

/// A source without any elements (only the project library can be used).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoLibrarySource;

impl LibraryElementSource for NoLibrarySource {
    fn element_directory(&self, _kind: LibraryElementKind, _uuid: &Uuid) -> Option<FilePath> {
        None
    }
}

/// Returns the long element name (file name without `.lp`, root node
/// suffix) of `kind`.
pub(crate) fn long_element_name(kind: LibraryElementKind) -> &'static str {
    match kind {
        LibraryElementKind::Symbol => Symbol::LONG_ELEMENT_NAME,
        LibraryElementKind::Package => Package::LONG_ELEMENT_NAME,
        LibraryElementKind::Component => Component::LONG_ELEMENT_NAME,
        LibraryElementKind::Device => Device::LONG_ELEMENT_NAME,
    }
}

const ALL_KINDS: [LibraryElementKind; 4] = [
    LibraryElementKind::Symbol,
    LibraryElementKind::Package,
    LibraryElementKind::Component,
    LibraryElementKind::Device,
];

/// A [`LibraryElementSource`] over explicitly added element directories.
///
/// If an element exists several times, the highest version wins (upstream
/// `getLatest()`); on equal versions the first added directory wins.
#[derive(Debug, Clone, Default)]
pub struct DirectoryLibrarySource {
    elements: BTreeMap<(LibraryElementKind, Uuid), (Version, FilePath)>,
}

impl DirectoryLibrarySource {
    /// Creates an empty source.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a source with all elements of the given library (`*.lplib`)
    /// directories.
    pub fn from_libraries<'a>(libraries: impl IntoIterator<Item = &'a FilePath>) -> Result<Self> {
        let mut source = Self::new();
        for library in libraries {
            source.add_library(library)?;
        }
        Ok(source)
    }

    /// Adds the element in `dir` (a directory containing
    /// `.librepcb-<sym|pkg|cmp|dev>` and the element file). Returns its
    /// UUID.
    pub fn add_element(&mut self, kind: LibraryElementKind, dir: &FilePath) -> Result<Uuid> {
        let file = dir.path_to(&format!("{}.lp", long_element_name(kind)));
        let content = std::fs::read(file.as_path()).map_err(|e| {
            Error::InvalidArgument(format!("Could not read {}: {e}", file.to_native()))
        })?;
        let root = SExpression::parse(&content, Some(file.as_path()), Mode::LibrePcb)?;
        let uuid: Uuid = root.child_value("@0")?;
        let version: Version = root.child_value("version/@0")?;
        match self.elements.get(&(kind, uuid)) {
            Some((existing, _)) if *existing >= version => {}
            _ => {
                self.elements.insert((kind, uuid), (version, dir.clone()));
            }
        }
        Ok(uuid)
    }

    /// Adds all elements of a library directory (`sym/`, `pkg/`, `cmp/` and
    /// `dev/` subdirectories); directories which are not valid elements
    /// are skipped with a warning. Returns the number of added elements.
    pub fn add_library(&mut self, library: &FilePath) -> Result<usize> {
        let mut count = 0;
        for kind in ALL_KINDS {
            let dir = library.path_to(kind.directory_name());
            let Ok(entries) = std::fs::read_dir(dir.as_path()) else {
                continue;
            };
            let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
            paths.sort();
            for path in paths {
                let Some(element_dir) = FilePath::new(&path) else {
                    continue;
                };
                let version_file =
                    element_dir.path_to(&format!(".librepcb-{}", kind.directory_name()));
                if !version_file.is_existing_file() {
                    continue;
                }
                match self.add_element(kind, &element_dir) {
                    Ok(_) => count += 1,
                    Err(e) => {
                        log::warn!("Skipping library element {}: {e}", element_dir.to_native())
                    }
                }
            }
        }
        Ok(count)
    }

    /// Returns all known elements with the directory of their latest
    /// version.
    pub fn elements(&self) -> impl Iterator<Item = (LibraryElementKind, Uuid, &FilePath)> {
        self.elements
            .iter()
            .map(|((kind, uuid), (_, dir))| (*kind, *uuid, dir))
    }
}

impl LibraryElementSource for DirectoryLibrarySource {
    fn element_directory(&self, kind: LibraryElementKind, uuid: &Uuid) -> Option<FilePath> {
        self.elements
            .get(&(kind, *uuid))
            .map(|(_, dir)| dir.clone())
    }

    /// Reads the component UUID of every known device (unreadable devices
    /// are skipped).
    fn component_devices(&self, component: &Uuid) -> Vec<Uuid> {
        let kind = LibraryElementKind::Device;
        self.elements
            .iter()
            .filter(|((k, _), _)| *k == kind)
            .filter_map(|((_, uuid), (_, dir))| {
                let file = dir.path_to(&format!("{}.lp", long_element_name(kind)));
                let content = std::fs::read(file.as_path()).ok()?;
                let root =
                    SExpression::parse(&content, Some(file.as_path()), Mode::LibrePcb).ok()?;
                let cmp: Uuid = root.child_value("component/@0").ok()?;
                (cmp == *component).then_some(*uuid)
            })
            .collect()
    }
}

/// The workspace library database (upstream `WorkspaceLibraryDb`): the
/// latest version of an element in all workspace libraries. Database
/// errors are logged and reported as "unknown element".
impl LibraryElementSource for LibraryDb {
    fn element_directory(&self, kind: LibraryElementKind, uuid: &Uuid) -> Option<FilePath> {
        let kind = match kind {
            LibraryElementKind::Symbol => ElementKind::Symbol,
            LibraryElementKind::Package => ElementKind::Package,
            LibraryElementKind::Component => ElementKind::Component,
            LibraryElementKind::Device => ElementKind::Device,
        };
        match self.element_dir(kind, *uuid) {
            Ok(dir) => dir,
            Err(e) => {
                log::error!("Failed to query the workspace library database: {e}");
                None
            }
        }
    }

    fn component_devices(&self, component: &Uuid) -> Vec<Uuid> {
        match LibraryDb::component_devices(self, *component) {
            Ok(devices) => devices.into_iter().collect(),
            Err(e) => {
                log::error!("Failed to query the workspace library database: {e}");
                Vec::new()
            }
        }
    }
}
