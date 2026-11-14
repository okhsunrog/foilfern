//! Port of libs/librepcb/core/library/cmp (components).

mod cmp_sig_pin_display_type;
mod component;
mod component_check;
mod component_check_messages;
mod component_pin_signal_map;
mod component_prefix;
mod component_signal;
mod component_symbol_variant;
mod component_symbol_variant_item;
mod component_symbol_variant_item_suffix;

pub use cmp_sig_pin_display_type::CmpSigPinDisplayType;
pub use component::Component;
pub use component_check::run_component_checks;
pub use component_check_messages::ComponentCheckMessage;
pub use component_pin_signal_map::{
    ComponentPinSignalMap, ComponentPinSignalMapItem, ComponentPinSignalMapTag,
};
pub use component_prefix::{ComponentPrefix, NormDependentPrefixMap, NormDependentPrefixMapPolicy};
pub use component_signal::{ComponentSignal, ComponentSignalList, ComponentSignalListTag};
pub use component_symbol_variant::{
    ComponentSymbolVariant, ComponentSymbolVariantList, ComponentSymbolVariantListTag,
};
pub use component_symbol_variant_item::{
    ComponentSymbolVariantItem, ComponentSymbolVariantItemList, ComponentSymbolVariantItemListTag,
};
pub use component_symbol_variant_item_suffix::ComponentSymbolVariantItemSuffix;
