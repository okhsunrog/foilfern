//! Port of libs/librepcb/core/project/board/boardpaddata.{h,cpp}.
//!
//! Upstream `BoardPadData` derives from `Pad`; here it embeds a [`Pad`]
//! (like [`FootprintPad`](crate::library::pkg::FootprintPad)), accessible
//! with [`BoardPadData::pad()`] and [`BoardPadData::pad_mut()`].

use std::collections::{BTreeMap, BTreeSet};

use super::BoardDesignRules;
use crate::geometry::{ComponentSide, Pad, PadGeometry, PadHoleList, PadShape, property};
use crate::serialization::{self, DeserializeObject, HasUuid, List, SExpression, SerializeObject};
use crate::types::{Layer, Length, Uuid};

/// A standalone pad of a board net segment (not belonging to a device).
///
/// Serde: `{"pad": {...}, "locked": bool}`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BoardPadData {
    pad: Pad,
    locked: bool,
}

impl BoardPadData {
    /// Creates a board pad.
    pub fn new(pad: Pad, locked: bool) -> Self {
        Self { pad, locked }
    }

    /// Returns the UUID.
    pub fn uuid(&self) -> Uuid {
        self.pad.uuid()
    }

    /// Returns a copy with another UUID (upstream
    /// `BoardPadData(uuid, other)`).
    pub fn with_uuid(&self, uuid: Uuid) -> Self {
        Self {
            pad: self.pad.with_uuid(uuid),
            locked: self.locked,
        }
    }

    /// Returns the pad attributes (upstream base class `Pad`).
    pub fn pad(&self) -> &Pad {
        &self.pad
    }

    /// Returns the pad attributes for modification.
    // upstream: the setters emit onEdited(...Changed)
    pub fn pad_mut(&mut self) -> &mut Pad {
        &mut self.pad
    }

    property!(
        /// Returns whether the pad is locked.
        copy locked: bool, set_locked
    );

    /// Returns the geometries on the copper layers of the board and on the
    /// stop mask and solder paste layers (upstream `BI_Pad::getGeometries()`
    /// of a standalone pad); `connected_layers` are the layers of the
    /// traces connected to the pad (see
    /// [`Board::anchor_trace_layers()`](super::Board::anchor_trace_layers)).
    pub fn geometries(
        &self,
        copper_layers: &BTreeSet<Layer>,
        rules: &BoardDesignRules,
        connected_layers: &BTreeSet<Layer>,
    ) -> BTreeMap<Layer, Vec<PadGeometry>> {
        PadOnBoard {
            pad: &self.pad,
            mirrored: false,
        }
        .geometries(copper_layers, rules, connected_layers)
    }
}

/// The placement-dependent properties of a pad on a board, shared by
/// standalone pads and footprint pads (upstream `BI_Pad` caches).
#[derive(Debug, Clone, Copy)]
pub(crate) struct PadOnBoard<'a> {
    /// The pad properties (in pad coordinates).
    pub pad: &'a Pad,
    /// Whether the pad is mirrored (device on the bottom side).
    pub mirrored: bool,
}

impl PadOnBoard<'_> {
    /// Upstream `BI_Pad::getComponentSide()`.
    pub fn component_side(&self) -> ComponentSide {
        match (self.pad.component_side(), self.mirrored) {
            (side, false) => side,
            (ComponentSide::Top, true) => ComponentSide::Bottom,
            (ComponentSide::Bottom, true) => ComponentSide::Top,
        }
    }

    /// Upstream `BI_Pad::getSolderLayer()`.
    pub fn solder_layer(&self) -> Layer {
        let bottom = self.component_side() == ComponentSide::Bottom;
        if self.pad.is_tht() == bottom {
            Layer::TOP_COPPER
        } else {
            Layer::BOT_COPPER
        }
    }

    /// Upstream `BI_Pad::isOnLayer()`.
    pub fn is_on_layer(&self, layer: Layer) -> bool {
        if self.pad.is_tht() {
            layer.is_copper()
        } else {
            layer == self.solder_layer()
        }
    }

    /// Upstream `BI_Pad::getSizeForMaskOffsetCalculation()`.
    fn size_for_mask_offset_calculation(&self) -> Length {
        if self.pad.shape() == PadShape::Custom {
            Length::ZERO
        } else {
            *self.pad.width().min(self.pad.height())
        }
    }

    /// Upstream `BI_Pad::updateGeometries()`: the geometries on the copper
    /// layers of the board and on the stop mask and solder paste layers.
    /// `connected_layers` are the layers of the traces connected to the
    /// pad.
    pub fn geometries(
        &self,
        copper_layers: &BTreeSet<Layer>,
        rules: &BoardDesignRules,
        connected_layers: &BTreeSet<Layer>,
    ) -> BTreeMap<Layer, Vec<PadGeometry>> {
        copper_layers
            .iter()
            .copied()
            .chain([
                Layer::TOP_STOP_MASK,
                Layer::BOT_STOP_MASK,
                Layer::TOP_SOLDER_PASTE,
                Layer::BOT_SOLDER_PASTE,
            ])
            .map(|layer| {
                let geometry = self.geometry_on_layer(layer, rules, connected_layers);
                (layer, geometry)
            })
            .collect()
    }

    /// Upstream `BI_Pad::getGeometryOnLayer()`.
    pub fn geometry_on_layer(
        &self,
        layer: Layer,
        rules: &BoardDesignRules,
        connected_layers: &BTreeSet<Layer>,
    ) -> Vec<PadGeometry> {
        if layer.is_copper() {
            return self.geometry_on_copper_layer(layer, rules, connected_layers);
        }
        let is_tht_solder_side = layer.is_top() == (self.component_side() == ComponentSide::Bottom);
        let mut offset = None;
        if layer.is_stop_mask() {
            let cfg = self.pad.stop_mask_config();
            let auto_annular_ring = rules.pad_cmp_side_auto_annular_ring();
            if let Some(manual) = cfg.offset()
                && (!self.pad.is_tht() || is_tht_solder_side || !auto_annular_ring)
            {
                offset = Some(manual);
            } else if cfg.is_enabled() {
                offset = Some(
                    *rules
                        .stop_mask_clearance()
                        .calc_value(self.size_for_mask_offset_calculation()),
                );
            }
        } else if layer.is_solder_paste() {
            let cfg = self.pad.solder_paste_config();
            if cfg.is_enabled() && (!self.pad.is_tht() || is_tht_solder_side) {
                offset = Some(match cfg.offset() {
                    Some(manual) => -manual,
                    None => -*rules
                        .solder_paste_clearance()
                        .calc_value(self.size_for_mask_offset_calculation()),
                });
            }
        }
        let Some(offset) = offset else {
            return Vec::new();
        };
        let copper = if layer.is_top() {
            Layer::TOP_COPPER
        } else {
            Layer::BOT_COPPER
        };
        self.geometry_on_copper_layer(copper, rules, connected_layers)
            .iter()
            .map(|g| g.without_holes().with_offset(offset))
            .collect()
    }

    /// Upstream `BI_Pad::getGeometryOnCopperLayer()`.
    fn geometry_on_copper_layer(
        &self,
        layer: Layer,
        rules: &BoardDesignRules,
        connected_layers: &BTreeSet<Layer>,
    ) -> Vec<PadGeometry> {
        let (component_side_layer, solder_side_layer) = match self.component_side() {
            ComponentSide::Top => (Layer::TOP_COPPER, Layer::BOT_COPPER),
            ComponentSide::Bottom => (Layer::BOT_COPPER, Layer::TOP_COPPER),
        };
        let mut full_shape = false;
        let mut auto_annular = false;
        let mut minimal_annular = false;
        if self.pad.is_tht() {
            let full_component_side = !rules.pad_cmp_side_auto_annular_ring();
            let full_inner = !rules.pad_inner_auto_annular_ring();
            if (layer == solder_side_layer)
                || (full_component_side && (layer == component_side_layer))
                || (full_inner && layer.is_inner())
            {
                full_shape = true;
            } else if connected_layers.contains(&layer) {
                auto_annular = true;
            } else {
                minimal_annular = true;
            }
        } else if layer == component_side_layer {
            full_shape = true;
        }
        if full_shape {
            vec![self.pad.geometry()]
        } else if auto_annular || minimal_annular {
            self.pad
                .holes()
                .iter()
                .map(|hole| {
                    let annular_width = if auto_annular {
                        rules.pad_annular_ring().calc_value(*hole.diameter())
                    } else {
                        rules.pad_annular_ring().min_value()
                    };
                    let diameter = hole.diameter() + annular_width + annular_width;
                    PadGeometry::stroke(
                        diameter,
                        hole.path().clone(),
                        PadHoleList::from(vec![hole.clone()]),
                    )
                })
                .collect()
        } else {
            Vec::new()
        }
    }
}

impl HasUuid for BoardPadData {
    fn uuid(&self) -> Uuid {
        self.pad.uuid()
    }
}

impl SerializeObject for BoardPadData {
    fn serialize(&self, root: &mut List) {
        self.pad.serialize_with(root, |root| {
            root.append_child("lock", &self.locked);
        });
    }
}

impl DeserializeObject for BoardPadData {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            pad: Pad::deserialize(node)?,
            locked: node.child_value("lock/@0")?,
        })
    }
}
