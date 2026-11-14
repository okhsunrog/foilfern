//! Port of libs/librepcb/core/library/sym/symbolpin.{h,cpp}.

use crate::geometry::{impl_uuid, object_list, property};
use crate::serialization::{self, DeserializeObject, HasName, List, SExpression, SerializeObject};
use crate::types::{
    Alignment, Angle, CircuitIdentifier, HAlign, Length, Point, PositiveLength, UnsignedLength,
    Uuid, VAlign,
};

fn positive(nm: i64) -> PositiveLength {
    PositiveLength::new(Length::new(nm)).expect("constant is positive")
}

/// A pin of a symbol.
///
/// The UUID is part of the symbol's interface and must never be changed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SymbolPin {
    uuid: Uuid,
    name: CircuitIdentifier,
    position: Point,
    length: UnsignedLength,
    rotation: Angle,
    name_position: Point,
    name_rotation: Angle,
    name_height: PositiveLength,
    name_alignment: Alignment,
}

impl SymbolPin {
    /// Creates a pin.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        uuid: Uuid,
        name: CircuitIdentifier,
        position: Point,
        length: UnsignedLength,
        rotation: Angle,
        name_position: Point,
        name_rotation: Angle,
        name_height: PositiveLength,
        name_alignment: Alignment,
    ) -> Self {
        Self {
            uuid,
            name,
            position,
            length,
            rotation,
            name_position,
            name_rotation,
            name_height,
            name_alignment,
        }
    }

    property!(
        /// Returns the name.
        ref name: CircuitIdentifier, set_name
    );
    property!(
        /// Returns the position (of the connection point).
        copy position: Point, set_position
    );
    property!(
        /// Returns the length of the pin line.
        copy length: UnsignedLength, set_length
    );
    property!(
        /// Returns the rotation.
        copy rotation: Angle, set_rotation
    );
    property!(
        /// Returns the position of the name text (relative to the pin).
        copy name_position: Point, set_name_position
    );
    property!(
        /// Returns the rotation of the name text (relative to the pin).
        copy name_rotation: Angle, set_name_rotation
    );
    property!(
        /// Returns the height of the name text.
        copy name_height: PositiveLength, set_name_height
    );
    property!(
        /// Returns the alignment of the name text.
        copy name_alignment: Alignment, set_name_alignment
    );

    /// Returns the position of the pin numbers text (relative to the pin).
    pub fn numbers_position(&self, flipped: bool) -> Point {
        Point::new(
            *self.length - Length::new(400_000),
            Length::new(if flipped { -50_000 } else { 50_000 }),
        )
    }

    /// Returns the default name position for a pin of length `length`.
    pub fn default_name_position(length: UnsignedLength) -> Point {
        Point::new(*length + Length::new(1_270_000), Length::ZERO)
    }

    /// Returns the default name height (2.5mm).
    pub fn default_name_height() -> PositiveLength {
        positive(2_500_000)
    }

    /// Returns the default name alignment (left center).
    pub fn default_name_alignment() -> Alignment {
        Alignment::new(HAlign::Left, VAlign::Center)
    }

    /// Returns the height of the pin numbers text (1.5mm).
    pub fn numbers_height() -> PositiveLength {
        positive(1_500_000)
    }

    /// Returns the alignment of the pin numbers text.
    pub fn numbers_alignment(flipped: bool) -> Alignment {
        Alignment::new(
            HAlign::Right,
            if flipped { VAlign::Top } else { VAlign::Bottom },
        )
    }
}

impl_uuid!(SymbolPin);

impl HasName for SymbolPin {
    fn name(&self) -> &str {
        self.name.as_str()
    }
}

object_list!(
    /// List of [`SymbolPin`]s.
    SymbolPinList,
    SymbolPinListTag,
    SymbolPin,
    "pin"
);

impl SerializeObject for SymbolPin {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("name", &self.name);
        root.ensure_line_break();
        self.position.serialize(root.append_list("position"));
        root.append_child("rotation", &self.rotation);
        root.append_child("length", &self.length);
        root.ensure_line_break();
        self.name_position
            .serialize(root.append_list("name_position"));
        root.append_child("name_rotation", &self.name_rotation);
        root.append_child("name_height", &self.name_height);
        root.ensure_line_break();
        self.name_alignment
            .serialize(root.append_list("name_align"));
        root.ensure_line_break();
    }
}

impl DeserializeObject for SymbolPin {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            name: node.child_value("name/@0")?,
            position: Point::deserialize(node.required_child("position")?)?,
            length: node.child_value("length/@0")?,
            rotation: node.child_value("rotation/@0")?,
            name_position: Point::deserialize(node.required_child("name_position")?)?,
            name_rotation: node.child_value("name_rotation/@0")?,
            name_height: node.child_value("name_height/@0")?,
            name_alignment: Alignment::deserialize(node.required_child("name_align")?)?,
        })
    }
}
