//! Port of libs/librepcb/core/library/cmp/componentsymbolvariant.{h,cpp}.

use std::collections::BTreeSet;

use super::component_symbol_variant_item::ComponentSymbolVariantItemList;
use crate::geometry::{impl_uuid, object_list, property};
use crate::serialization::{
    self, DeserializeObject, HasName, List, LocalizedDescriptionMap, LocalizedNameMap, SExpression,
    SerializeObject,
};
use crate::types::{ElementName, Uuid};

/// A symbol variant of a component (e.g. one symbol per gate, or a single
/// symbol for all gates; possibly according to a norm).
///
/// The UUID and the items (their UUIDs, symbols and pin-signal-maps) are
/// part of the component's interface; adding variants is allowed, removing
/// is not.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentSymbolVariant {
    uuid: Uuid,
    norm: String,
    names: LocalizedNameMap,
    descriptions: LocalizedDescriptionMap,
    symbol_items: ComponentSymbolVariantItemList,
}

impl ComponentSymbolVariant {
    /// Creates a variant without items.
    pub fn new(
        uuid: Uuid,
        norm: impl Into<String>,
        name_en_us: ElementName,
        description_en_us: impl Into<String>,
    ) -> Self {
        Self {
            uuid,
            norm: norm.into(),
            names: LocalizedNameMap::new(name_en_us),
            descriptions: LocalizedDescriptionMap::new(description_en_us.into()),
            symbol_items: ComponentSymbolVariantItemList::new(),
        }
    }

    property!(
        /// Returns the norm (may be empty).
        ref norm: String, set_norm
    );
    property!(
        /// Returns the localized names.
        ref names: LocalizedNameMap, set_names
    );
    property!(
        /// Returns the localized descriptions.
        ref descriptions: LocalizedDescriptionMap, set_descriptions
    );

    /// Returns the default (en_US) name.
    pub fn name(&self) -> &ElementName {
        self.names.default_value()
    }

    /// Sets the name for `locale`. Returns whether it was modified.
    pub fn set_name(&mut self, locale: &str, name: ElementName) -> bool {
        // upstream: emits onEdited(NamesChanged)
        self.names.insert(locale, name)
    }

    /// Sets the description for `locale`. Returns whether it was modified.
    pub fn set_description(&mut self, locale: &str, description: impl Into<String>) -> bool {
        // upstream: emits onEdited(DescriptionsChanged)
        self.descriptions.insert(locale, description.into())
    }

    /// Returns the symbol items.
    pub fn symbol_items(&self) -> &ComponentSymbolVariantItemList {
        &self.symbol_items
    }

    /// Returns the symbol items for modification.
    pub fn symbol_items_mut(&mut self) -> &mut ComponentSymbolVariantItemList {
        // upstream: emits onEdited(SymbolItemsEdited) on modifications
        &mut self.symbol_items
    }

    /// Returns the UUIDs of all symbols used by the items.
    pub fn all_symbol_uuids(&self) -> BTreeSet<Uuid> {
        self.symbol_items.all_symbol_uuids()
    }
}

impl_uuid!(ComponentSymbolVariant);

impl HasName for ComponentSymbolVariant {
    fn name(&self) -> &str {
        self.names.default_value().as_str()
    }
}

object_list!(
    /// List of [`ComponentSymbolVariant`]s.
    ComponentSymbolVariantList,
    ComponentSymbolVariantListTag,
    ComponentSymbolVariant,
    "variant"
);

impl SerializeObject for ComponentSymbolVariant {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.append_child("norm", &self.norm);
        root.ensure_line_break();
        self.names.serialize(root);
        root.ensure_line_break();
        self.descriptions.serialize(root);
        root.ensure_line_break();
        self.symbol_items.serialize(root);
        root.ensure_line_break();
    }
}

impl DeserializeObject for ComponentSymbolVariant {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            norm: node.child_value("norm/@0")?,
            names: LocalizedNameMap::deserialize(node)?,
            descriptions: LocalizedDescriptionMap::deserialize(node)?,
            symbol_items: ComponentSymbolVariantItemList::deserialize(node)?,
        })
    }
}
