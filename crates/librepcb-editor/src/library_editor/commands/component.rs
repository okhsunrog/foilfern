//! Component commands: port of libs/librepcb/editor/library/cmd/
//! {cmdcomponentedit,cmdcomponentsignaledit,cmdcomponentsymbolvariantedit,
//! cmdcomponentsymbolvariantitemedit,cmdcomponentpinsignalmapitemedit}.{h,cpp}
//! and of the editing logic of libs/librepcb/editor/library/cmp/
//! {componentsignallistmodel,componentvariantlistmodel,
//! componentgatelistmodel,componentvarianteditor,componentgateeditor}.cpp
//! (signals, symbol variants, gates, pin-signal-map, auto-connect).
//!
//! Symbols are passed in as data (the caller loads them, e.g. with the
//! [`LibraryElementCache`](crate::library_editor::LibraryElementCache));
//! upstream's dialogs (symbol chooser) are the caller's business.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use librepcb_core::attribute::AttributeList;
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::cmp::{
    CmpSigPinDisplayType, Component, ComponentPinSignalMap, ComponentPinSignalMapItem,
    ComponentSignal, ComponentSymbolVariant, ComponentSymbolVariantItem,
    ComponentSymbolVariantItemSuffix, NormDependentPrefixMap,
};
use librepcb_core::library::sym::Symbol;
use librepcb_core::serialization::{LocalizedDescriptionMap, LocalizedNameMap};
use librepcb_core::types::{Angle, CircuitIdentifier, ElementName, Point, SignalRole, Uuid};
use librepcb_core::utils::toolbox;
use librepcb_i18n::tr;

use super::symbol::not_found;
use crate::error::{Error, Result};
use crate::library_editor::ElementCommand;

/// Edits component properties (upstream `CmdComponentEdit` and the
/// attribute list); `None` keeps a value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditComponent {
    /// Schematic-only.
    pub schematic_only: Option<bool>,
    /// Default value.
    pub default_value: Option<String>,
    /// Prefixes.
    pub prefixes: Option<NormDependentPrefixMap>,
    /// Attributes.
    pub attributes: Option<AttributeList>,
}

impl ElementCommand<Component> for EditComponent {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdComponentEdit",
            "Edit Component Properties"
        )
    }

    fn execute(self, cmp: &mut Component) -> Result<()> {
        if let Some(v) = self.schematic_only {
            cmp.set_schematic_only(v);
        }
        if let Some(v) = self.default_value {
            cmp.set_default_value(v);
        }
        if let Some(v) = self.prefixes {
            cmp.set_prefixes(v);
        }
        if let Some(v) = self.attributes {
            *cmp.attributes_mut() = v;
        }
        Ok(())
    }
}

// --- Signals ---

fn duplicate_signal_error(name: &str) -> Error {
    Error::InvalidArgument(tr!(
        "librepcb::editor::ComponentSignalListModel",
        "There is already a signal with the name \"{0}\".",
        name
    ))
}

/// Cleans a forced net name (upstream
/// `ComponentSignalListModel::cleanForcedNetName()`: like circuit
/// identifiers, but allowing attribute placeholders).
pub fn clean_forced_net_name(name: &str) -> String {
    toolbox::clean_user_input_string(
        name,
        |c| c.is_ascii_alphanumeric() || "-_+/!?@#${}".contains(c),
        true,
        false,
        false,
        "",
        None,
    )
}

/// Adds signals (upstream `ComponentSignalListModel::add()`): `names` may
/// contain ranges (e.g. `"D0..7"`). New signals are passive, not required.
/// Returns the UUIDs of the new signals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddComponentSignals {
    /// Signal names (with ranges).
    pub names: String,
}

impl ElementCommand<Component> for AddComponentSignals {
    type Output = Vec<Uuid>;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::ComponentSignalListModel",
            "Add Component Signal(s)"
        )
    }

    fn execute(self, cmp: &mut Component) -> Result<Vec<Uuid>> {
        let mut uuids = Vec::new();
        for name in toolbox::expand_ranges_in_string(&self.names) {
            let name = CircuitIdentifier::clean(&name);
            if cmp.signals().contains_name(&name) {
                return Err(duplicate_signal_error(&name));
            }
            let uuid = Uuid::new_random();
            cmp.signals_mut().push(ComponentSignal::new(
                uuid,
                CircuitIdentifier::new(name)?,
                SignalRole::Passive,
                String::new(),
                false,
                false,
                false,
            ));
            uuids.push(uuid);
        }
        Ok(uuids)
    }
}

/// Edits a signal (upstream `CmdComponentSignalEdit`); `None` keeps a
/// value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditComponentSignal {
    /// The signal.
    pub signal: Uuid,
    /// New name (must be unique).
    pub name: Option<CircuitIdentifier>,
    /// New role.
    pub role: Option<SignalRole>,
    /// New forced net name (see [`clean_forced_net_name()`]).
    pub forced_net_name: Option<String>,
    /// Whether the signal is required.
    pub required: Option<bool>,
    /// Whether the signal is negated.
    pub negated: Option<bool>,
    /// Whether the signal is a clock.
    pub clock: Option<bool>,
}

impl EditComponentSignal {
    /// An edit of `signal` which does not change anything yet.
    pub fn new(signal: Uuid) -> Self {
        Self {
            signal,
            name: None,
            role: None,
            forced_net_name: None,
            required: None,
            negated: None,
            clock: None,
        }
    }
}

impl ElementCommand<Component> for EditComponentSignal {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdComponentSignalEdit",
            "Edit component signal"
        )
    }

    fn execute(self, cmp: &mut Component) -> Result<()> {
        if let Some(name) = &self.name
            && cmp
                .signals()
                .iter()
                .any(|s| s.uuid() != self.signal && s.name() == name)
        {
            return Err(duplicate_signal_error(name.as_str()));
        }
        let sig = cmp
            .signals_mut()
            .by_uuid_mut(&self.signal)
            .ok_or_else(|| not_found("signal", self.signal))?;
        if let Some(v) = self.name {
            sig.set_name(v);
        }
        if let Some(v) = self.role {
            sig.set_role(v);
        }
        if let Some(v) = self.forced_net_name {
            sig.set_forced_net_name(v);
        }
        if let Some(v) = self.required {
            sig.set_is_required(v);
        }
        if let Some(v) = self.negated {
            sig.set_is_negated(v);
        }
        if let Some(v) = self.clock {
            sig.set_is_clock(v);
        }
        Ok(())
    }
}

/// Removes a signal and disconnects all pins from it (upstream
/// `ComponentSignalListModel` "delete").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveComponentSignal {
    /// The signal.
    pub signal: Uuid,
}

impl ElementCommand<Component> for RemoveComponentSignal {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::ComponentSignalListModel",
            "Delete Component Signal"
        )
    }

    fn execute(self, cmp: &mut Component) -> Result<()> {
        for variant in cmp.symbol_variants_mut().iter_mut() {
            for gate in variant.symbol_items_mut().iter_mut() {
                for item in gate.pin_signal_map_mut().iter_mut() {
                    if item.signal_uuid() == Some(self.signal) {
                        item.set_signal_uuid(None);
                    }
                }
            }
        }
        cmp.signals_mut()
            .take_by_uuid(&self.signal)
            .ok_or_else(|| not_found("signal", self.signal))?;
        Ok(())
    }
}

// --- Symbol variants ---

/// Adds a symbol variant with one gate of `symbol` (upstream
/// `ComponentVariantListModel::add()`); the name is "default" for the first
/// variant, else "Variant <n>". Returns the UUID of the variant.
#[derive(Debug, Clone)]
pub struct AddSymbolVariant<'a> {
    /// The symbol of the first gate.
    pub symbol: &'a Symbol,
}

impl ElementCommand<Component> for AddSymbolVariant<'_> {
    type Output = Uuid;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdListElementInsert",
            "Add {0}",
            "variant"
        )
    }

    fn execute(self, cmp: &mut Component) -> Result<Uuid> {
        let count = cmp.symbol_variants().len();
        let name = if count == 0 {
            "default".to_owned()
        } else {
            tr!(
                "librepcb::editor::ComponentVariantListModel",
                "Variant {0}",
                count + 1
            )
        };
        let name = ElementName::new(name)
            .or_else(|_| ElementName::new(format!("Variant {}", count + 1)))?;
        let mut variant = ComponentSymbolVariant::new(Uuid::new_random(), "", name, "");
        variant.symbol_items_mut().push(new_gate(self.symbol, None));
        let uuid = variant.uuid();
        cmp.symbol_variants_mut().push(variant);
        Ok(uuid)
    }
}

/// Edits a symbol variant (upstream `CmdComponentSymbolVariantEdit`);
/// `None` keeps a value.
#[derive(Debug, Clone, PartialEq)]
pub struct EditSymbolVariant {
    /// The variant.
    pub variant: Uuid,
    /// New norm.
    pub norm: Option<String>,
    /// New names.
    pub names: Option<LocalizedNameMap>,
    /// New descriptions.
    pub descriptions: Option<LocalizedDescriptionMap>,
}

impl EditSymbolVariant {
    /// An edit of `variant` which does not change anything yet.
    pub fn new(variant: Uuid) -> Self {
        Self {
            variant,
            norm: None,
            names: None,
            descriptions: None,
        }
    }
}

impl ElementCommand<Component> for EditSymbolVariant {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdComponentSymbolVariantEdit",
            "Edit component symbol variant"
        )
    }

    fn execute(self, cmp: &mut Component) -> Result<()> {
        let var = variant_mut(cmp, self.variant)?;
        if let Some(v) = self.norm {
            var.set_norm(v);
        }
        if let Some(v) = self.names {
            var.set_names(v);
        }
        if let Some(v) = self.descriptions {
            var.set_descriptions(v);
        }
        Ok(())
    }
}

/// Moves a symbol variant one position up (upstream
/// `CmdComponentSymbolVariantsSwap`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveSymbolVariantUp {
    /// The variant.
    pub variant: Uuid,
}

impl ElementCommand<Component> for MoveSymbolVariantUp {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdListElementsSwap",
            "Move {0}",
            "variant"
        )
    }

    fn execute(self, cmp: &mut Component) -> Result<()> {
        let index = cmp
            .symbol_variants()
            .index_of_uuid(&self.variant)
            .ok_or_else(|| not_found("symbol variant", self.variant))?;
        if index > 0 {
            cmp.symbol_variants_mut().swap(index, index - 1);
        }
        Ok(())
    }
}

/// Makes a symbol variant the default one (moves it to the top, upstream
/// "Set Default Component Variant").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetDefaultSymbolVariant {
    /// The variant.
    pub variant: Uuid,
}

impl ElementCommand<Component> for SetDefaultSymbolVariant {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::ComponentVariantListModel",
            "Set Default Component Variant"
        )
    }

    fn execute(self, cmp: &mut Component) -> Result<()> {
        let variant = cmp
            .symbol_variants_mut()
            .take_by_uuid(&self.variant)
            .ok_or_else(|| not_found("symbol variant", self.variant))?;
        cmp.symbol_variants_mut().insert(0, variant);
        Ok(())
    }
}

/// Removes a symbol variant (upstream `CmdComponentSymbolVariantRemove`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveSymbolVariant {
    /// The variant.
    pub variant: Uuid,
}

impl ElementCommand<Component> for RemoveSymbolVariant {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdListElementRemove",
            "Remove {0}",
            "variant"
        )
    }

    fn execute(self, cmp: &mut Component) -> Result<()> {
        cmp.symbol_variants_mut()
            .take_by_uuid(&self.variant)
            .ok_or_else(|| not_found("symbol variant", self.variant))?;
        Ok(())
    }
}

fn variant_mut(cmp: &mut Component, uuid: Uuid) -> Result<&mut ComponentSymbolVariant> {
    cmp.symbol_variants_mut()
        .by_uuid_mut(&uuid)
        .ok_or_else(|| not_found("symbol variant", uuid))
}

fn gate_mut(
    cmp: &mut Component,
    variant: Uuid,
    gate: Uuid,
) -> Result<&mut ComponentSymbolVariantItem> {
    variant_mut(cmp, variant)?
        .symbol_items_mut()
        .by_uuid_mut(&gate)
        .ok_or_else(|| not_found("gate", gate))
}

// --- Gates ---

/// Creates a gate of `symbol` at the origin with a pin-signal-map of all
/// pins (connected to `signals` if given, by pin UUID).
fn new_gate(symbol: &Symbol, signals: Option<&BTreeMap<Uuid, Uuid>>) -> ComponentSymbolVariantItem {
    let mut gate = ComponentSymbolVariantItem::new(
        Uuid::new_random(),
        symbol.metadata().uuid(),
        Point::ORIGIN,
        Angle::DEG0,
        true,
        ComponentSymbolVariantItemSuffix::default(),
    );
    for pin in symbol.pins().iter() {
        gate.pin_signal_map_mut()
            .push(ComponentPinSignalMapItem::new(
                pin.uuid(),
                signals.and_then(|s| s.get(&pin.uuid()).copied()),
                CmpSigPinDisplayType::ComponentSignal,
            ));
    }
    gate
}

/// Upstream `appendNumberToSignalName()`.
fn append_number_to_signal_name(name: &str, number: usize) -> String {
    let mut name: String = name.chars().take(32 - 4).collect();
    if name.chars().last().is_some_and(|c| c.is_ascii_digit()) {
        name.push('_');
    }
    name.push_str(&number.to_string());
    CircuitIdentifier::clean(&name)
}

/// Updates the suffixes of the gates of a variant to "A", "B", ... (none
/// for a single gate), upstream `ComponentGateListModel::createSuffixUpdateCmd()`.
fn update_suffixes(variant: &mut ComponentSymbolVariant) {
    const SUFFIXES: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let count = variant.symbol_items().len();
    for (i, gate) in variant.symbol_items_mut().iter_mut().enumerate() {
        let suffix = if count == 1 {
            String::new()
        } else {
            SUFFIXES
                .chars()
                .nth(i)
                .map(String::from)
                .unwrap_or_default()
        };
        let suffix =
            ComponentSymbolVariantItemSuffix::new(ComponentSymbolVariantItemSuffix::clean(&suffix))
                .unwrap_or_default();
        gate.set_suffix(suffix);
    }
}

/// Adds a gate of `symbol` to a variant (upstream
/// `ComponentGateListModel::add()`); with `create_signals` (upstream wizard
/// mode), a signal is created for every pin. Gate suffixes are updated.
/// Returns the UUID of the gate.
#[derive(Debug, Clone)]
pub struct AddGate<'a> {
    /// The variant.
    pub variant: Uuid,
    /// The symbol.
    pub symbol: &'a Symbol,
    /// Create a signal for every pin (named like the pin, made unique).
    pub create_signals: bool,
}

impl ElementCommand<Component> for AddGate<'_> {
    type Output = Uuid;

    fn text(&self) -> String {
        "Add Component Gate".to_owned()
    }

    fn execute(self, cmp: &mut Component) -> Result<Uuid> {
        variant_mut(cmp, self.variant)?;
        let mut signals = BTreeMap::new();
        if self.create_signals {
            for pin in self.symbol.pins().iter() {
                let mut name = pin.name().as_str().to_owned();
                let mut number = 2;
                while cmp.signals().contains_name(&name) {
                    name = append_number_to_signal_name(pin.name().as_str(), number);
                    number += 1;
                }
                let sig = ComponentSignal::new(
                    Uuid::new_random(),
                    CircuitIdentifier::new(name)?,
                    SignalRole::Passive,
                    String::new(),
                    false,
                    false,
                    false,
                );
                signals.insert(pin.uuid(), sig.uuid());
                cmp.signals_mut().push(sig);
            }
        }
        let gate = new_gate(self.symbol, Some(&signals));
        let uuid = gate.uuid();
        let var = variant_mut(cmp, self.variant)?;
        var.symbol_items_mut().push(gate);
        update_suffixes(var);
        Ok(uuid)
    }
}

/// Edits a gate (upstream `CmdComponentSymbolVariantItemEdit`); `None`
/// keeps a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditGate {
    /// The variant.
    pub variant: Uuid,
    /// The gate.
    pub gate: Uuid,
    /// New symbol position.
    pub position: Option<Point>,
    /// New symbol rotation.
    pub rotation: Option<Angle>,
    /// Whether the gate is required.
    pub required: Option<bool>,
    /// New suffix.
    pub suffix: Option<ComponentSymbolVariantItemSuffix>,
}

impl EditGate {
    /// An edit of `gate` which does not change anything yet.
    pub fn new(variant: Uuid, gate: Uuid) -> Self {
        Self {
            variant,
            gate,
            position: None,
            rotation: None,
            required: None,
            suffix: None,
        }
    }
}

impl ElementCommand<Component> for EditGate {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdComponentSymbolVariantItemEdit",
            "Edit Component Gate"
        )
    }

    fn execute(self, cmp: &mut Component) -> Result<()> {
        let gate = gate_mut(cmp, self.variant, self.gate)?;
        if let Some(v) = self.position {
            gate.set_symbol_position(v);
        }
        if let Some(v) = self.rotation {
            gate.set_symbol_rotation(v);
        }
        if let Some(v) = self.required {
            gate.set_is_required(v);
        }
        if let Some(v) = self.suffix {
            gate.set_suffix(v);
        }
        Ok(())
    }
}

/// Replaces the symbol of a gate; the pin-signal-map is recreated
/// (unconnected) for the new symbol's pins (upstream
/// `ComponentGateEditor::chooseSymbol()`).
#[derive(Debug, Clone)]
pub struct SetGateSymbol<'a> {
    /// The variant.
    pub variant: Uuid,
    /// The gate.
    pub gate: Uuid,
    /// The new symbol.
    pub symbol: &'a Symbol,
}

impl ElementCommand<Component> for SetGateSymbol<'_> {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::ComponentGateEditor",
            "Edit Component Gate"
        )
    }

    fn execute(self, cmp: &mut Component) -> Result<()> {
        let gate = gate_mut(cmp, self.variant, self.gate)?;
        let symbol_uuid = self.symbol.metadata().uuid();
        if gate.symbol_uuid() == symbol_uuid {
            return Ok(());
        }
        gate.set_symbol_uuid(symbol_uuid);
        *gate.pin_signal_map_mut() = ComponentPinSignalMap::with_pins(
            self.symbol.pins().iter().map(|p| p.uuid()),
            CmpSigPinDisplayType::ComponentSignal,
        );
        Ok(())
    }
}

/// Moves a gate one position up (upstream
/// `CmdComponentSymbolVariantItemsSwap`); suffixes are updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveGateUp {
    /// The variant.
    pub variant: Uuid,
    /// The gate.
    pub gate: Uuid,
}

impl ElementCommand<Component> for MoveGateUp {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdListElementsSwap", "Move {0}", "gate")
    }

    fn execute(self, cmp: &mut Component) -> Result<()> {
        let var = variant_mut(cmp, self.variant)?;
        let index = var
            .symbol_items()
            .index_of_uuid(&self.gate)
            .ok_or_else(|| not_found("gate", self.gate))?;
        if index > 0 {
            var.symbol_items_mut().swap(index, index - 1);
        }
        update_suffixes(var);
        Ok(())
    }
}

/// Removes a gate (upstream `ComponentGateListModel` "delete"); with
/// `remove_signals` (upstream wizard mode) the signals of its pins are
/// removed too. Suffixes are updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveGate {
    /// The variant.
    pub variant: Uuid,
    /// The gate.
    pub gate: Uuid,
    /// Remove the connected signals.
    pub remove_signals: bool,
}

impl ElementCommand<Component> for RemoveGate {
    type Output = ();

    fn text(&self) -> String {
        "Remove Component Gate".to_owned()
    }

    fn execute(self, cmp: &mut Component) -> Result<()> {
        let var = variant_mut(cmp, self.variant)?;
        let gate = var
            .symbol_items_mut()
            .take_by_uuid(&self.gate)
            .ok_or_else(|| not_found("gate", self.gate))?;
        update_suffixes(var);
        if self.remove_signals {
            for item in gate.pin_signal_map().iter() {
                if let Some(sig) = item.signal_uuid() {
                    cmp.signals_mut().take_by_uuid(&sig);
                }
            }
        }
        Ok(())
    }
}

// --- Pin-signal-map ---

/// Connects a pin of a gate to a signal and/or sets its display type
/// (upstream `CmdComponentPinSignalMapItemEdit`); `None` keeps a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetPinSignal {
    /// The variant.
    pub variant: Uuid,
    /// The gate.
    pub gate: Uuid,
    /// The symbol pin.
    pub pin: Uuid,
    /// New signal (`Some(None)`: unconnected).
    pub signal: Option<Option<Uuid>>,
    /// New display type.
    pub display_type: Option<CmpSigPinDisplayType>,
}

impl ElementCommand<Component> for SetPinSignal {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdComponentPinSignalMapItemEdit",
            "Edit Component Pinout"
        )
    }

    fn execute(self, cmp: &mut Component) -> Result<()> {
        if let Some(Some(sig)) = self.signal
            && !cmp.signals().contains_uuid(&sig)
        {
            return Err(not_found("signal", sig));
        }
        let item = gate_mut(cmp, self.variant, self.gate)?
            .pin_signal_map_mut()
            .by_uuid_mut(&self.pin)
            .ok_or_else(|| not_found("pin", self.pin))?;
        if let Some(v) = self.signal {
            item.set_signal_uuid(v);
        }
        if let Some(v) = self.display_type {
            item.set_display_type(v);
        }
        Ok(())
    }
}

/// Connects the pins of all gates of a variant to the signals with the
/// same name (upstream `ComponentVariantEditor::autoConnectPins()`): if
/// several pins have the same name, signals with a number appended are
/// used (e.g. `GND1`, `GND2`). Unmatched pins are disconnected.
#[derive(Debug, Clone)]
pub struct AutoConnectPins<'a> {
    /// The variant.
    pub variant: Uuid,
    /// The symbols of the gates (by symbol UUID); gates whose symbol is
    /// missing are skipped.
    pub symbols: &'a HashMap<Uuid, &'a Symbol>,
}

impl ElementCommand<Component> for AutoConnectPins<'_> {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::ComponentVariantEditor",
            "Auto-Assign Component Signals"
        )
    }

    fn execute(self, cmp: &mut Component) -> Result<()> {
        let signals = cmp.signals().clone();
        let mut numbers: HashMap<String, usize> = HashMap::new();
        let var = variant_mut(cmp, self.variant)?;
        for gate in var.symbol_items_mut().iter_mut() {
            let Some(symbol) = self.symbols.get(&gate.symbol_uuid()) else {
                continue;
            };
            for item in gate.pin_signal_map_mut().iter_mut() {
                let Some(pin) = symbol.pins().by_uuid(&item.pin_uuid()) else {
                    continue;
                };
                let pin_name = pin.name().as_str();
                let mut signal = signals.by_name(pin_name, true);
                if signal.is_none() {
                    // Also look for names with a number at the end.
                    let number = numbers.get(pin_name).copied().unwrap_or(0) + 1;
                    signal = signals.by_name(&append_number_to_signal_name(pin_name, number), true);
                    if signal.is_some() {
                        numbers.insert(pin_name.to_owned(), number);
                    }
                }
                item.set_signal_uuid(signal.map(|s| s.uuid()));
            }
        }
        Ok(())
    }
}

/// Whether a variant has unconnected pins while signals are unused
/// (upstream `ComponentVariantEditor::updateUnassignedSignals()`, shows the
/// "auto-connect" hint).
pub fn has_unassigned_signals(cmp: &Component, variant: Uuid) -> bool {
    let Some(var) = cmp.symbol_variants().by_uuid(&variant) else {
        return false;
    };
    let mut connected = BTreeSet::new();
    let mut unconnected_pins = 0;
    for gate in var.symbol_items().iter() {
        for item in gate.pin_signal_map().iter() {
            match item.signal_uuid() {
                Some(u) => {
                    connected.insert(u);
                }
                None => unconnected_pins += 1,
            }
        }
    }
    unconnected_pins > 0 && cmp.signals().iter().any(|s| !connected.contains(&s.uuid()))
}
