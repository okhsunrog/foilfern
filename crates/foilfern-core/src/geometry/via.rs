//! Port of libs/librepcb/core/geometry/via.{h,cpp}.

use super::{Error, Path, object_list, property};
use crate::serialization::{
    self, DeserializeObject, FromSExpression, HasUuid, List, SExpression, SerializeObject,
    ToSExpression,
};
use crate::types::{
    BoundedUnsignedRatio, Layer, Length, MaskConfig, Point, PositiveLength, UnsignedLength, Uuid,
};

/// A via of a board.
///
/// Invariants: the start layer is a copper layer above the (copper) end
/// layer; if the drill is automatic (`None`), the size is automatic too; a
/// manual size is not smaller than a manual drill.
///
/// Serde: an object with the fields below (`None` = automatic); the
/// invariants are checked when deserializing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "ViaFields")]
pub struct Via {
    uuid: Uuid,
    start_layer: Layer,
    end_layer: Layer,
    position: Point,
    drill_diameter: Option<PositiveLength>,
    size: Option<PositiveLength>,
    exposure_config: MaskConfig,
}

/// Unvalidated serde representation of [`Via`].
#[derive(serde::Deserialize)]
struct ViaFields {
    uuid: Uuid,
    start_layer: Layer,
    end_layer: Layer,
    position: Point,
    drill_diameter: Option<PositiveLength>,
    size: Option<PositiveLength>,
    exposure_config: MaskConfig,
}

impl TryFrom<ViaFields> for Via {
    type Error = Error;
    fn try_from(f: ViaFields) -> Result<Self, Error> {
        Self::new(
            f.uuid,
            f.start_layer,
            f.end_layer,
            f.position,
            f.drill_diameter,
            f.size,
            f.exposure_config,
        )
    }
}

impl Via {
    /// Creates a via, or returns an error if the invariants are violated.
    pub fn new(
        uuid: Uuid,
        start_layer: Layer,
        end_layer: Layer,
        position: Point,
        drill_diameter: Option<PositiveLength>,
        size: Option<PositiveLength>,
        exposure_config: MaskConfig,
    ) -> Result<Self, Error> {
        check_layers(start_layer, end_layer)?;
        check_drill_and_size(drill_diameter, size)?;
        Ok(Self {
            uuid,
            start_layer,
            end_layer,
            position,
            drill_diameter,
            size,
            exposure_config,
        })
    }

    property!(
        /// Returns the UUID.
        copy uuid: Uuid, set_uuid
    );
    property!(
        /// Returns the position.
        copy position: Point, set_position
    );
    property!(
        /// Returns the stop mask exposure configuration.
        copy exposure_config: MaskConfig, set_exposure_config
    );

    /// Returns a copy with another UUID.
    pub fn with_uuid(&self, uuid: Uuid) -> Self {
        Self { uuid, ..*self }
    }

    /// Returns the start (top-most) copper layer.
    pub fn start_layer(&self) -> Layer {
        self.start_layer
    }

    /// Returns the end (bottom-most) copper layer.
    pub fn end_layer(&self) -> Layer {
        self.end_layer
    }

    /// Returns the drill diameter (`None` = automatic).
    pub fn drill_diameter(&self) -> Option<PositiveLength> {
        self.drill_diameter
    }

    /// Returns the size (`None` = automatic).
    pub fn size(&self) -> Option<PositiveLength> {
        self.size
    }

    /// Returns whether the via goes from top to bottom.
    pub fn is_through(&self) -> bool {
        (self.start_layer == Layer::TOP_COPPER) && (self.end_layer == Layer::BOT_COPPER)
    }

    /// Returns whether the via connects an outer layer with an inner layer.
    pub fn is_blind(&self) -> bool {
        (self.start_layer == Layer::TOP_COPPER) != (self.end_layer == Layer::BOT_COPPER)
    }

    /// Returns whether the via connects only inner layers.
    pub fn is_buried(&self) -> bool {
        (self.start_layer != Layer::TOP_COPPER) && (self.end_layer != Layer::BOT_COPPER)
    }

    /// Returns whether the via has copper on the given layer.
    pub fn is_on_layer(&self, layer: Layer) -> bool {
        layer.is_copper() && Self::is_on_layer_between(layer, self.start_layer, self.end_layer)
    }

    /// Returns whether the via has copper on any of the given layers.
    pub fn is_on_any_layer(&self, layers: impl IntoIterator<Item = Layer>) -> bool {
        Self::is_on_any_layer_between(layers, self.start_layer, self.end_layer)
    }

    /// Sets the layers, returns whether they were modified.
    pub fn set_layers(&mut self, from: Layer, to: Layer) -> Result<bool, Error> {
        if (from == self.start_layer) && (to == self.end_layer) {
            return Ok(false);
        }
        check_layers(from, to)?;
        self.start_layer = from;
        self.end_layer = to;
        // upstream: emits onEdited(LayersChanged)
        Ok(true)
    }

    /// Sets drill and size, returns whether they were modified.
    pub fn set_drill_and_size(
        &mut self,
        drill: Option<PositiveLength>,
        size: Option<PositiveLength>,
    ) -> Result<bool, Error> {
        if (drill == self.drill_diameter) && (size == self.size) {
            return Ok(false);
        }
        check_drill_and_size(drill, size)?;
        self.drill_diameter = drill;
        self.size = size;
        // upstream: emits onEdited(DrillOrSizeChanged)
        Ok(true)
    }

    /// Returns the via size resulting from a drill and the annular ring
    /// design rule.
    pub fn calc_size_from_rules(
        drill: PositiveLength,
        ratio: &BoundedUnsignedRatio,
    ) -> PositiveLength {
        let annular = UnsignedLength::new(*ratio.calc_value(*drill) * 2).unwrap_or_default();
        drill + annular
    }

    /// Returns the outline (a circle around the origin) of a via with the
    /// given size and expansion, or an empty path if the result is empty.
    pub fn outline(size: PositiveLength, expansion: Length) -> Path {
        PositiveLength::new(size + (expansion * 2)).map_or_else(|_| Path::default(), Path::circle)
    }

    /// Returns whether a via from `from` to `to` has copper on `layer`.
    pub fn is_on_layer_between(layer: Layer, from: Layer, to: Layer) -> bool {
        let nbr = layer.copper_number();
        (nbr >= from.copper_number()) && (nbr <= to.copper_number())
    }

    /// Returns whether a via from `from` to `to` has copper on any of
    /// `layers`.
    pub fn is_on_any_layer_between(
        layers: impl IntoIterator<Item = Layer>,
        from: Layer,
        to: Layer,
    ) -> bool {
        layers
            .into_iter()
            .any(|layer| Self::is_on_layer_between(layer, from, to))
    }
}

fn check_layers(from: Layer, to: Layer) -> Result<(), Error> {
    if !from.is_copper() || !to.is_copper() || (from.copper_number() >= to.copper_number()) {
        return Err(Error::InvalidViaLayers);
    }
    Ok(())
}

fn check_drill_and_size(
    drill: Option<PositiveLength>,
    size: Option<PositiveLength>,
) -> Result<(), Error> {
    match (drill, size) {
        (None, Some(_)) => Err(Error::ViaDrillAutoSizeManual),
        (Some(drill), Some(size)) if size < drill => Err(Error::ViaDrillLargerThanSize),
        _ => Ok(()),
    }
}

/// Serializes a drill/size as length or `auto`.
fn serialize_size(size: Option<PositiveLength>) -> SExpression {
    size.map_or_else(|| SExpression::token("auto"), |s| s.to_sexpression())
}

fn deserialize_size(node: &SExpression) -> serialization::Result<Option<PositiveLength>> {
    match node.value()? {
        "auto" => Ok(None),
        _ => PositiveLength::from_sexpression(node).map(Some),
    }
}

impl HasUuid for Via {
    fn uuid(&self) -> Uuid {
        self.uuid
    }
}

object_list!(
    /// List of [`Via`]s.
    ViaList,
    ViaListTag,
    Via,
    "via"
);

impl SerializeObject for Via {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("from", &self.start_layer);
        root.append_child("to", &self.end_layer);
        root.ensure_line_break();
        self.position.serialize(root.append_list("position"));
        root.append_child("drill", &serialize_size(self.drill_diameter));
        root.append_child("size", &serialize_size(self.size));
        root.append_child("exposure", &self.exposure_config);
        root.ensure_line_break();
    }
}

impl DeserializeObject for Via {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self::new(
            node.child_value("@0")?,
            node.child_value("from/@0")?,
            node.child_value("to/@0")?,
            Point::deserialize(node.required_child("position")?)?,
            deserialize_size(node.required_child("drill/@0")?)?,
            deserialize_size(node.required_child("size/@0")?)?,
            node.child_value("exposure/@0")?,
        )?)
    }
}
