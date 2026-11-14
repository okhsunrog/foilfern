//! Port of libs/librepcb/core/geometry/stroketext.{h,cpp}.

use super::{Path, impl_uuid, object_list, property};
use crate::font::{StrokeFont, StrokeTextPathBuilder};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{
    Alignment, Angle, Layer, Point, PositiveLength, StrokeTextSpacing, UnsignedLength, Uuid,
};

/// A text rendered with a stroke font, e.g. in footprints and boards.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StrokeText {
    uuid: Uuid,
    layer: Layer,
    text: String,
    position: Point,
    rotation: Angle,
    height: PositiveLength,
    stroke_width: UnsignedLength,
    letter_spacing: StrokeTextSpacing,
    line_spacing: StrokeTextSpacing,
    align: Alignment,
    mirrored: bool,
    auto_rotate: bool,
    locked: bool,
}

impl StrokeText {
    /// Creates a stroke text.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        uuid: Uuid,
        layer: Layer,
        text: impl Into<String>,
        position: Point,
        rotation: Angle,
        height: PositiveLength,
        stroke_width: UnsignedLength,
        letter_spacing: StrokeTextSpacing,
        line_spacing: StrokeTextSpacing,
        align: Alignment,
        mirrored: bool,
        auto_rotate: bool,
        locked: bool,
    ) -> Self {
        Self {
            uuid,
            layer,
            text: text.into(),
            position,
            rotation,
            height,
            stroke_width,
            letter_spacing,
            line_spacing,
            align,
            mirrored,
            auto_rotate,
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
        /// Returns the stroke width.
        copy stroke_width: UnsignedLength, set_stroke_width
    );
    property!(
        /// Returns the letter spacing.
        copy letter_spacing: StrokeTextSpacing, set_letter_spacing
    );
    property!(
        /// Returns the line spacing.
        copy line_spacing: StrokeTextSpacing, set_line_spacing
    );
    property!(
        /// Returns the alignment.
        copy align: Alignment, set_align
    );
    property!(
        /// Returns whether the text is mirrored.
        copy mirrored: bool, set_mirrored
    );
    property!(
        /// Returns whether the text is rotated by 180° when it would be
        /// upside down.
        copy auto_rotate: bool, set_auto_rotate
    );
    property!(
        /// Returns whether the text is locked.
        copy locked: bool, set_locked
    );

    /// Returns the stroke paths of the own text (relative to the text
    /// position, not rotated or mirrored).
    pub fn generate_paths(&self, font: &StrokeFont) -> Vec<Path> {
        self.generate_paths_for(font, &self.text)
    }

    /// Returns the stroke paths of `text` (e.g. with substituted attributes)
    /// rendered with the properties of `self`.
    pub fn generate_paths_for(&self, font: &StrokeFont, text: &str) -> Vec<Path> {
        StrokeTextPathBuilder::build(
            font,
            self.letter_spacing,
            self.line_spacing,
            self.height,
            self.stroke_width,
            self.align,
            self.rotation,
            self.auto_rotate,
            text,
        )
    }
}

impl_uuid!(StrokeText);

object_list!(
    /// List of [`StrokeText`]s.
    StrokeTextList,
    StrokeTextListTag,
    StrokeText,
    "stroke_text"
);

impl SerializeObject for StrokeText {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("layer", &self.layer);
        root.ensure_line_break();
        root.append_child("height", &self.height);
        root.append_child("stroke_width", &self.stroke_width);
        root.append_child("letter_spacing", &self.letter_spacing);
        root.append_child("line_spacing", &self.line_spacing);
        root.ensure_line_break();
        self.align.serialize(root.append_list("align"));
        self.position.serialize(root.append_list("position"));
        root.append_child("rotation", &self.rotation);
        root.append_child("lock", &self.locked);
        root.ensure_line_break();
        root.append_child("auto_rotate", &self.auto_rotate);
        root.append_child("mirror", &self.mirrored);
        root.append_child("value", &self.text);
        root.ensure_line_break();
    }
}

impl DeserializeObject for StrokeText {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            layer: node.child_value("layer/@0")?,
            text: node.child_value("value/@0")?,
            position: Point::deserialize(node.required_child("position")?)?,
            rotation: node.child_value("rotation/@0")?,
            height: node.child_value("height/@0")?,
            stroke_width: node.child_value("stroke_width/@0")?,
            letter_spacing: node.child_value("letter_spacing/@0")?,
            line_spacing: node.child_value("line_spacing/@0")?,
            align: Alignment::deserialize(node.required_child("align")?)?,
            mirrored: node.child_value("mirror/@0")?,
            auto_rotate: node.child_value("auto_rotate/@0")?,
            locked: node.child_value("lock/@0")?,
        })
    }
}
