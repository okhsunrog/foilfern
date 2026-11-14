//! Port of libs/librepcb/core/library/cmp/componentsymbolvariantitem.{h,cpp}.

use std::collections::BTreeSet;

use super::component_pin_signal_map::ComponentPinSignalMap;
use super::component_symbol_variant_item_suffix::ComponentSymbolVariantItemSuffix;
use crate::geometry::{impl_uuid, object_list, property};
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{Angle, Point, Uuid};

/// A symbol of a component symbol variant (also called "gate").
///
/// The UUID, the symbol UUID and the pin-signal-map are part of the
/// component's interface and must never be changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentSymbolVariantItem {
    uuid: Uuid,
    symbol_uuid: Uuid,
    symbol_position: Point,
    symbol_rotation: Angle,
    is_required: bool,
    suffix: ComponentSymbolVariantItemSuffix,
    pin_signal_map: ComponentPinSignalMap,
}

impl ComponentSymbolVariantItem {
    /// Creates an item with an empty pin-signal-map.
    pub fn new(
        uuid: Uuid,
        symbol_uuid: Uuid,
        symbol_position: Point,
        symbol_rotation: Angle,
        is_required: bool,
        suffix: ComponentSymbolVariantItemSuffix,
    ) -> Self {
        Self {
            uuid,
            symbol_uuid,
            symbol_position,
            symbol_rotation,
            is_required,
            suffix,
            pin_signal_map: ComponentPinSignalMap::new(),
        }
    }

    property!(
        /// Returns the UUID of the symbol.
        copy symbol_uuid: Uuid, set_symbol_uuid
    );
    property!(
        /// Returns the position of the symbol (relative to the other items).
        copy symbol_position: Point, set_symbol_position
    );
    property!(
        /// Returns the rotation of the symbol.
        copy symbol_rotation: Angle, set_symbol_rotation
    );
    property!(
        /// Returns whether the symbol must be placed in schematics.
        copy is_required: bool, set_is_required
    );
    property!(
        /// Returns the suffix appended to the component name.
        ref suffix: ComponentSymbolVariantItemSuffix, set_suffix
    );

    /// Returns the pin-signal-map.
    pub fn pin_signal_map(&self) -> &ComponentPinSignalMap {
        &self.pin_signal_map
    }

    /// Returns the pin-signal-map for modification.
    pub fn pin_signal_map_mut(&mut self) -> &mut ComponentPinSignalMap {
        // upstream: emits onEdited(PinSignalMapEdited) on modifications
        &mut self.pin_signal_map
    }
}

impl_uuid!(ComponentSymbolVariantItem);

object_list!(
    /// List of [`ComponentSymbolVariantItem`]s.
    ComponentSymbolVariantItemList,
    ComponentSymbolVariantItemListTag,
    ComponentSymbolVariantItem,
    "gate"
);

impl ComponentSymbolVariantItemList {
    /// Returns the UUIDs of all symbols used by the items (upstream
    /// `ComponentSymbolVariantItemListHelpers::getAllSymbolUuids()`).
    pub fn all_symbol_uuids(&self) -> BTreeSet<Uuid> {
        self.iter().map(|item| item.symbol_uuid).collect()
    }
}

impl SerializeObject for ComponentSymbolVariantItem {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        root.append_child("symbol", &self.symbol_uuid);
        root.ensure_line_break();
        self.symbol_position.serialize(root.append_list("position"));
        root.append_child("rotation", &self.symbol_rotation);
        root.append_child("required", &self.is_required);
        root.append_child("suffix", &self.suffix);
        root.ensure_line_break();
        self.pin_signal_map.sorted_by_uuid().serialize(root);
        root.ensure_line_break();
    }
}

impl DeserializeObject for ComponentSymbolVariantItem {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            symbol_uuid: node.child_value("symbol/@0")?,
            symbol_position: Point::deserialize(node.required_child("position")?)?,
            symbol_rotation: node.child_value("rotation/@0")?,
            is_required: node.child_value("required/@0")?,
            suffix: node.child_value("suffix/@0")?,
            pin_signal_map: ComponentPinSignalMap::deserialize(node)?,
        })
    }
}
