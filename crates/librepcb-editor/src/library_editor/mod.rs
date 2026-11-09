//! UI independent editing of library elements (symbols, packages,
//! components, devices): port of the non-graphical parts of
//! libs/librepcb/editor/library (the element tabs, `LibraryEditorTab`,
//! `LibraryElementCache`, `DevicePinoutBuilder`, the component variant and
//! gate editors) and of the library element commands
//! (libs/librepcb/editor/library/cmd/*).
//!
//! - [`LibraryElementEditor`] owns one element and its undo stack
//!   ([`ElementUndoStack`], snapshot based), tracks the dirty state and
//!   saves the element (byte-identical files through the core's element
//!   serialization). [`SymbolEditor`], [`PackageEditor`],
//!   [`ComponentEditor`] and [`DeviceEditor`] are its instances.
//! - [`commands`] are the edit operations (plain parameter structs
//!   implementing [`ElementCommand`]) used by the editor FSMs
//!   ([`crate::fsm::library`]), the future Slint tabs and the MCP server.
//! - [`LibraryElementCache`] loads (and caches) the elements an element
//!   refers to (e.g. the symbols of a component) from a
//!   [`LibraryElementSource`](crate::LibraryElementSource).
//! - [`checks`] maps the library check messages to their automatic fixes.
//!
//! Differences to upstream: see the module docs; most notably, the undo
//! stack stores content snapshots instead of command objects.

pub mod cache;
pub mod checks;
pub mod commands;
mod editor;
mod element;
mod undo;

pub use cache::LibraryElementCache;
pub use editor::{ElementCommand, GridElement, LibraryElementEditor};
pub use element::{
    ComponentContent, DeviceContent, EditableElement, PackageContent, SymbolContent,
};
pub use undo::ElementUndoStack;

use librepcb_core::library::cmp::Component;
use librepcb_core::library::dev::Device;
use librepcb_core::library::pkg::Package;
use librepcb_core::library::sym::Symbol;

/// The editor of a symbol.
pub type SymbolEditor = LibraryElementEditor<Symbol>;
/// The editor of a package.
pub type PackageEditor = LibraryElementEditor<Package>;
/// The editor of a component.
pub type ComponentEditor = LibraryElementEditor<Component>;
/// The editor of a device.
pub type DeviceEditor = LibraryElementEditor<Device>;
