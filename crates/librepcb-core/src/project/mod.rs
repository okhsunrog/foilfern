//! Port of libs/librepcb/core/project (projects: metadata, settings,
//! project library, circuit, schematics, boards, loading and saving).
//!
//! Design: `docs/project-model-design.md`. In short:
//!
//! - [`Project`] is a plain data tree; UUID keyed collections are
//!   `BTreeMap`s ordered like the saved files, references are typed
//!   [identifiers](id) wrapping the file format UUIDs, there are no
//!   back-pointers (context is passed explicitly).
//! - All modifications go through [`Project::apply()`] with a [`Mutation`]
//!   value, which validates (check-then-commit), journals [`Change`]s and
//!   returns the inverse mutation. Typed wrappers exist for the common
//!   operations. Project library elements are file backed and added or
//!   removed with dedicated methods instead.
//! - Reverse relations (which segments belong to a net, which symbols to a
//!   component, ...) live in a private reverse index maintained by the
//!   mutations and rebuilt after loading.
//! - Change notification is a pull based journal
//!   ([`Project::changes_since()`]) with a revision counter.
//! - Loading ([`ProjectLoader`]) and saving ([`Project::save()`]) write the
//!   same files in the same order as upstream (byte-identical).
//!
//! Module layout (file names describe the Rust types, the upstream file is
//! named in each module doc):
//!
//! - [`circuit`]: assembly variants, net classes, nets, buses, component
//!   instances.
//! - [`schematic`]: schematic pages and their items.
//! - [`board`]: boards, their items and derived data.
//! - [`loader`]: the project loader.
//! - `mutation`, `change`, `ref_index`, `library`, `project`, `id`,
//!   `error`: see the re-exports below.
//!
//! Not ported yet: `board/drc/`, the
//! export glue (`boardgerberexport`, ...),
//! `outputjobrunner`.

mod attribute_lookup;
pub mod board;
mod bom_generator;
mod change;
pub mod circuit;
pub mod erc;
mod error;
mod id;
mod json_export;
mod library;
pub mod loader;
mod mutation;
#[allow(clippy::module_inception)] // Upstream file name.
mod project;
mod ref_index;
pub mod schematic;

pub use attribute_lookup::ProjectAttributeLookup;
pub use board::BoardChange;
pub use bom_generator::BomGenerator;
pub use change::{Change, ChangeLog, ChangesSince, DEFAULT_CHANGE_LOG_CAPACITY};
pub use error::{EntityKind, Error, Result};
pub use id::{
    AssemblyVariantId, BoardId, BoardNetSegmentRef, BusId, BusSegmentId, BusSegmentRef,
    ComponentInstanceId, ComponentSignalRef, DeviceRef, NetClassId, NetSegmentId, NetSegmentRef,
    NetSignalId, PlaneId, PlaneRef, SchematicId, SymbolId, SymbolRef,
};
pub use json_export::ProjectJsonExport;
pub use library::{LibraryElementKind, ProjectLibrary};
pub use loader::ProjectLoader;
pub use mutation::{BoardMutation, Mutation, SchematicMutation};
pub use project::{Project, ProjectMetadata, ProjectSettings, ProjectView};
pub use ref_index::{ComponentUses, NetUse, Reference};
pub use schematic::SchematicChange;
