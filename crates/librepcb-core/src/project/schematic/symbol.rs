//! Port of libs/librepcb/core/project/schematic/items/si_symbol.{h,cpp}
//! (with `si_symbolpin.{h,cpp}`, whose pins become the computed
//! [`SymbolPinView`]s, and the symbol texts of `si_text.{h,cpp}`).
//!
//! Differences to upstream:
//! - The symbol stores the component instance and the gate (symbol variant
//!   item) by UUID; the library symbol, the gate and the pins are resolved
//!   on demand through a [`ProjectView`] ([`SchematicSymbol::resolve()`],
//!   [`SchematicSymbol::pins()`]). Pins are not stored: the only
//!   non-derivable pin state ("which net lines touch it") is the
//!   schematic's anchor index.
//! - Pin numbers need the pad names of the component's primary device
//!   (board data); [`SymbolPinView::numbers_text()`] takes them as argument.
//!   The truncation uses the length of the text built so far (upstream
//!   accidentally uses the previously cached text, see `COMPAT.md`).
//! - The attribute substitution of texts (`SI_Text::updateText()`) is done
//!   by the renderer with
//!   [`ProjectAttributeLookup::for_symbol()`](crate::project::ProjectAttributeLookup::for_symbol).
//!   Not ported: the forced net name / `hasError()` of pins; the
//!   `attributesChanged` signals.

use std::collections::{BTreeMap, BTreeSet};

use super::uuid_map;
use crate::geometry::{NetLineAnchor, Text, property};
use crate::library::cmp::{
    CmpSigPinDisplayType, Component, ComponentPinSignalMapItem, ComponentSignal,
    ComponentSymbolVariantItem,
};
use crate::library::sym::{Symbol, SymbolPin};
use crate::project::ProjectView;
use crate::project::circuit::ComponentInstance;
use crate::project::error::{Error, Result};
use crate::project::id::{ComponentInstanceId, ComponentSignalRef, NetSignalId, SymbolId};
use crate::serialization::{HasUuid, List, SerializeObject};
use crate::types::{Alignment, Angle, Point, Uuid};
use crate::utils::toolbox;
use crate::utils::transform::{HasTransform, Transform};

/// A symbol placed in a schematic: one gate (symbol variant item) of a
/// component instance.
///
/// Texts are keyed by UUID (upstream `QMap`, file order). The library texts
/// keep their UUIDs when copied into the schematic ([`default_texts()`](Self::default_texts)).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SchematicSymbol {
    uuid: Uuid,
    component: ComponentInstanceId,
    lib_gate: Uuid,
    position: Point,
    rotation: Angle,
    mirrored: bool,
    #[serde(deserialize_with = "uuid_map::deserialize")]
    texts: BTreeMap<Uuid, Text>,
}

impl SchematicSymbol {
    /// Creates a symbol without texts.
    pub fn new(
        uuid: Uuid,
        component: ComponentInstanceId,
        lib_gate: Uuid,
        position: Point,
        rotation: Angle,
        mirrored: bool,
    ) -> Self {
        Self {
            uuid,
            component,
            lib_gate,
            position,
            rotation,
            mirrored,
            texts: BTreeMap::new(),
        }
    }

    /// Returns the UUID.
    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    /// Returns the typed identifier.
    pub fn id(&self) -> SymbolId {
        SymbolId(self.uuid)
    }

    /// Returns the component instance.
    pub fn component(&self) -> ComponentInstanceId {
        self.component
    }

    /// Returns the UUID of the gate (symbol variant item of the component).
    pub fn lib_gate(&self) -> Uuid {
        self.lib_gate
    }

    property!(
        /// Returns the position.
        copy position: Point, set_position
    );
    property!(
        /// Returns the rotation.
        copy rotation: Angle, set_rotation
    );
    property!(
        /// Returns whether the symbol is mirrored.
        copy mirrored: bool, set_mirrored
    );

    /// Returns the texts.
    pub fn texts(&self) -> &BTreeMap<Uuid, Text> {
        &self.texts
    }

    /// Adds or replaces a text, returns the previous one with the same
    /// UUID.
    pub fn insert_text(&mut self, text: Text) -> Option<Text> {
        self.texts.insert(text.uuid(), text)
    }

    /// Removes a text.
    pub fn remove_text(&mut self, uuid: &Uuid) -> Option<Text> {
        self.texts.remove(uuid)
    }

    /// Returns the transformation from symbol to schematic coordinates.
    pub fn transform(&self) -> Transform {
        Transform::from_obj(self)
    }

    /// Resolves the component instance, library component, gate and
    /// library symbol (upstream `SI_Symbol` constructor), with the upstream
    /// errors.
    pub fn resolve<'a>(&self, ctx: ProjectView<'a>) -> Result<ResolvedSymbol<'a>> {
        let component = ctx
            .circuit
            .component_instance(self.component)
            .ok_or(Error::InexistentComponent(self.component.0))?;
        let lib_component = ctx
            .library
            .component(&component.lib_component())
            .ok_or(Error::MissingLibraryComponent(component.lib_component()))?;
        let gate = lib_component
            .symbol_variants()
            .by_uuid(&component.lib_variant())
            .and_then(|v| v.symbol_items().by_uuid(&self.lib_gate))
            .ok_or(Error::InvalidSymbolItem(self.lib_gate))?;
        let lib_symbol = ctx
            .library
            .symbol(&gate.symbol_uuid())
            .ok_or(Error::MissingLibrarySymbol(gate.symbol_uuid()))?;
        Ok(ResolvedSymbol {
            component,
            lib_component,
            gate,
            lib_symbol,
        })
    }

    /// Returns the name shown in the schematic: the component name,
    /// followed by `-<suffix>` if the gate has a suffix (upstream
    /// `getName()`).
    pub fn name(&self, ctx: ProjectView<'_>) -> Result<String> {
        let r = self.resolve(ctx)?;
        let name = r.component.name().as_str();
        let suffix = r.gate.suffix().as_str();
        Ok(if suffix.is_empty() {
            name.to_owned()
        } else {
            format!("{name}-{suffix}")
        })
    }

    /// Returns the texts of the library symbol transformed to schematic
    /// coordinates, keeping their UUIDs (upstream `getDefaultTexts()`).
    pub fn default_texts(&self, lib_symbol: &Symbol) -> Vec<Text> {
        let transform = self.transform();
        lib_symbol
            .texts()
            .iter()
            .map(|text| {
                let mut text = text.clone();
                text.set_position(transform.map(&text.position()));
                text.set_rotation(transform.map_non_mirrorable(text.rotation()));
                if transform.mirrored {
                    text.set_align(text.align().mirrored_v());
                }
                text
            })
            .collect()
    }

    /// Returns the connected pins (pins mapped to a component signal; upstream
    /// hides the others), in the order of the library symbol.
    ///
    /// Fails like upstream's `SI_Symbol` constructor if the symbol cannot be
    /// resolved or the library symbol does not match the pin-signal-map of
    /// the gate.
    pub fn pins<'a>(&self, ctx: ProjectView<'a>) -> Result<Vec<SymbolPinView<'a>>> {
        let r = self.resolve(ctx)?;
        let map = r.gate.pin_signal_map();
        if r.lib_symbol.pins().len() != map.len() {
            return Err(Error::PinCountMismatch(self.uuid));
        }
        let transform = self.transform();
        let mut seen = BTreeSet::new();
        let mut pins = Vec::new();
        for lib_pin in r.lib_symbol.pins().iter() {
            let item = map.required_by_uuid(&lib_pin.uuid())?;
            let Some(signal_uuid) = item.signal_uuid() else {
                continue; // Hide pins which are not connected.
            };
            if !seen.insert(lib_pin.uuid()) {
                return Err(Error::DuplicateSymbolPin(lib_pin.uuid()));
            }
            let lib_signal = r
                .lib_component
                .signals()
                .by_uuid(&signal_uuid)
                .ok_or(Error::InexistentComponentSignal(signal_uuid))?;
            let net = r
                .component
                .signal(&signal_uuid)
                .ok_or(Error::InexistentComponentSignal(signal_uuid))?
                .net();
            let name = match item.display_type() {
                CmpSigPinDisplayType::None => String::new(),
                CmpSigPinDisplayType::PinName => lib_pin.name().to_string(),
                CmpSigPinDisplayType::ComponentSignal => lib_signal.name().to_string(),
                CmpSigPinDisplayType::NetSignal => net
                    .and_then(|n| ctx.circuit.net_signal(n))
                    .map(|n| n.name().to_string())
                    .unwrap_or_default(),
            };
            pins.push(SymbolPinView {
                symbol: self.id(),
                lib_pin,
                map_item: item,
                lib_signal,
                signal: ComponentSignalRef {
                    component: self.component,
                    signal: signal_uuid,
                },
                net,
                position: transform.map(&lib_pin.position()),
                rotation: transform.map_non_mirrorable(lib_pin.rotation()),
                name,
                component_signal_count: r.component.signals().len(),
            });
        }
        Ok(pins)
    }

    /// Returns the connected pin with the library pin UUID `pin` (upstream
    /// `getPin()`), `None` if there is no such (connected) pin.
    pub fn pin<'a>(&self, ctx: ProjectView<'a>, pin: Uuid) -> Result<Option<SymbolPinView<'a>>> {
        Ok(self.pins(ctx)?.into_iter().find(|p| p.uuid() == pin))
    }
}

impl HasUuid for SchematicSymbol {
    fn uuid(&self) -> Uuid {
        self.uuid
    }
}

impl HasTransform for SchematicSymbol {
    fn position(&self) -> Point {
        self.position
    }

    fn rotation(&self) -> Angle {
        self.rotation
    }

    fn mirrored(&self) -> bool {
        self.mirrored
    }
}

impl SerializeObject for SchematicSymbol {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        root.append_child("component", &self.component);
        root.ensure_line_break();
        root.append_child("lib_gate", &self.lib_gate);
        root.ensure_line_break();
        self.position.serialize(root.append_list("position"));
        root.append_child("rotation", &self.rotation);
        root.append_child("mirror", &self.mirrored);
        root.ensure_line_break();
        for text in self.texts.values() {
            root.ensure_line_break();
            text.serialize(root.append_list("text"));
        }
        root.ensure_line_break();
    }
}

/// The library and circuit objects a [`SchematicSymbol`] refers to.
#[derive(Debug, Clone, Copy)]
pub struct ResolvedSymbol<'a> {
    /// The component instance.
    pub component: &'a ComponentInstance,
    /// The library component of the instance.
    pub lib_component: &'a Component,
    /// The gate (symbol variant item).
    pub gate: &'a ComponentSymbolVariantItem,
    /// The library symbol of the gate.
    pub lib_symbol: &'a Symbol,
}

/// A connected pin of a placed symbol, computed from the library symbol,
/// the gate's pin-signal-map and the component instance (upstream
/// `SI_SymbolPin`).
#[derive(Debug, Clone)]
pub struct SymbolPinView<'a> {
    symbol: SymbolId,
    lib_pin: &'a SymbolPin,
    map_item: &'a ComponentPinSignalMapItem,
    lib_signal: &'a ComponentSignal,
    signal: ComponentSignalRef,
    net: Option<NetSignalId>,
    position: Point,
    rotation: Angle,
    name: String,
    component_signal_count: usize,
}

impl<'a> SymbolPinView<'a> {
    /// Returns the symbol.
    pub fn symbol(&self) -> SymbolId {
        self.symbol
    }

    /// Returns the UUID of the library pin.
    pub fn uuid(&self) -> Uuid {
        self.lib_pin.uuid()
    }

    /// Returns the library pin.
    pub fn lib_pin(&self) -> &'a SymbolPin {
        self.lib_pin
    }

    /// Returns the library component signal of the pin.
    pub fn lib_signal(&self) -> &'a ComponentSignal {
        self.lib_signal
    }

    /// Returns the component signal instance of the pin.
    pub fn signal(&self) -> ComponentSignalRef {
        self.signal
    }

    /// Returns the net of the component signal (upstream
    /// `getCompSigInstNetSignal()`).
    pub fn net(&self) -> Option<NetSignalId> {
        self.net
    }

    /// Returns the absolute position (schematic coordinates).
    pub fn position(&self) -> Point {
        self.position
    }

    /// Returns the absolute rotation.
    pub fn rotation(&self) -> Angle {
        self.rotation
    }

    /// Returns the displayed pin name (according to the display type of
    /// the pin-signal-map).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns what text is displayed at the pin.
    pub fn display_type(&self) -> CmpSigPinDisplayType {
        self.map_item.display_type()
    }

    /// Returns whether the component signal must be connected.
    pub fn is_required(&self) -> bool {
        self.lib_signal.is_required()
    }

    /// Returns the net line anchor of the pin.
    pub fn anchor(&self) -> NetLineAnchor {
        NetLineAnchor::Pin {
            symbol: self.symbol.0,
            pin: self.lib_pin.uuid(),
        }
    }

    /// Returns the position of the pin numbers text (relative to the pin).
    pub fn numbers_position(&self) -> Point {
        self.lib_pin
            .numbers_position(toolbox::is_text_upside_down(self.rotation))
    }

    /// Returns the alignment of the pin numbers text.
    pub fn numbers_alignment(&self) -> Alignment {
        SymbolPin::numbers_alignment(toolbox::is_text_upside_down(self.rotation))
    }

    /// Returns the (truncated) pin numbers text for the given pad names of
    /// the component signal (upstream `updateNumbers()`): hidden if it
    /// equals the displayed pin name or the component has only one signal,
    /// otherwise comma separated and truncated with "…" at 8 characters.
    pub fn numbers_text<S: AsRef<str>>(&self, numbers: &[S]) -> String {
        let mut result = String::new();
        let shows_name = matches!(
            self.display_type(),
            CmpSigPinDisplayType::PinName | CmpSigPinDisplayType::ComponentSignal
        );
        if shows_name && numbers.len() == 1 && numbers[0].as_ref() == self.name {
            return result;
        }
        if self.component_signal_count == 1 {
            return result;
        }
        for number in numbers {
            let number = if result.is_empty() {
                number.as_ref().to_owned()
            } else {
                format!(",{}", number.as_ref())
            };
            if result.chars().count() + number.chars().count() < 8 {
                result += &number;
            } else {
                result += "…";
                break;
            }
        }
        result
    }
}
