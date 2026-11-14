//! Port of libs/librepcb/core/geometry/text.{h,cpp}.

use super::{impl_uuid, object_list, property};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{Alignment, Angle, Layer, Point, PositiveLength, Uuid};

/// A text rendered with a regular (non-stroke) font, e.g. in symbols.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Text {
    uuid: Uuid,
    layer: Layer,
    text: String,
    position: Point,
    rotation: Angle,
    height: PositiveLength,
    align: Alignment,
    locked: bool,
}

impl Text {
    /// Creates a text.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        uuid: Uuid,
        layer: Layer,
        text: impl Into<String>,
        position: Point,
        rotation: Angle,
        height: PositiveLength,
        align: Alignment,
        locked: bool,
    ) -> Self {
        Self {
            uuid,
            layer,
            text: text.into(),
            position,
            rotation,
            height,
            align,
            locked,
        }
    }

    property!(
        /// Returns the layer.
        copy layer: Layer, set_layer
    );
    property!(
        /// Returns the text (may contain attribute placeholders like
        /// `{{NAME}}`).
        ref text: String, set_text
    );
    property!(
        /// Returns the position.
        copy position: Point, set_position
    );
    property!(
        /// Returns the rotation.
        copy rotation: Angle, set_rotation
    );
    property!(
        /// Returns the text height.
        copy height: PositiveLength, set_height
    );
    property!(
        /// Returns the alignment.
        copy align: Alignment, set_align
    );
    property!(
        /// Returns whether the text is locked.
        copy locked: bool, set_locked
    );
}

impl_uuid!(Text);

object_list!(
    /// List of [`Text`]s.
    TextList,
    TextListTag,
    Text,
    "text"
);

impl SerializeObject for Text {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("layer", &self.layer);
        root.append_child("height", &self.height);
        root.ensure_line_break();
        self.align.serialize(root.append_list("align"));
        self.position.serialize(root.append_list("position"));
        root.append_child("rotation", &self.rotation);
        root.append_child("lock", &self.locked);
        root.ensure_line_break();
        root.append_child("value", &self.text);
        root.ensure_line_break();
    }
}

impl DeserializeObject for Text {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            layer: node.child_value("layer/@0")?,
            text: node.child_value("value/@0")?,
            position: Point::deserialize(node.required_child("position")?)?,
            rotation: node.child_value("rotation/@0")?,
            height: node.child_value("height/@0")?,
            align: Alignment::deserialize(node.required_child("align")?)?,
            locked: node.child_value("lock/@0")?,
        })
    }
}
