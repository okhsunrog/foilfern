//! Port of libs/librepcb/core/geometry/netline.{h,cpp}.

use std::cmp::Ordering;

use super::{object_list, property};
use crate::serialization::{self, DeserializeObject, HasUuid, List, SExpression, SerializeObject};
use crate::types::{UnsignedLength, Uuid};

/// An endpoint of a [`NetLine`].
///
/// The ordering is part of the file format (it defines which anchor is
/// serialized as `from`): pins < bus junctions < junctions, then by UUIDs.
///
/// Serde: externally tagged (`{"Junction": "<uuid>"}`,
/// `{"BusJunction": {"segment": .., "junction": ..}}`,
/// `{"Pin": {"symbol": .., "pin": ..}}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum NetLineAnchor {
    /// A net junction of the schematic.
    Junction(Uuid),
    /// A junction of a bus segment.
    BusJunction {
        /// Bus segment UUID.
        segment: Uuid,
        /// Junction UUID within the segment.
        junction: Uuid,
    },
    /// A pin of a symbol.
    Pin {
        /// Symbol UUID.
        symbol: Uuid,
        /// Pin UUID.
        pin: Uuid,
    },
}

impl NetLineAnchor {
    fn rank(&self) -> u8 {
        match self {
            Self::Pin { .. } => 0,
            Self::BusJunction { .. } => 1,
            Self::Junction(_) => 2,
        }
    }
}

impl Ord for NetLineAnchor {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Junction(a), Self::Junction(b)) => a.cmp(b),
            (
                Self::BusJunction {
                    segment: s1,
                    junction: j1,
                },
                Self::BusJunction {
                    segment: s2,
                    junction: j2,
                },
            ) => (s1, j1).cmp(&(s2, j2)),
            (
                Self::Pin {
                    symbol: s1,
                    pin: p1,
                },
                Self::Pin {
                    symbol: s2,
                    pin: p2,
                },
            ) => (s1, p1).cmp(&(s2, p2)),
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

impl PartialOrd for NetLineAnchor {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl SerializeObject for NetLineAnchor {
    fn serialize(&self, root: &mut List) {
        match self {
            Self::Junction(junction) => {
                root.append_child("junction", junction);
            }
            Self::BusJunction { segment, junction } => {
                root.append_child("bus", segment);
                root.append_child("junction", junction);
            }
            Self::Pin { symbol, pin } => {
                root.append_child("symbol", symbol);
                root.append_child("pin", pin);
            }
        }
    }
}

impl DeserializeObject for NetLineAnchor {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        if let Some(junction) = node.child("junction/@0") {
            let junction = serialization::FromSExpression::from_sexpression(junction)?;
            if let Some(bus) = node.child("bus/@0") {
                Ok(Self::BusJunction {
                    segment: serialization::FromSExpression::from_sexpression(bus)?,
                    junction,
                })
            } else {
                Ok(Self::Junction(junction))
            }
        } else {
            Ok(Self::Pin {
                symbol: node.child_value("symbol/@0")?,
                pin: node.child_value("pin/@0")?,
            })
        }
    }
}

/// A net line (wire) in a schematic.
///
/// The anchors are normalized: [`p1()`](Self::p1) is never greater than
/// [`p2()`](Self::p2).
///
/// Serde: `{"uuid", "width", "p1", "p2"}`; the anchors are normalized when
/// deserializing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(from = "NetLineData")]
pub struct NetLine {
    uuid: Uuid,
    width: UnsignedLength,
    p1: NetLineAnchor,
    p2: NetLineAnchor,
}

impl NetLine {
    /// Creates a net line (the anchors are normalized).
    pub fn new(uuid: Uuid, width: UnsignedLength, a: NetLineAnchor, b: NetLineAnchor) -> Self {
        let (p1, p2) = normalize_anchors(a, b);
        Self {
            uuid,
            width,
            p1,
            p2,
        }
    }

    property!(
        /// Returns the UUID.
        copy uuid: Uuid, set_uuid
    );
    property!(
        /// Returns the line width.
        copy width: UnsignedLength, set_width
    );

    /// Returns a copy with another UUID.
    pub fn with_uuid(&self, uuid: Uuid) -> Self {
        Self { uuid, ..*self }
    }

    /// Returns the first (smaller) anchor.
    pub fn p1(&self) -> NetLineAnchor {
        self.p1
    }

    /// Returns the second (greater) anchor.
    pub fn p2(&self) -> NetLineAnchor {
        self.p2
    }

    /// Sets the anchors (normalized), returns whether they were modified.
    pub fn set_anchors(&mut self, a: NetLineAnchor, b: NetLineAnchor) -> bool {
        let (a, b) = normalize_anchors(a, b);
        if (a == self.p1) && (b == self.p2) {
            return false;
        }
        self.p1 = a;
        self.p2 = b;
        // upstream: emits onEdited(AnchorsChanged)
        true
    }
}

/// Wire form of [`NetLine`] (deserialized, then normalized).
#[derive(serde::Deserialize)]
struct NetLineData {
    uuid: Uuid,
    width: UnsignedLength,
    p1: NetLineAnchor,
    p2: NetLineAnchor,
}

impl From<NetLineData> for NetLine {
    fn from(d: NetLineData) -> Self {
        Self::new(d.uuid, d.width, d.p1, d.p2)
    }
}

fn normalize_anchors(a: NetLineAnchor, b: NetLineAnchor) -> (NetLineAnchor, NetLineAnchor) {
    if b < a { (b, a) } else { (a, b) }
}

impl HasUuid for NetLine {
    fn uuid(&self) -> Uuid {
        self.uuid
    }
}

object_list!(
    /// List of [`NetLine`]s.
    NetLineList,
    NetLineListTag,
    NetLine,
    "line"
);

impl SerializeObject for NetLine {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("width", &self.width);
        root.ensure_line_break();
        self.p1.serialize(root.append_list("from"));
        root.ensure_line_break();
        self.p2.serialize(root.append_list("to"));
        root.ensure_line_break();
    }
}

impl DeserializeObject for NetLine {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self::new(
            node.child_value("@0")?,
            node.child_value("width/@0")?,
            NetLineAnchor::deserialize(node.required_child("from")?)?,
            NetLineAnchor::deserialize(node.required_child("to")?)?,
        ))
    }
}
