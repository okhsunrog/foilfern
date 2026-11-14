//! Port of libs/librepcb/eagleimport (and libs/parseagle as [`model`]):
//! import of EAGLE libraries and projects.
//!
//! - [`model`]: the EAGLE XML file format (parseagle).
//! - [`EagleTypeConverter`]: conversion of EAGLE types into LibrePCB types.
//! - [`EagleLibraryConverter`]: conversion of EAGLE symbols, packages,
//!   device sets and devices into LibrePCB library elements.
//! - [`EagleLibraryImport`]: the library import (element selection with
//!   dependencies, options, running the import into a library directory).
//! - [`EagleProjectImport`]: import of a schematic and board into a
//!   project.

mod error;
mod library_converter;
mod library_import;
pub mod model;
mod project_import;
mod type_converter;

pub use error::{Error, Result};
pub use library_converter::{EagleLibraryConverter, EagleLibraryConverterSettings};
pub use library_import::{
    CATEGORY_NAME, EagleLibraryImport, ImportComponent, ImportDevice, ImportPackage, ImportSymbol,
    NAME_PREFIX,
};
pub use project_import::EagleProjectImport;
pub use type_converter::{ConvertedPin, EagleTypeConverter, Geometry};
