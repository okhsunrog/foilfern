//! Errors of the editor crate.
//!
//! Model level errors are the [`project::Error`]s of the mutations (with
//! the upstream messages); the variants below are the errors the upstream
//! editor commands and states throw themselves (translated with their
//! upstream Qt context) plus argument errors of the command parameters,
//! which have no upstream counterpart (upstream gets them from the UI).

use librepcb_core::project::{self, LibraryElementKind};
use librepcb_core::types::Uuid;
use librepcb_core::{fileio, library, serialization, types};
use librepcb_i18n::tr;

/// Result type of the editor crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// An error of an editor command or of the undo stack.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A mutation of the project model failed (boxed: large).
    #[error(transparent)]
    Project(Box<project::Error>),
    /// A library element could not be opened.
    #[error(transparent)]
    Library(#[from] library::Error),
    /// A file operation failed.
    #[error(transparent)]
    FileIo(#[from] fileio::Error),
    /// A value is invalid (e.g. a name).
    #[error(transparent)]
    Types(#[from] types::Error),
    /// A file could not be parsed.
    #[error(transparent)]
    Serialization(#[from] serialization::Error),
    /// A group is active, so no other group can be started and undo/redo
    /// are not possible.
    #[error("{}", tr!("librepcb::editor::UndoStack", "Another command is active at the moment. Please finish that command to continue."))]
    GroupActive,
    /// No group is active.
    #[error("{}", tr!("librepcb::editor::UndoStack", "No command group active!"))]
    NoGroupActive,
    /// The library element does not exist in the library element source
    /// (upstream: "workspace library").
    #[error("{}", not_in_source_message(*.kind, .uuid))]
    NotInLibrarySource {
        /// Kind of the element.
        kind: LibraryElementKind,
        /// UUID of the element.
        uuid: Uuid,
    },
    /// The library element does not exist in the project library.
    #[error("{}", not_in_project_library_message(*.kind, .uuid))]
    NotInProjectLibrary {
        /// Kind of the element.
        kind: LibraryElementKind,
        /// UUID of the element.
        uuid: Uuid,
    },
    /// The package has no footprints.
    #[error("{}", tr!("librepcb::editor::CmdAddDeviceToBoard", "Package does not have any footprints: {0}", .0))]
    NoFootprints(Uuid),
    /// The device is not a compatible device of the component and the
    /// component's assembly options are locked.
    #[error("{}", tr!("librepcb::editor::CmdAddDeviceToBoard", "The component in the schematic does not specify the chosen device as compatible and is locked for modifications from the board editor. Either add a corresponding assembly option to the component in the schematic, or remove the lock from the component."))]
    DeviceNotCompatible,
    /// The component has no symbols (gates) in its symbol variant.
    #[error("{}", tr!("librepcb::editor::SchematicEditorState_AddComponent", "The component with the UUID \"{0}\" does not have any symbol.", .0))]
    ComponentWithoutSymbols(Uuid),
    /// All gates of the component are placed already.
    #[error("All gates of the component \"{0}\" are placed already.")]
    AllGatesPlaced(String),
    /// A trace cannot be attached to a pad without net.
    #[error("{}", tr!("librepcb::editor::BoardEditorState_DrawTrace", "This pad is not connected to any net, therefore no trace can be attached to it. To allow attaching a trace, first connect this pad to a net in the schematics. So this is a problem of the schematics, not of the board."))]
    PadNotConnected,
    /// A trace cannot connect copper of two different nets.
    #[error("Cannot connect copper of the nets \"{0}\" and \"{1}\".")]
    NetMismatch(String, String),
    /// An invalid name (e.g. a directory name could not be derived).
    #[error("{}", tr!("librepcb::editor::ProjectEditor", "Invalid name: '{0}'", .0))]
    InvalidName(String),
    /// An entity referenced by the command parameters does not exist.
    #[error("{kind} not found: {id}")]
    NotFound {
        /// Kind of the entity (English noun).
        kind: &'static str,
        /// The identifier given in the parameters.
        id: String,
    },
    /// A name given in the parameters matches several entities.
    #[error("{kind} \"{id}\" is ambiguous.")]
    Ambiguous {
        /// Kind of the entity (English noun).
        kind: &'static str,
        /// The name given in the parameters.
        id: String,
    },
    /// Wire anchors are located in different schematics.
    #[error("The anchors are located in different schematic pages.")]
    DifferentSchematics,
    /// Another invalid argument.
    #[error("{0}")]
    InvalidArgument(String),
    /// The routing problem extracted from a board is invalid.
    #[error(transparent)]
    Autoroute(#[from] librepcb_autoroute::Error),
    /// A board export failed (e.g. the Specctra DSN export).
    #[error(transparent)]
    BoardExport(Box<librepcb_core::project::board::BoardExportError>),
}

impl From<project::Error> for Error {
    fn from(e: project::Error) -> Self {
        Self::Project(Box::new(e))
    }
}

impl Error {
    /// Creates a [`BoardExport`](Self::BoardExport) error.
    pub(crate) fn from_export(e: librepcb_core::project::board::BoardExportError) -> Self {
        Self::BoardExport(Box::new(e))
    }

    /// Creates a [`NotFound`](Self::NotFound) error.
    pub(crate) fn not_found(kind: &'static str, id: impl ToString) -> Self {
        Self::NotFound {
            kind,
            id: id.to_string(),
        }
    }
}

fn not_in_source_message(kind: LibraryElementKind, uuid: &Uuid) -> String {
    match kind {
        LibraryElementKind::Symbol => tr!(
            "librepcb::editor::CmdAddSymbolToSchematic",
            "The symbol with the UUID \"{0}\" does not exist in the workspace library!",
            uuid
        ),
        LibraryElementKind::Package => tr!(
            "librepcb::editor::CmdAddDeviceToBoard",
            "The package with the UUID \"{0}\" does not exist in the workspace library!",
            uuid
        ),
        LibraryElementKind::Component => tr!(
            "librepcb::editor::CmdAddComponentToCircuit",
            "The component with the UUID \"{0}\" does not exist in the workspace library!",
            uuid
        ),
        LibraryElementKind::Device => tr!(
            "librepcb::editor::CmdAddDeviceToBoard",
            "The device with the UUID \"{0}\" does not exist in the workspace library!",
            uuid
        ),
    }
}

fn not_in_project_library_message(kind: LibraryElementKind, uuid: &Uuid) -> String {
    match kind {
        LibraryElementKind::Component => tr!(
            "librepcb::editor::CmdComponentInstanceAdd",
            "The component with the UUID \"{0}\" does not exist in the project's library!",
            uuid
        ),
        LibraryElementKind::Symbol => tr!(
            "librepcb::SI_Symbol",
            "No symbol with the UUID \"{0}\" found in the project's library.",
            uuid
        ),
        LibraryElementKind::Package => {
            format!("The package with the UUID \"{uuid}\" does not exist in the project's library!")
        }
        LibraryElementKind::Device => {
            format!("The device with the UUID \"{uuid}\" does not exist in the project's library!")
        }
    }
}
