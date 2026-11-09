//! Project library commands: port of
//! libs/librepcb/editor/project/cmd/cmdprojectlibraryaddelement.{h,cpp},
//! cmdprojectlibraryremoveelement.{h,cpp} and
//! cmdremoveunusedlibraryelements.{h,cpp}, plus the "copy from the
//! workspace library if missing" steps of `CmdAddComponentToCircuit`,
//! `CmdAddSymbolToSchematic` and `CmdAddDeviceToBoard`.

use std::collections::BTreeSet;
use std::sync::Arc;

use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::cmp::Component;
use librepcb_core::library::dev::Device;
use librepcb_core::library::pkg::Package;
use librepcb_core::library::sym::Symbol;
use librepcb_core::project::{LibraryElementKind, Project};
use librepcb_core::types::Uuid;

use crate::editor::{Command, Transaction};
use crate::error::{Error, Result};
use crate::undo_stack::LibraryElement;

/// Identifies a library element.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct LibraryElementId {
    /// Kind of the element.
    pub kind: LibraryElementKind,
    /// UUID of the element.
    pub uuid: Uuid,
}

/// Opens the library element of `kind` stored in `dir` (read-only; the
/// project library copies its files).
pub(crate) fn open_element(kind: LibraryElementKind, dir: &FilePath) -> Result<LibraryElement> {
    let fs = TransactionalFileSystem::open_ro(dir)?;
    let directory = TransactionalDirectory::new(Arc::new(fs), "");
    Ok(match kind {
        LibraryElementKind::Symbol => LibraryElement::Symbol(Symbol::open(directory)?),
        LibraryElementKind::Package => LibraryElement::Package(Package::open(directory)?),
        LibraryElementKind::Component => LibraryElement::Component(Component::open(directory)?),
        LibraryElementKind::Device => LibraryElement::Device(Device::open(directory)?),
    })
}

/// Adds the element from the library element source to the project
/// library if it is not there yet. Returns whether it was added.
pub(crate) fn ensure_element(
    tx: &mut Transaction<'_>,
    kind: LibraryElementKind,
    uuid: Uuid,
) -> Result<bool> {
    if tx.project().library().contains(kind, &uuid) {
        return Ok(false);
    }
    let dir = tx
        .source()
        .element_directory(kind, &uuid)
        .ok_or(Error::NotInLibrarySource { kind, uuid })?;
    let element = open_element(kind, &dir)?;
    if element.uuid() != uuid {
        return Err(Error::InvalidArgument(format!(
            "The library element in {} has the UUID {} instead of {uuid}.",
            dir.to_native(),
            element.uuid()
        )));
    }
    tx.add_library_element(element)?;
    Ok(true)
}

/// Adds an element and (optionally) its dependencies, recording what was
/// added.
fn ensure_with_dependencies(
    tx: &mut Transaction<'_>,
    kind: LibraryElementKind,
    uuid: Uuid,
    dependencies: bool,
    added: &mut Vec<LibraryElementId>,
) -> Result<()> {
    if ensure_element(tx, kind, uuid)? {
        added.push(LibraryElementId { kind, uuid });
    }
    if !dependencies {
        return Ok(());
    }
    let lib = tx.project().library();
    let deps: Vec<(LibraryElementKind, Uuid)> = match kind {
        LibraryElementKind::Device => {
            let device = lib
                .device(&uuid)
                .ok_or(Error::NotInProjectLibrary { kind, uuid })?;
            vec![
                (LibraryElementKind::Component, device.component_uuid()),
                (LibraryElementKind::Package, device.package_uuid()),
            ]
        }
        LibraryElementKind::Component => {
            let component = lib
                .component(&uuid)
                .ok_or(Error::NotInProjectLibrary { kind, uuid })?;
            component_symbols(component)
                .into_iter()
                .map(|s| (LibraryElementKind::Symbol, s))
                .collect()
        }
        LibraryElementKind::Symbol | LibraryElementKind::Package => Vec::new(),
    };
    for (kind, uuid) in deps {
        ensure_with_dependencies(tx, kind, uuid, true, added)?;
    }
    Ok(())
}

/// Returns the symbols used by all symbol variants of a component.
pub(crate) fn component_symbols(component: &Component) -> BTreeSet<Uuid> {
    component
        .symbol_variants()
        .iter()
        .flat_map(|v| v.all_symbol_uuids())
        .collect()
}

/// Adds a library element from the library element source to the project
/// library (upstream `CmdProjectLibraryAddElement`), by default with its
/// dependencies (device → component and package, component → symbols of
/// all symbol variants). Elements already in the project library are kept.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddLibraryElement {
    /// Kind of the element.
    pub kind: LibraryElementKind,
    /// UUID of the element.
    pub uuid: Uuid,
    /// Whether the dependencies are added too (default: true).
    #[serde(default = "default_true")]
    pub with_dependencies: bool,
}

fn default_true() -> bool {
    true
}

impl AddLibraryElement {
    /// Creates the command (with dependencies).
    pub fn new(kind: LibraryElementKind, uuid: Uuid) -> Self {
        Self {
            kind,
            uuid,
            with_dependencies: true,
        }
    }
}

impl Command for AddLibraryElement {
    /// The elements which were added (those already in the project library
    /// are not listed).
    type Output = Vec<LibraryElementId>;

    fn text(&self) -> String {
        librepcb_i18n::tr!("librepcb::editor", "Add element to library")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Self::Output> {
        let mut added = Vec::new();
        ensure_with_dependencies(tx, self.kind, self.uuid, self.with_dependencies, &mut added)?;
        Ok(added)
    }
}

/// Returns the project library elements which are not used by the project
/// (upstream `CmdRemoveUnusedLibraryElements`): components without
/// instances, devices and packages without board devices, symbols without
/// placed symbols.
pub fn unused_library_elements(p: &Project) -> Vec<LibraryElementId> {
    let mut used_components = BTreeSet::new();
    let mut used_devices = BTreeSet::new();
    let mut used_packages = BTreeSet::new();
    let mut used_symbols = BTreeSet::new();
    for component in p.circuit().component_instances().values() {
        used_components.insert(component.lib_component());
    }
    for board in p.boards() {
        for device in board.devices().values() {
            used_devices.insert(device.lib_device());
            if let Some(lib_device) = p.library().device(&device.lib_device()) {
                used_packages.insert(lib_device.package_uuid());
            }
        }
    }
    for schematic in p.schematics() {
        for symbol in schematic.symbols().values() {
            if let Ok(resolved) = symbol.resolve(p.view()) {
                used_symbols.insert(resolved.lib_symbol.metadata().uuid());
            }
        }
    }
    let lib = p.library();
    let mut unused = Vec::new();
    let mut collect = |kind, uuids: Vec<Uuid>, used: &BTreeSet<Uuid>| {
        unused.extend(
            uuids
                .into_iter()
                .filter(|u| !used.contains(u))
                .map(|uuid| LibraryElementId { kind, uuid }),
        );
    };
    // Same order as upstream (symbols, packages, devices, components).
    collect(
        LibraryElementKind::Symbol,
        lib.symbols().keys().copied().collect(),
        &used_symbols,
    );
    collect(
        LibraryElementKind::Package,
        lib.packages().keys().copied().collect(),
        &used_packages,
    );
    collect(
        LibraryElementKind::Device,
        lib.devices().keys().copied().collect(),
        &used_devices,
    );
    collect(
        LibraryElementKind::Component,
        lib.components().keys().copied().collect(),
        &used_components,
    );
    unused
}

/// Removes all unused project library elements, see
/// [`unused_library_elements()`].
pub(crate) fn remove_unused_library_elements(
    tx: &mut Transaction<'_>,
) -> Result<Vec<LibraryElementId>> {
    let unused = unused_library_elements(tx.project());
    for id in &unused {
        tx.remove_library_element(id.kind, id.uuid)?;
    }
    Ok(unused)
}

/// Removes all unused elements from the project library (upstream
/// `CmdRemoveUnusedLibraryElements`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoveUnusedLibraryElements {}

impl Command for RemoveUnusedLibraryElements {
    /// The removed elements.
    type Output = Vec<LibraryElementId>;

    fn text(&self) -> String {
        librepcb_i18n::tr!(
            "librepcb::editor::CmdRemoveUnusedLibraryElements",
            "Remove unused library elements"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Self::Output> {
        remove_unused_library_elements(tx)
    }
}
