//! Port of libs/librepcb/core/geometry/trace.{h,cpp}.

use std::cmp::Ordering;

use super::{object_list, property};
use crate::serialization::{self, DeserializeObject, HasUuid, List, SExpression, SerializeObject};
use crate::types::{Layer, PositiveLength, Uuid};

/// An endpoint of a [`Trace`].
///
/// The ordering is part of the file format (it defines which anchor is
/// serialized as `from`): footprint pads < pads < vias < junctions, then by
/// UUIDs.
///
/// Serde: externally tagged, e.g. `{"Via": "<uuid>"}` or
/// `{"FootprintPad": {"device": "<uuid>", "pad": "<uuid>"}}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TraceAnchor {
    /// A junction of the board.
    Junction(Uuid),
    /// A via of the board.
    Via(Uuid),
    /// A standalone pad of the board.
    Pad(Uuid),
    /// A pad of a device's footprint.
    FootprintPad {
        /// Device UUID.
        device: Uuid,
        /// Footprint pad UUID.
        pad: Uuid,
    },
}

impl TraceAnchor {
    fn rank(&self) -> u8 {
        match self {
            Self::FootprintPad { .. } => 0,
            Self::Pad(_) => 1,
            Self::Via(_) => 2,
            Self::Junction(_) => 3,
        }
    }
}

impl Ord for TraceAnchor {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Junction(a), Self::Junction(b))
            | (Self::Via(a), Self::Via(b))
            | (Self::Pad(a), Self::Pad(b)) => a.cmp(b),
            (
                Self::FootprintPad {
                    device: d1,
                    pad: p1,
                },
                Self::FootprintPad {
                    device: d2,
                    pad: p2,
                },
            ) => (d1, p1).cmp(&(d2, p2)),
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

impl PartialOrd for TraceAnchor {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl SerializeObject for TraceAnchor {
    fn serialize(&self, root: &mut List) {
        match self {
            Self::Junction(junction) => {
                root.append_child("junction", junction);
            }
            Self::Via(via) => {
                root.append_child("via", via);
            }
            Self::Pad(pad) => {
                root.append_child("pad", pad);
            }
            Self::FootprintPad { device, pad } => {
                root.append_child("device", device);
                root.append_child("pad", pad);
            }
        }
    }
}

impl DeserializeObject for TraceAnchor {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        if let Some(junction) = node.child("junction") {
            Ok(Self::Junction(junction.child_value("@0")?))
        } else if let Some(via) = node.child("via") {
            Ok(Self::Via(via.child_value("@0")?))
        } else if let Some(device) = node.child("device") {
            Ok(Self::FootprintPad {
                device: device.child_value("@0")?,
                pad: node.child_value("pad/@0")?,
            })
        } else {
            Ok(Self::Pad(node.child_value("pad/@0")?))
        }
    }
}

/// A copper trace segment of a board.
///
/// The anchors are normalized: [`p1()`](Self::p1) is never greater than
/// [`p2()`](Self::p2).
///
/// Serde: `{"uuid", "layer", "width", "p1", "p2"}`; the anchors are
/// normalized when deserializing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(from = "TraceFields")]
pub struct Trace {
    uuid: Uuid,
    layer: Layer,
    width: PositiveLength,
    p1: TraceAnchor,
    p2: TraceAnchor,
}

/// Unnormalized serde representation of [`Trace`].
#[derive(serde::Deserialize)]
struct TraceFields {
    uuid: Uuid,
    layer: Layer,
    width: PositiveLength,
    p1: TraceAnchor,
    p2: TraceAnchor,
}

impl From<TraceFields> for Trace {
    fn from(f: TraceFields) -> Self {
        Self::new(f.uuid, f.layer, f.width, f.p1, f.p2)
    }
}

impl Trace {
    /// Creates a trace (the anchors are normalized).
    pub fn new(
        uuid: Uuid,
        layer: Layer,
        width: PositiveLength,
        a: TraceAnchor,
        b: TraceAnchor,
    ) -> Self {
        let (p1, p2) = normalize_anchors(a, b);
        Self {
            uuid,
            layer,
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
        /// Returns the copper layer.
        copy layer: Layer, set_layer
    );
    property!(
        /// Returns the trace width.
        copy width: PositiveLength, set_width
    );

    /// Returns a copy with another UUID.
    pub fn with_uuid(&self, uuid: Uuid) -> Self {
        Self { uuid, ..*self }
    }

    /// Returns the first (smaller) anchor.
    pub fn p1(&self) -> TraceAnchor {
        self.p1
    }

    /// Returns the second (greater) anchor.
    pub fn p2(&self) -> TraceAnchor {
        self.p2
    }

    /// Sets the anchors (normalized), returns whether they were modified.
    pub fn set_anchors(&mut self, a: TraceAnchor, b: TraceAnchor) -> bool {
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

fn normalize_anchors(a: TraceAnchor, b: TraceAnchor) -> (TraceAnchor, TraceAnchor) {
    if b < a { (b, a) } else { (a, b) }
}

impl HasUuid for Trace {
    fn uuid(&self) -> Uuid {
        self.uuid
    }
}

object_list!(
    /// List of [`Trace`]s.
    TraceList,
    TraceListTag,
    Trace,
    "trace"
);

impl SerializeObject for Trace {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("layer", &self.layer);
        root.append_child("width", &self.width);
        root.ensure_line_break();
        self.p1.serialize(root.append_list("from"));
        root.ensure_line_break();
        self.p2.serialize(root.append_list("to"));
        root.ensure_line_break();
    }
}

impl DeserializeObject for Trace {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self::new(
            node.child_value("@0")?,
            node.child_value("layer/@0")?,
            node.child_value("width/@0")?,
            TraceAnchor::deserialize(node.required_child("from")?)?,
            TraceAnchor::deserialize(node.required_child("to")?)?,
        ))
    }
}
