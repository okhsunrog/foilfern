//! Typed identifiers of project entities (no upstream counterpart: upstream
//! references entities by raw pointers).
//!
//! Every identifier wraps the [`Uuid`] the file format already uses, so
//! references are stable across load/save/undo and meaningful outside the
//! process (MCP, CLI). Serde: serialized like [`Uuid`] (string). Nested
//! items are addressed by path structs ([`ComponentSignalRef`], ...).

use std::fmt;
use std::str::FromStr;

use crate::serialization::{self, FromSExpression, SExpression, ToSExpression};
use crate::types::{self, Uuid};

/// Defines a typed identifier wrapping a [`Uuid`].
macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            serde::Serialize,
            serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Creates a new random identifier.
            pub fn new_random() -> Self {
                Self(Uuid::new_random())
            }

            /// Returns the UUID.
            pub fn uuid(self) -> Uuid {
                self.0
            }
        }

        impl From<Uuid> for $name {
            fn from(uuid: Uuid) -> Self {
                Self(uuid)
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Uuid {
                id.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl FromStr for $name {
            type Err = types::Error;
            fn from_str(s: &str) -> Result<Self, types::Error> {
                s.parse().map(Self)
            }
        }

        impl ToSExpression for $name {
            fn to_sexpression(&self) -> SExpression {
                self.0.to_sexpression()
            }
        }

        impl FromSExpression for $name {
            fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
                Uuid::from_sexpression(node).map(Self)
            }
        }

        impl ToSExpression for Option<$name> {
            fn to_sexpression(&self) -> SExpression {
                self.map(|id| id.0).to_sexpression()
            }
        }

        impl FromSExpression for Option<$name> {
            fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
                Ok(Option::<Uuid>::from_sexpression(node)?.map($name))
            }
        }
    };
}

id_type!(
    /// Identifier of an assembly variant of the circuit.
    AssemblyVariantId
);
id_type!(
    /// Identifier of a net class of the circuit.
    NetClassId
);
id_type!(
    /// Identifier of a net signal of the circuit.
    NetSignalId
);
id_type!(
    /// Identifier of a bus of the circuit.
    BusId
);
id_type!(
    /// Identifier of a component instance of the circuit.
    ComponentInstanceId
);
id_type!(
    /// Identifier of a schematic page.
    SchematicId
);
id_type!(
    /// Identifier of a board.
    BoardId
);
id_type!(
    /// Identifier of a symbol placed in a schematic.
    SymbolId
);
id_type!(
    /// Identifier of a net segment of a schematic or board.
    NetSegmentId
);
id_type!(
    /// Identifier of a bus segment of a schematic.
    BusSegmentId
);
id_type!(
    /// Identifier of a plane of a board.
    PlaneId
);

/// Reference to a signal instance of a component instance (upstream
/// `ComponentSignalInstance`, keyed by the library signal UUID).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ComponentSignalRef {
    /// The component instance.
    pub component: ComponentInstanceId,
    /// UUID of the signal in the library component.
    pub signal: Uuid,
}

/// Reference to a symbol of a schematic.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct SymbolRef {
    /// The schematic.
    pub schematic: SchematicId,
    /// The symbol.
    pub symbol: SymbolId,
}

/// Reference to a net segment of a schematic.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct NetSegmentRef {
    /// The schematic.
    pub schematic: SchematicId,
    /// The net segment.
    pub segment: NetSegmentId,
}

/// Reference to a bus segment of a schematic.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct BusSegmentRef {
    /// The schematic.
    pub schematic: SchematicId,
    /// The bus segment.
    pub segment: BusSegmentId,
}

/// Reference to a net segment of a board.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct BoardNetSegmentRef {
    /// The board.
    pub board: BoardId,
    /// The net segment.
    pub segment: NetSegmentId,
}

/// Reference to a plane of a board.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct PlaneRef {
    /// The board.
    pub board: BoardId,
    /// The plane.
    pub plane: PlaneId,
}

/// Reference to a device of a board (a board holds at most one device per
/// component instance, upstream `BI_Device` keyed by the component UUID).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct DeviceRef {
    /// The board.
    pub board: BoardId,
    /// The component instance the device belongs to.
    pub component: ComponentInstanceId,
}
