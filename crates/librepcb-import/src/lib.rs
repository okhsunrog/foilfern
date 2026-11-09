//! Import of EAGLE and KiCad libraries (and EAGLE projects) into LibrePCB,
//! port of libs/librepcb/eagleimport, libs/librepcb/kicadimport and
//! libs/parseagle.
//!
//! The import is UI independent: each import type holds the parsed source
//! elements with their selection state ([`CheckState`], dependencies are
//! selected automatically), the options of the upstream import wizards
//! (name prefix, categories) and runs the conversion as a blocking call
//! reporting [`Progress`] (with cancellation) and messages
//! ([`MessageLogger`](librepcb_core::utils::message_logger::MessageLogger)).
//!
//! - [`eagle`]: EAGLE `*.lbr` library import.
//! - [`kicad`]: KiCad `*.kicad_sym`/`*.pretty` library import.
//!
//! Created elements get random UUIDs by default; [`UuidGenerator`] allows
//! reproducible output.

pub mod eagle;
mod html;
pub mod kicad;
mod library_writer;
mod progress;
mod uuid_generator;

pub use progress::{CheckState, Progress, ProgressEvent};
pub use uuid_generator::UuidGenerator;

/// Kind of a library element to import.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum ElementKind {
    /// Symbol.
    Symbol,
    /// Package (EAGLE package, KiCad footprint).
    Package,
    /// Component (EAGLE device set, KiCad symbol).
    Component,
    /// Device.
    Device,
}

/// A change of the selection state of an element caused by selecting or
/// deselecting another element (upstream `*CheckStateChanged()` signals).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CheckStateChange {
    /// Element kind.
    pub kind: ElementKind,
    /// Name of the source library (KiCad library name, empty for EAGLE).
    pub library: String,
    /// Element name.
    pub name: String,
    /// The new state.
    pub state: CheckState,
}

impl CheckStateChange {
    fn new(kind: ElementKind, library: &str, name: &str, state: CheckState) -> Self {
        Self {
            kind,
            library: library.to_owned(),
            name: name.to_owned(),
            state,
        }
    }
}

/// Result of an import run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct ImportSummary {
    /// Number of elements selected for import.
    pub total: usize,
    /// Number of elements processed (imported or failed).
    pub processed: usize,
    /// Number of elements successfully imported.
    pub imported: usize,
    /// Whether the import was canceled.
    pub canceled: bool,
}
