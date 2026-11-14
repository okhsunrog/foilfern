//! Port of libs/librepcb/core/library/cmp/componentcheck.{h,cpp}.

use std::collections::BTreeSet;

use super::component::Component;
use super::component_check_messages::ComponentCheckMessage as Msg;
use crate::library::sym::has_non_functional_inversion_sign;
use crate::library::{LibraryCheckMessage, LibraryElement, run_element_checks};

type MsgList = Vec<LibraryCheckMessage>;

/// Runs the component check, including the checks common to all elements
/// (upstream `ComponentCheck::runChecks()`).
pub fn run_component_checks(component: &Component, msgs: &mut MsgList) {
    run_element_checks(component.element_metadata(), msgs);
    check_missing_prefix(component, msgs);
    check_missing_default_value(component, msgs);
    check_duplicate_signal_names(component, msgs);
    check_signal_names_inversion_sign(component, msgs);
    check_suspicious_forced_nets(component, msgs);
    check_missing_symbol_variants(component, msgs);
    check_missing_symbol_variant_items(component, msgs);
    check_no_pins_connected(component, msgs);
}

fn check_missing_prefix(component: &Component, msgs: &mut MsgList) {
    if component.prefixes().default_value().is_empty() {
        msgs.push(Msg::MissingPrefix.into());
    }
}

fn check_missing_default_value(component: &Component, msgs: &mut MsgList) {
    if component.default_value().trim().is_empty() {
        msgs.push(Msg::MissingDefaultValue.into());
    }
}

fn check_duplicate_signal_names(component: &Component, msgs: &mut MsgList) {
    let mut names = BTreeSet::new();
    for signal in component.signals() {
        if !names.insert(signal.name()) {
            msgs.push(
                Msg::DuplicateSignalName {
                    name: signal.name().clone(),
                }
                .into(),
            );
        }
    }
}

fn check_signal_names_inversion_sign(component: &Component, msgs: &mut MsgList) {
    for signal in component.signals() {
        if has_non_functional_inversion_sign(signal.name()) {
            msgs.push(
                Msg::NonFunctionalSignalInversionSign {
                    signal: signal.clone(),
                }
                .into(),
            );
        }
    }
}

fn check_suspicious_forced_nets(component: &Component, msgs: &mut MsgList) {
    // Do not emit "suspicious forced net" for single-signal components like
    // supply symbols (GND, VCC, ...).
    if component.signals().len() > 1
        && component
            .signals()
            .iter()
            .any(|s| !s.forced_net_name().is_empty())
    {
        msgs.push(Msg::SuspiciousForcedNets.into());
    }
}

fn check_missing_symbol_variants(component: &Component, msgs: &mut MsgList) {
    if component.symbol_variants().is_empty() {
        msgs.push(Msg::MissingSymbolVariant.into());
    }
}

fn check_missing_symbol_variant_items(component: &Component, msgs: &mut MsgList) {
    for symbol_variant in component.symbol_variants() {
        if symbol_variant.symbol_items().is_empty() {
            msgs.push(
                Msg::MissingSymbolVariantItem {
                    symbol_variant: symbol_variant.clone(),
                }
                .into(),
            );
        }
    }
}

fn check_no_pins_connected(component: &Component, msgs: &mut MsgList) {
    // This warning makes no sense if there are no component signals.
    if component.signals().is_empty() {
        return;
    }
    for symbol_variant in component.symbol_variants() {
        let connected_pins = symbol_variant
            .symbol_items()
            .iter()
            .flat_map(|item| item.pin_signal_map())
            .filter(|map| map.signal_uuid().is_some())
            .count();
        if connected_pins == 0 && !symbol_variant.symbol_items().is_empty() {
            msgs.push(
                Msg::NoPinsInSymbolVariantConnected {
                    symbol_variant: symbol_variant.clone(),
                }
                .into(),
            );
        }
    }
}
