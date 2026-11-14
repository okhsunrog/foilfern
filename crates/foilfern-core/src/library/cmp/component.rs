//! Port of libs/librepcb/core/library/cmp/component.{h,cpp}.

use std::collections::HashMap;

use super::component_check::run_component_checks;
use super::component_pin_signal_map::ComponentPinSignalMapItem;
use super::component_prefix::{ComponentPrefix, NormDependentPrefixMap};
use super::component_signal::{ComponentSignal, ComponentSignalList};
use super::component_symbol_variant::{ComponentSymbolVariant, ComponentSymbolVariantList};
use super::component_symbol_variant_item::ComponentSymbolVariantItem;
use crate::attribute::AttributeList;
use crate::fileio::{FileSystem, TransactionalDirectory};
use crate::geometry::property;
use crate::library::{
    BaseMetadata, ElementMetadata, LibraryBaseElement, LibraryCheckMessage, Result,
    element_accessors, impl_library_element,
};
use crate::serialization::{DeserializeObject, List, SExpression, SerializeObject};
use crate::types::Uuid;

/// A "generic" device in the library, e.g. a resistor or a specific
/// microcontroller, independent of its package.
///
/// The interface of a component (must never be changed): its UUID, the
/// "schematic only" property, the signal UUIDs (and their meaning), and the
/// symbol variants (adding is allowed, removing not) with their UUIDs and
/// items (UUIDs, symbols and pin-signal-maps).
#[derive(Debug)]
pub struct Component {
    directory: TransactionalDirectory,
    metadata: ElementMetadata,
    schematic_only: bool,
    default_value: String,
    prefixes: NormDependentPrefixMap,
    attributes: AttributeList,
    signals: ComponentSignalList,
    symbol_variants: ComponentSymbolVariantList,
}

static_assertions::assert_impl_all!(Component: Send, Sync);

impl Component {
    /// Creates a new, empty component in a temporary directory.
    pub fn new(metadata: BaseMetadata) -> Result<Self> {
        Ok(Self {
            directory: TransactionalDirectory::new_temporary()?,
            metadata: ElementMetadata::new(metadata),
            schematic_only: false,
            default_value: String::new(),
            prefixes: NormDependentPrefixMap::new(ComponentPrefix::default()),
            attributes: AttributeList::new(),
            signals: ComponentSignalList::new(),
            symbol_variants: ComponentSymbolVariantList::new(),
        })
    }

    property!(
        /// Returns whether the component is schematic-only (has no package,
        /// e.g. a supply symbol or a schematic frame).
        copy schematic_only: bool, set_schematic_only
    );
    property!(
        /// Returns the default value (may contain attribute placeholders).
        ref default_value: String, set_default_value
    );
    property!(
        /// Returns the prefixes (by norm).
        ref prefixes: NormDependentPrefixMap, set_prefixes
    );

    /// Returns the attributes.
    pub fn attributes(&self) -> &AttributeList {
        &self.attributes
    }

    /// Returns the attributes for modification.
    pub fn attributes_mut(&mut self) -> &mut AttributeList {
        &mut self.attributes
    }

    /// Returns the signals.
    pub fn signals(&self) -> &ComponentSignalList {
        &self.signals
    }

    /// Returns the signals for modification.
    pub fn signals_mut(&mut self) -> &mut ComponentSignalList {
        &mut self.signals
    }

    /// Returns the symbol variants.
    pub fn symbol_variants(&self) -> &ComponentSymbolVariantList {
        &self.symbol_variants
    }

    /// Returns the symbol variants for modification.
    pub fn symbol_variants_mut(&mut self) -> &mut ComponentSymbolVariantList {
        &mut self.symbol_variants
    }

    /// Returns the symbol variant item `item` of the symbol variant
    /// `symb_var`; fails if one of them does not exist (upstream
    /// `getSymbVarItem()`).
    pub fn symb_var_item(&self, symb_var: Uuid, item: Uuid) -> Result<&ComponentSymbolVariantItem> {
        Ok(self
            .symbol_variants
            .required_by_uuid(&symb_var)?
            .symbol_items()
            .required_by_uuid(&item)?)
    }

    /// Returns the signal connected to the pin `pin` of the symbol variant
    /// item `item` of the symbol variant `symb_var` (`None` if the pin is not
    /// connected); fails if one of them does not exist (upstream
    /// `getSignalOfPin()`).
    pub fn signal_of_pin(
        &self,
        symb_var: Uuid,
        item: Uuid,
        pin: Uuid,
    ) -> Result<Option<&ComponentSignal>> {
        let map = self
            .symb_var_item(symb_var, item)?
            .pin_signal_map()
            .required_by_uuid(&pin)?;
        match map.signal_uuid() {
            Some(signal) => Ok(Some(self.signals.required_by_uuid(&signal)?)),
            None => Ok(None),
        }
    }

    /// Returns the index of the first symbol variant matching the first
    /// matching norm of `norm_order` (norms are compared ignoring case and
    /// all characters except `[0-9A-Z]`).
    pub fn symbol_variant_index_by_norm<S: AsRef<str>>(&self, norm_order: &[S]) -> Option<usize> {
        norm_order.iter().find_map(|norm| {
            let norm = clean_norm(norm.as_ref());
            self.symbol_variants
                .iter()
                .position(|var| clean_norm(var.norm()) == norm)
        })
    }

    /// Makes this component a copy of `other` with new UUIDs of the signals,
    /// symbol variants and items (but keeps the component UUID), removing
    /// all files of this component (upstream `duplicateFrom()`).
    pub fn duplicate_from(&mut self, other: &Component) -> Result<()> {
        self.directory.remove_dir_recursively("")?;
        self.metadata.duplicate_from(&other.metadata);
        self.schematic_only = other.schematic_only;
        self.default_value = other.default_value.clone();
        self.prefixes = other.prefixes.clone();
        self.attributes = other.attributes.clone();

        // Copy signals but generate new UUIDs.
        let mut signal_uuid_map = HashMap::new();
        self.signals = other
            .signals
            .iter()
            .map(|signal| {
                let new_uuid = Uuid::new_random();
                signal_uuid_map.insert(signal.uuid(), new_uuid);
                signal.with_uuid(new_uuid)
            })
            .collect();

        // Copy symbol variants but generate new UUIDs (don't copy
        // translations as they would need to be adjusted anyway).
        self.symbol_variants = other
            .symbol_variants
            .iter()
            .map(|var| {
                let mut copy = ComponentSymbolVariant::new(
                    Uuid::new_random(),
                    var.norm().clone(),
                    var.names().default_value().clone(),
                    var.descriptions().default_value().clone(),
                );
                for item in var.symbol_items() {
                    let mut item_copy = ComponentSymbolVariantItem::new(
                        Uuid::new_random(),
                        item.symbol_uuid(),
                        item.symbol_position(),
                        item.symbol_rotation(),
                        item.is_required(),
                        item.suffix().clone(),
                    );
                    for map in item.pin_signal_map() {
                        let signal = map
                            .signal_uuid()
                            .and_then(|s| signal_uuid_map.get(&s).copied());
                        item_copy
                            .pin_signal_map_mut()
                            .push(ComponentPinSignalMapItem::new(
                                map.pin_uuid(),
                                signal,
                                map.display_type(),
                            ));
                    }
                    copy.symbol_items_mut().push(item_copy);
                }
                copy
            })
            .collect();
        Ok(())
    }
}

/// Normalizes a norm for comparison (upstream `cleanNorm()`).
fn clean_norm(norm: &str) -> String {
    norm.to_uppercase()
        .chars()
        .filter(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        .collect()
}

impl LibraryBaseElement for Component {
    const SHORT_ELEMENT_NAME: &'static str = "cmp";
    const LONG_ELEMENT_NAME: &'static str = "component";

    element_accessors!();

    fn load(directory: TransactionalDirectory, root: &SExpression) -> Result<Self> {
        Ok(Self {
            directory,
            metadata: ElementMetadata::deserialize(root)?,
            schematic_only: root.child_value("schematic_only/@0")?,
            default_value: root.child_value("default_value/@0")?,
            prefixes: NormDependentPrefixMap::deserialize(root)?,
            attributes: AttributeList::deserialize(root)?,
            signals: ComponentSignalList::deserialize(root)?,
            symbol_variants: ComponentSymbolVariantList::deserialize(root)?,
        })
    }

    fn run_checks(&self) -> Result<Vec<LibraryCheckMessage>> {
        let mut msgs = Vec::new();
        run_component_checks(self, &mut msgs);
        Ok(msgs)
    }
}

impl_library_element!(Component);

impl SerializeObject for Component {
    fn serialize(&self, root: &mut List) {
        self.metadata.serialize(root);
        root.ensure_line_break();
        root.append_child("schematic_only", &self.schematic_only);
        root.ensure_line_break();
        root.append_child("default_value", &self.default_value);
        root.ensure_line_break();
        self.prefixes.serialize(root);
        root.ensure_line_break();
        self.attributes.serialize(root);
        root.ensure_line_break();
        self.signals.serialize(root);
        root.ensure_line_break();
        self.symbol_variants.serialize(root);
        root.ensure_line_break();
        self.metadata.base().serialize_message_approvals(root);
        root.ensure_line_break();
    }
}

#[cfg(test)]
mod tests {
    use super::clean_norm;

    #[test]
    fn norm() {
        assert_eq!(clean_norm("IEEE 315-1975"), "IEEE3151975");
        assert_eq!(clean_norm("iec_60617"), "IEC60617");
    }
}
