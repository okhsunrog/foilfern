//! Port of libs/librepcb/core/project/board/items/bi_plane.{h,cpp}.
//!
//! Differences to upstream: only the persistent state is stored (plus the
//! visibility, which upstream keeps in the board's `settings.user.lp`); the
//! calculated fragments are derived data of the board
//! ([`BoardDerived::plane_fragments()`](super::BoardDerived::plane_fragments)).

use std::fmt;
use std::str::FromStr;

use crate::geometry::{Path, property};
use crate::project::id::{NetSignalId, PlaneId};
use crate::serialization::{
    self, DeserializeObject, FromSExpression, List, SExpression, SerializeObject, ToSExpression,
};
use crate::types::{self, Layer, Length, PositiveLength, UnsignedLength, Uuid};

/// How pads are connected to a plane (upstream `BI_Plane::ConnectStyle`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PlaneConnectStyle {
    /// Pads are not connected to the plane.
    None,
    /// Pads are connected with thermal spokes.
    #[default]
    ThermalRelief,
    /// Pads are connected completely.
    Solid,
}

impl PlaneConnectStyle {
    /// Returns the file format token.
    pub fn to_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ThermalRelief => "thermal",
            Self::Solid => "solid",
        }
    }
}

impl fmt::Display for PlaneConnectStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.to_str())
    }
}

impl FromStr for PlaneConnectStyle {
    type Err = types::Error;
    fn from_str(s: &str) -> Result<Self, types::Error> {
        [Self::None, Self::ThermalRelief, Self::Solid]
            .into_iter()
            .find(|v| v.to_str() == s)
            .ok_or_else(|| types::Error::UnknownPlaneConnectStyle(s.to_owned()))
    }
}

// Serde: the file format token.
crate::utils::serde_string::serde_string!(PlaneConnectStyle);

impl ToSExpression for PlaneConnectStyle {
    fn to_sexpression(&self) -> SExpression {
        SExpression::token(self.to_str())
    }
}

impl FromSExpression for PlaneConnectStyle {
    fn from_sexpression(node: &SExpression) -> serialization::Result<Self> {
        Ok(node.value()?.parse()?)
    }
}

/// A copper plane of a board.
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BoardPlane {
    uuid: Uuid,
    layer: Layer,
    net: Option<NetSignalId>,
    outline: Path,
    min_width: UnsignedLength,
    min_clearance_to_copper: UnsignedLength,
    min_clearance_to_board: UnsignedLength,
    min_clearance_to_npth: UnsignedLength,
    keep_islands: bool,
    priority: i32,
    connect_style: PlaneConnectStyle,
    thermal_gap: PositiveLength,
    thermal_spoke_width: PositiveLength,
    locked: bool,
    /// Stored in `settings.user.lp`, not in `board.lp`.
    visible: bool,
}

fn unsigned(nm: i64) -> UnsignedLength {
    UnsignedLength::new(Length::new(nm)).expect("constant is not negative")
}

fn positive(nm: i64) -> PositiveLength {
    PositiveLength::new(Length::new(nm)).expect("constant is positive")
}

impl BoardPlane {
    /// Creates a plane with the upstream default settings.
    pub fn new(uuid: Uuid, layer: Layer, net: Option<NetSignalId>, outline: Path) -> Self {
        Self {
            uuid,
            layer,
            net,
            outline,
            min_width: unsigned(200_000),               // 200um
            min_clearance_to_copper: unsigned(250_000), // 250um
            min_clearance_to_board: unsigned(300_000),  // 300um
            min_clearance_to_npth: unsigned(300_000),   // 300um
            keep_islands: false,
            priority: 0,
            connect_style: PlaneConnectStyle::ThermalRelief,
            thermal_gap: positive(300_000),         // 300um
            thermal_spoke_width: positive(300_000), // 300um
            locked: false,
            visible: true,
        }
    }

    /// Returns the UUID.
    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    /// Returns the typed identifier.
    pub fn id(&self) -> PlaneId {
        PlaneId(self.uuid)
    }

    /// Returns a copy with another UUID.
    pub fn with_uuid(&self, uuid: Uuid) -> Self {
        Self {
            uuid,
            ..self.clone()
        }
    }

    property!(
        /// Returns the (copper) layer.
        copy layer: Layer, set_layer
    );
    property!(
        /// Returns the net signal (`None` = no net).
        copy net: Option<NetSignalId>, set_net
    );
    property!(
        /// Returns the outline.
        ref outline: Path, set_outline
    );
    property!(
        /// Returns the minimum copper width.
        copy min_width: UnsignedLength, set_min_width
    );
    property!(
        /// Returns the minimum clearance to other copper.
        copy min_clearance_to_copper: UnsignedLength, set_min_clearance_to_copper
    );
    property!(
        /// Returns the minimum clearance to the board outline.
        copy min_clearance_to_board: UnsignedLength, set_min_clearance_to_board
    );
    property!(
        /// Returns the minimum clearance to non-plated holes.
        copy min_clearance_to_npth: UnsignedLength, set_min_clearance_to_npth
    );
    property!(
        /// Returns whether unconnected islands are kept.
        copy keep_islands: bool, set_keep_islands
    );
    property!(
        /// Returns the priority (higher is filled first).
        copy priority: i32, set_priority
    );
    property!(
        /// Returns how pads are connected.
        copy connect_style: PlaneConnectStyle, set_connect_style
    );
    property!(
        /// Returns the thermal relief gap.
        copy thermal_gap: PositiveLength, set_thermal_gap
    );
    property!(
        /// Returns the thermal spoke width.
        copy thermal_spoke_width: PositiveLength, set_thermal_spoke_width
    );
    property!(
        /// Returns whether the plane is locked.
        copy locked: bool, set_locked
    );
    property!(
        /// Returns whether the plane is visible (user setting).
        copy visible: bool, set_visible
    );
}

impl SerializeObject for BoardPlane {
    /// Writes the `board.lp` content (without the visibility).
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("layer", &self.layer);
        root.ensure_line_break();
        root.append_child("net", &self.net);
        root.append_child("priority", &self.priority);
        root.append_child("min_width", &self.min_width);
        root.ensure_line_break();
        root.append_child("min_copper_clearance", &self.min_clearance_to_copper);
        root.append_child("min_board_clearance", &self.min_clearance_to_board);
        root.append_child("min_npth_clearance", &self.min_clearance_to_npth);
        root.ensure_line_break();
        root.append_child("connect_style", &self.connect_style);
        root.append_child("thermal_gap", &self.thermal_gap);
        root.append_child("thermal_spoke", &self.thermal_spoke_width);
        root.ensure_line_break();
        root.append_child("keep_islands", &self.keep_islands);
        root.append_child("lock", &self.locked);
        root.ensure_line_break();
        self.outline.serialize(root);
        root.ensure_line_break();
    }
}

impl DeserializeObject for BoardPlane {
    /// Reads the `board.lp` content (the plane is visible).
    ///
    /// Upstream `ProjectLoader::loadBoardPlane()` does not read the `lock`
    /// attribute (it is always written, but loaded as unlocked); this port
    /// reads it, so a locked plane stays locked (see COMPAT.md).
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            layer: node.child_value("layer/@0")?,
            net: node.child_value("net/@0")?,
            outline: Path::deserialize(node)?,
            min_width: node.child_value("min_width/@0")?,
            min_clearance_to_copper: node.child_value("min_copper_clearance/@0")?,
            min_clearance_to_board: node.child_value("min_board_clearance/@0")?,
            min_clearance_to_npth: node.child_value("min_npth_clearance/@0")?,
            keep_islands: node.child_value("keep_islands/@0")?,
            priority: node.child_value("priority/@0")?,
            connect_style: node.child_value("connect_style/@0")?,
            thermal_gap: node.child_value("thermal_gap/@0")?,
            thermal_spoke_width: node.child_value("thermal_spoke/@0")?,
            locked: node.child_value("lock/@0")?,
            visible: true,
        })
    }
}
