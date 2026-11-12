//! Port of libs/librepcb/core/project/projectlibrary.{h,cpp}.
//!
//! The project library holds project-local copies of the library elements
//! used by the project, each in `library/<sym|pkg|cmp|dev>/<uuid>/` of the
//! project directory. Elements are the [`library`](crate::library) types
//! and own their [`TransactionalDirectory`] like upstream.
//!
//! Differences to upstream: elements are stored by value in UUID ordered
//! maps (upstream `QHash<Uuid, T*>`, order irrelevant); adding and
//! removing go through [`Project`](super::Project) so that they are
//! journaled.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::error::{EntityKind, Error, Result};
use crate::fileio::TransactionalDirectory;
use crate::library::LibraryBaseElement;
use crate::library::cmp::Component;
use crate::library::dev::Device;
use crate::library::pkg::Package;
use crate::library::sym::Symbol;
use crate::types::Uuid;

/// Kind of a library element in the project library.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum LibraryElementKind {
    /// A symbol (`library/sym/`).
    Symbol,
    /// A package (`library/pkg/`).
    Package,
    /// A component (`library/cmp/`).
    Component,
    /// A device (`library/dev/`).
    Device,
}

impl LibraryElementKind {
    /// The subdirectory of `library/`, e.g. `"sym"`.
    pub fn directory_name(self) -> &'static str {
        match self {
            Self::Symbol => Symbol::SHORT_ELEMENT_NAME,
            Self::Package => Package::SHORT_ELEMENT_NAME,
            Self::Component => Component::SHORT_ELEMENT_NAME,
            Self::Device => Device::SHORT_ELEMENT_NAME,
        }
    }
}

/// The project-local library elements.
#[derive(Debug)]
pub struct ProjectLibrary {
    directory: TransactionalDirectory,
    symbols: BTreeMap<Uuid, Symbol>,
    packages: BTreeMap<Uuid, Package>,
    components: BTreeMap<Uuid, Component>,
    devices: BTreeMap<Uuid, Device>,
}

static_assertions::assert_impl_all!(ProjectLibrary: Send, Sync);

impl ProjectLibrary {
    /// Creates an empty library in `directory` (the `library/` directory of
    /// the project).
    pub(crate) fn new(directory: TransactionalDirectory) -> Self {
        Self {
            directory,
            symbols: BTreeMap::new(),
            packages: BTreeMap::new(),
            components: BTreeMap::new(),
            devices: BTreeMap::new(),
        }
    }

    /// Returns the `library/` directory.
    pub fn directory(&self) -> &TransactionalDirectory {
        &self.directory
    }

    pub(crate) fn directory_mut(&mut self) -> &mut TransactionalDirectory {
        &mut self.directory
    }

    /// Returns all symbols.
    pub fn symbols(&self) -> &BTreeMap<Uuid, Symbol> {
        &self.symbols
    }

    /// Returns all packages.
    pub fn packages(&self) -> &BTreeMap<Uuid, Package> {
        &self.packages
    }

    /// Returns all components.
    pub fn components(&self) -> &BTreeMap<Uuid, Component> {
        &self.components
    }

    /// Returns all devices.
    pub fn devices(&self) -> &BTreeMap<Uuid, Device> {
        &self.devices
    }

    /// Returns the symbol with the given UUID.
    pub fn symbol(&self, uuid: &Uuid) -> Option<&Symbol> {
        self.symbols.get(uuid)
    }

    /// Returns the package with the given UUID.
    pub fn package(&self, uuid: &Uuid) -> Option<&Package> {
        self.packages.get(uuid)
    }

    /// Returns the component with the given UUID.
    pub fn component(&self, uuid: &Uuid) -> Option<&Component> {
        self.components.get(uuid)
    }

    /// Returns the device with the given UUID.
    pub fn device(&self, uuid: &Uuid) -> Option<&Device> {
        self.devices.get(uuid)
    }

    /// Returns all devices of a component (upstream
    /// `getDevicesOfComponent()`).
    pub fn devices_of_component(&self, component: &Uuid) -> Vec<&Device> {
        self.devices
            .values()
            .filter(|d| d.component_uuid() == *component)
            .collect()
    }

    /// Whether an element with the given kind and UUID exists.
    pub fn contains(&self, kind: LibraryElementKind, uuid: &Uuid) -> bool {
        match kind {
            LibraryElementKind::Symbol => self.symbols.contains_key(uuid),
            LibraryElementKind::Package => self.packages.contains_key(uuid),
            LibraryElementKind::Component => self.components.contains_key(uuid),
            LibraryElementKind::Device => self.devices.contains_key(uuid),
        }
    }

    pub(crate) fn add_symbol(&mut self, symbol: Symbol) -> Result<()> {
        add_element(&mut self.directory, &mut self.symbols, symbol)
    }

    pub(crate) fn add_package(&mut self, package: Package) -> Result<()> {
        add_element(&mut self.directory, &mut self.packages, package)
    }

    pub(crate) fn add_component(&mut self, component: Component) -> Result<()> {
        add_element(&mut self.directory, &mut self.components, component)
    }

    pub(crate) fn add_device(&mut self, device: Device) -> Result<()> {
        add_element(&mut self.directory, &mut self.devices, device)
    }

    pub(crate) fn remove_symbol(&mut self, uuid: &Uuid) -> Result<Symbol> {
        remove_element(&mut self.symbols, uuid)
    }

    pub(crate) fn remove_package(&mut self, uuid: &Uuid) -> Result<Package> {
        remove_element(&mut self.packages, uuid)
    }

    pub(crate) fn remove_component(&mut self, uuid: &Uuid) -> Result<Component> {
        remove_element(&mut self.components, uuid)
    }

    pub(crate) fn remove_device(&mut self, uuid: &Uuid) -> Result<Device> {
        remove_element(&mut self.devices, uuid)
    }
}

/// Adds `element` (upstream `addElement()`): elements from another file
/// system are copied into `library/<short name>/<uuid>/`, elements already
/// in the project's file system are saved in place.
fn add_element<E: LibraryBaseElement>(
    directory: &mut TransactionalDirectory,
    map: &mut BTreeMap<Uuid, E>,
    mut element: E,
) -> Result<()> {
    let uuid = element.metadata().uuid();
    if map.contains_key(&uuid) {
        return Err(Error::DuplicateLibraryElement(uuid));
    }
    let fs = Arc::clone(directory.file_system());
    let overlay = fs.snapshot_overlay();
    let result = if Arc::ptr_eq(element.directory().file_system(), &fs) {
        element.save()
    } else {
        let mut dir = directory.subdir(E::SHORT_ELEMENT_NAME);
        element.save_into_parent_directory(&mut dir)
    };
    if let Err(error) = result {
        fs.restore_overlay(overlay);
        return Err(error.into());
    }
    map.insert(uuid, element);
    Ok(())
}

/// Removes the element (upstream `removeElement()`): its files are moved
/// into a temporary file system so that the returned element stays
/// intact (and can be added again).
fn remove_element<E: LibraryBaseElement>(map: &mut BTreeMap<Uuid, E>, uuid: &Uuid) -> Result<E> {
    let element = map.get_mut(uuid).ok_or(Error::NotFound {
        kind: EntityKind::LibraryElement,
        uuid: *uuid,
    })?;
    let mut tmp = TransactionalDirectory::new_temporary()?;
    let fs = Arc::clone(element.directory().file_system());
    let overlay = fs.snapshot_overlay();
    let path = element.directory().path().to_owned();
    if let Err(error) = element.move_into_parent_directory(&mut tmp) {
        *element.directory_mut() = TransactionalDirectory::new(Arc::clone(&fs), &path);
        fs.restore_overlay(overlay);
        return Err(error.into());
    }
    Ok(map.remove(uuid).expect("element was found above"))
}
