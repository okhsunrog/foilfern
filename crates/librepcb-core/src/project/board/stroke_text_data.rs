//! Port of libs/librepcb/core/project/board/boardstroketextdata.{h,cpp} (and
//! `items/bi_stroketext.{h,cpp}`, whose state is this data; the rendered
//! paths are not cached, they are computed by the consumers with
//! [`StrokeTextPathBuilder`](crate::font::StrokeTextPathBuilder)).

use crate::geometry::{StrokeText, property};
use crate::serialization::{self, DeserializeObject, HasUuid, List, SExpression, SerializeObject};
use crate::types::{
    Alignment, Angle, Layer, Point, PositiveLength, StrokeTextSpacing, UnsignedLength, Uuid,
};

/// A stroke text of a board or of a board device.
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BoardStrokeTextData {
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

impl BoardStrokeTextData {
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

    /// Creates the board text of a (transformed) footprint stroke text with
    /// the given lock state (upstream `BI_Device` constructor).
    pub fn from_stroke_text(text: &StrokeText, locked: bool) -> Self {
        Self::new(
            text.uuid(),
            text.layer(),
            text.text(),
            text.position(),
            text.rotation(),
            text.height(),
            text.stroke_width(),
            text.letter_spacing(),
            text.line_spacing(),
            text.align(),
            text.mirrored(),
            text.auto_rotate(),
            locked,
        )
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

    property!(
        /// Returns the layer.
        copy layer: Layer, set_layer
    );
    property!(
        /// Returns the text.
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
        /// Returns whether the text is rotated automatically to be readable.
        copy auto_rotate: bool, set_auto_rotate
    );
    property!(
        /// Returns whether the text is locked.
        copy locked: bool, set_locked
    );
}

impl HasUuid for BoardStrokeTextData {
    fn uuid(&self) -> Uuid {
        self.uuid
    }
}

impl SerializeObject for BoardStrokeTextData {
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
        root.ensure_line_break();
        root.append_child("auto_rotate", &self.auto_rotate);
        root.append_child("mirror", &self.mirrored);
        root.append_child("lock", &self.locked);
        root.append_child("value", &self.text);
        root.ensure_line_break();
    }
}

impl DeserializeObject for BoardStrokeTextData {
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

impl BoardStrokeTextData {
    /// Expands attributes and strokes this board/device text in local text
    /// coordinates (upstream `BI_StrokeText::updateText()/updatePaths()`).
    pub fn paths(
        &self,
        project: &crate::project::Project,
        board: &super::Board,
        device: Option<&super::BoardDevice>,
    ) -> crate::project::Result<Vec<crate::geometry::Path>> {
        use crate::font::StrokeTextPathBuilder;
        use crate::project::ProjectAttributeLookup;
        let parts = device
            .map(|device| device.parts(project.circuit(), None))
            .transpose()?
            .unwrap_or_default();
        let lookup = match device {
            Some(device) => ProjectAttributeLookup::device(project, board, device, parts.first()),
            None => ProjectAttributeLookup::board(project, board, None),
        };
        let text = lookup.substitute(self.text())?;
        let font = project
            .stroke_fonts()
            .font(&board.settings().default_font)?;
        Ok(StrokeTextPathBuilder::build(
            font,
            self.letter_spacing(),
            self.line_spacing(),
            self.height(),
            self.stroke_width(),
            self.align(),
            self.rotation(),
            self.auto_rotate(),
            &text,
        ))
    }

    /// Returns the placement of the locally stroked paths on the board.
    pub fn transform(&self) -> crate::utils::transform::Transform {
        crate::utils::transform::Transform::new(self.position(), self.rotation(), self.mirrored())
    }
}
