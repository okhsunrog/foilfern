//! High-level editor commands.
//!
//! Each command is a plain parameter struct (serde, for the MCP layer)
//! implementing [`Command`]: it validates its input, performs the
//! semantics of the corresponding upstream `Cmd*` class or editor state as
//! a sequence of model [`Mutation`]s and project library operations, and
//! returns the UUIDs of the affected entities. Run commands with
//! [`ProjectEditor::execute()`](crate::ProjectEditor::execute) (one undo
//! group each) or nested with [`Transaction::run()`].
//!
//! Entities are referenced by UUID or, where an agent naturally works with
//! names, by name ([`ComponentRef`], [`NetRef`], [`PinRef`], [`PadRef`]).
//!
//! Modules:
//! - [`library`]: project library (add elements with dependencies, remove
//!   unused ones).
//! - [`project`]: metadata, schematic pages and boards.
//! - [`circuit`]: nets and net classes.
//! - [`component`]: components, symbols and devices.
//! - [`schematic`]: wires, net labels, removal of schematic items.
//! - [`image`]: images of schematic pages (with their files).
//! - [`bus`]: buses (drawing bus lines, bus labels, combining segments
//!   and buses).
//! - [`autoroute`]: routing of air wires with the built-in autorouter.
//! - [`board`]: traces, vias, planes, outlines, other board items, board
//!   settings, removal of board items.
//! - [`wiring`]: net-level connecting and disconnecting of pins (agents).
//! - [`placement`]: automatic placement of symbols and devices (agents).
//! - [`schematic_tidy`]: re-layout of a schematic page by connectivity
//!   (agents).
//! - [`specctra`]: Specctra DSN export and session import (external
//!   autorouters).

pub mod autoroute;
pub mod board;
pub mod bus;
pub mod circuit;
pub mod component;
pub mod image;
pub mod library;
pub mod placement;
pub mod project;
mod resolve;
pub mod schematic;
pub mod schematic_tidy;
pub mod specctra;
pub mod wiring;

use librepcb_core::project::{ComponentInstanceId, Mutation, NetSignalId};

use crate::editor::{Command, Transaction};
use crate::error::Result;

pub use autoroute::*;
pub use board::*;
pub use bus::{AddBusLabel, BusAnchor, BusResult, DrawBus, RenameBusSegment};
pub use circuit::*;
pub use component::*;
pub use image::AddSchematicImage;
pub use library::*;
pub use placement::*;
pub use project::*;
pub use schematic::*;
pub use schematic_tidy::*;
pub use specctra::*;
pub use wiring::*;

/// Reference to a component instance: its UUID or its name (designator,
/// e.g. `"R1"`).
///
/// Serde: a string; strings which are valid UUIDs are UUIDs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ComponentRef {
    /// By UUID.
    Id(ComponentInstanceId),
    /// By name (designator).
    Name(String),
}

impl From<ComponentInstanceId> for ComponentRef {
    fn from(id: ComponentInstanceId) -> Self {
        Self::Id(id)
    }
}

impl From<&str> for ComponentRef {
    fn from(name: &str) -> Self {
        Self::Name(name.to_owned())
    }
}

/// Reference to a net signal: its UUID or its name.
///
/// Serde: a string; strings which are valid UUIDs are UUIDs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum NetRef {
    /// By UUID.
    Id(NetSignalId),
    /// By name.
    Name(String),
}

impl From<NetSignalId> for NetRef {
    fn from(id: NetSignalId) -> Self {
        Self::Id(id)
    }
}

impl From<&str> for NetRef {
    fn from(name: &str) -> Self {
        Self::Name(name.to_owned())
    }
}

/// Reference to a pin of a placed symbol of a component, e.g. `R1` pin
/// `"1"`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PinRef {
    /// The component.
    pub component: ComponentRef,
    /// The pin: UUID of the symbol pin, name of the symbol pin, or name of
    /// the component signal (tried in this order).
    pub pin: String,
}

impl PinRef {
    /// Creates a reference by component name and pin name.
    pub fn new(component: impl Into<String>, pin: impl Into<String>) -> Self {
        Self {
            component: ComponentRef::Name(component.into()),
            pin: pin.into(),
        }
    }
}

/// Reference to a footprint pad of a device on a board, e.g. `R1` pad
/// `"1"`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PadRef {
    /// The component of the device.
    pub component: ComponentRef,
    /// The pad: UUID of the footprint pad, name of the package pad, or name
    /// of the component signal (tried in this order).
    pub pad: String,
}

impl PadRef {
    /// Creates a reference by component name and pad name.
    pub fn new(component: impl Into<String>, pad: impl Into<String>) -> Self {
        Self {
            component: ComponentRef::Name(component.into()),
            pad: pad.into(),
        }
    }
}

/// Applies raw model mutations as one command (the `apply_mutation`
/// escape hatch of the MCP server; every mutation is validated by the
/// model).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ApplyMutations {
    /// Text of the undo group (default: "Apply mutations").
    #[serde(default)]
    pub text: Option<String>,
    /// The mutations, applied in order.
    pub mutations: Vec<Mutation>,
}

impl Command for ApplyMutations {
    type Output = ();

    fn text(&self) -> String {
        self.text
            .clone()
            .unwrap_or_else(|| "Apply mutations".to_owned())
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        tx.apply_all(self.mutations)
    }
}
