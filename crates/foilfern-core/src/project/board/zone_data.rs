//! Port of libs/librepcb/core/project/board/boardzonedata.{h,cpp} (and
//! `items/bi_zone.{h,cpp}`, whose state is this data).

use std::collections::BTreeSet;

use crate::geometry::{Path, ZoneRules, property};
use crate::serialization::{self, DeserializeObject, HasUuid, List, SExpression, SerializeObject};
use crate::types::{self, Layer, Uuid};

/// A keepout zone of a board.
///
/// Invariant: all layers are copper layers.
///
/// Serde: `{"uuid", "layers": [..], "rules": [..], "outline", "locked"}`;
/// the layers are checked when deserializing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "BoardZoneFields")]
pub struct BoardZoneData {
    uuid: Uuid,
    layers: BTreeSet<Layer>,
    rules: ZoneRules,
    outline: Path,
    locked: bool,
}

/// Unvalidated serde representation of [`BoardZoneData`].
#[derive(serde::Deserialize)]
struct BoardZoneFields {
    uuid: Uuid,
    layers: BTreeSet<Layer>,
    rules: ZoneRules,
    outline: Path,
    locked: bool,
}

impl TryFrom<BoardZoneFields> for BoardZoneData {
    type Error = types::Error;
    fn try_from(f: BoardZoneFields) -> Result<Self, types::Error> {
        Self::new(f.uuid, f.layers, f.rules, f.outline, f.locked)
    }
}

fn check_layers(layers: &BTreeSet<Layer>) -> Result<(), types::Error> {
    match layers.iter().find(|l| !l.is_copper()) {
        Some(layer) => Err(types::Error::InvalidZoneLayer(layer.id().to_owned())),
        None => Ok(()),
    }
}

impl BoardZoneData {
    /// Creates a zone, or returns an error if a layer is not a copper
    /// layer.
    pub fn new(
        uuid: Uuid,
        layers: BTreeSet<Layer>,
        rules: ZoneRules,
        outline: Path,
        locked: bool,
    ) -> Result<Self, types::Error> {
        check_layers(&layers)?;
        Ok(Self {
            uuid,
            layers,
            rules,
            outline,
            locked,
        })
    }

    /// Returns the UUID.
    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    /// Returns a copy with another UUID.
    pub fn with_uuid(&self, uuid: Uuid) -> Self {
        Self {
            uuid,
            ..self.clone()
        }
    }

    /// Returns the (copper) layers.
    pub fn layers(&self) -> &BTreeSet<Layer> {
        &self.layers
    }

    /// Sets the layers, returns whether they were modified, or an error if
    /// a layer is not a copper layer.
    pub fn set_layers(&mut self, layers: BTreeSet<Layer>) -> Result<bool, types::Error> {
        if layers == self.layers {
            return Ok(false);
        }
        check_layers(&layers)?;
        self.layers = layers;
        Ok(true)
    }

    property!(
        /// Returns the rules.
        copy rules: ZoneRules, set_rules
    );
    property!(
        /// Returns the outline.
        ref outline: Path, set_outline
    );
    property!(
        /// Returns whether the zone is locked.
        copy locked: bool, set_locked
    );
}

impl HasUuid for BoardZoneData {
    fn uuid(&self) -> Uuid {
        self.uuid
    }
}

impl SerializeObject for BoardZoneData {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        let rules = self.rules;
        root.append_child("no_copper", &rules.contains(ZoneRules::NO_COPPER));
        root.append_child("no_planes", &rules.contains(ZoneRules::NO_PLANES));
        root.append_child("no_exposure", &rules.contains(ZoneRules::NO_EXPOSURE));
        root.append_child("no_devices", &rules.contains(ZoneRules::NO_DEVICES));
        root.ensure_line_break();
        // The layer order is `Layer::sorted()`, i.e. the `Layer` ordering.
        for layer in &self.layers {
            root.append_child("layer", layer);
            root.ensure_line_break();
        }
        root.append_child("lock", &self.locked);
        root.ensure_line_break();
        self.outline.serialize(root);
        root.ensure_line_break();
    }
}

impl DeserializeObject for BoardZoneData {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        let layers = node
            .children_named("layer")
            .map(|child| child.child_value::<Layer>("@0"))
            .collect::<serialization::Result<BTreeSet<_>>>()?;
        let mut rules = ZoneRules::empty();
        rules.set(ZoneRules::NO_COPPER, node.child_value("no_copper/@0")?);
        rules.set(ZoneRules::NO_PLANES, node.child_value("no_planes/@0")?);
        rules.set(ZoneRules::NO_EXPOSURE, node.child_value("no_exposure/@0")?);
        rules.set(ZoneRules::NO_DEVICES, node.child_value("no_devices/@0")?);
        Ok(Self::new(
            node.child_value("@0")?,
            layers,
            rules,
            Path::deserialize(node)?,
            node.child_value("lock/@0")?,
        )?)
    }
}
