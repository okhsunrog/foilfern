//! UI independent editing layer of LibrePCB: the undo stack and the
//! high-level editor commands (port of the relevant parts of
//! `libs/librepcb/editor`: `undostack`, `undocommand`, `undocommandgroup`,
//! `project/cmd/*` and the editing logic of the schematic and board editor
//! states).
//!
//! This crate is the command bus shared by the MCP server and the (later)
//! Slint UI (see `docs/project-model-design.md` §4 and
//! `docs/mcp-research-konnect.md`):
//!
//! - [`ProjectEditor`] owns a [`Project`](librepcb_core::project::Project),
//!   its [`UndoStack`] and a [`LibraryElementSource`] (where library
//!   elements are copied from, e.g. the workspace library database).
//! - Every user or agent action is one labeled undo group. A group records
//!   the inverses of the applied [`Operation`]s: project
//!   [`Mutation`](librepcb_core::project::Mutation)s and the (file backed)
//!   project library additions and removals, so adding parts is undoable
//!   too.
//! - High-level [`commands`] (plain parameter structs implementing
//!   [`Command`]) validate their input, port the semantics of the upstream
//!   `Cmd*` classes and editor states, run as one undo group (rolled back
//!   completely on error) and return the UUIDs of the affected entities.
//!
//! [`OpenProject`] is a project opened from disk (locked directory, save
//! state) around its editor; the application and the embedded MCP server
//! share it as [`SharedProject`] (`Arc<parking_lot::Mutex<OpenProject>>`),
//! so both edit through the same undo stack.
//!
//! [`FreeroutingRouter`] routes boards with the external FreeRouting
//! autorouter (Specctra DSN export, session import).
//!
//! [`fsm`] holds the UI-toolkit independent editor state machines of the
//! schematic and board editors (tools, selection, live previews in undo
//! groups, clipboard).
//!
//! Differences to upstream: commands are data instead of `UndoCommand`
//! objects holding pointers; there are no Qt signals (the project's change
//! journal reports changes, the stack has a [`UndoStack::state_id()`]);
//! interactive steps (dialogs, cursor positions, snapping) are replaced by
//! explicit parameters.

pub mod commands;
mod editor;
mod error;
mod freerouting;
pub mod fsm;
pub mod library_editor;
mod library_source;
mod open_project;
mod undo_stack;

pub use editor::{Command, ProjectEditor, Transaction, create_project};
pub use error::{Error, Result};
pub use freerouting::{
    FREEROUTING_MIN_JAVA_VERSION, FreeroutingConfig, FreeroutingError, FreeroutingLauncher,
    FreeroutingOutput, FreeroutingReport, FreeroutingRouter, RoutingStats, dsn_for_freerouting,
    routing_stats,
};
pub use library_source::{DirectoryLibrarySource, LibraryElementSource, NoLibrarySource};
pub use open_project::{OpenProject, SharedProject, override_stale_locks};
pub use undo_stack::{HistoryEntry, LibraryElement, Operation, UndoStack};
