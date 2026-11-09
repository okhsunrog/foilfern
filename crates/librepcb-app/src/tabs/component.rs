//! The component editor tab.
//!
//! Port of libs/librepcb/editor/library/cmp/componenttab.{h,cpp} with the
//! list models of the tab (libs/librepcb/editor/library/cmp/
//! {componentsignallistmodel,componentsignalnamelistmodel,
//! componentvariantlistmodel,componentvarianteditor,componentgatelistmodel,
//! componentgateeditor,componentpinoutlistmodel}.{h,cpp}): the metadata
//! with datasheet, schematic-only, prefix, default value and attributes, the
//! signals, the symbol variants with their gates (symbol, position,
//! rotation, suffix, required, pinout) and the symbol previews, the checks
//! with approvals and automatic fixes, undo/redo and saving (see
//! [`ElementCore`]).
//!
//! Differences to upstream: the symbol previews are images rendered by
//! `librepcb-scene` (pins are labeled with the connected signal names,
//! texts are shown raw); the symbols are chosen in a list with a search
//! field (see [`ElementChooserDialog`](crate::dialogs::chooser)).

use std::collections::HashMap;
use std::rc::Rc;

use chrono::Utc;
use librepcb_app_ui as ui;
use librepcb_core::attribute::AttributeList;
use librepcb_core::library::cmp::{CmpSigPinDisplayType, Component, ComponentPrefix};
use librepcb_core::library::sym::Symbol;
use librepcb_core::library::{BaseMetadata, LibraryBaseElement, LibraryElement, Resource};
use librepcb_core::types::{
    Angle, CircuitIdentifier, ElementName, LengthUnit, Point, Uuid, Version,
};
use librepcb_core::utils::toolbox;
use librepcb_core::workspace::{ElementKind, LibraryDb};
use librepcb_editor::library_editor::commands::{
    AddComponentSignals, AddGate, AddSymbolVariant, AutoConnectPins, EditComponent,
    EditComponentSignal, EditElementMetadata, EditGate, EditSymbolVariant, MoveGateUp,
    MoveSymbolVariantUp, RemoveComponentSignal, RemoveGate, RemoveSymbolVariant,
    SetDefaultSymbolVariant, SetGateSymbol, SetPinSignal, Transformable, clean_forced_net_name,
    has_unassigned_signals,
};
use librepcb_i18n::tr;
use librepcb_scene::{ColorScheme, RenderOptions, RenderSize, SymbolScene, render_scene};

use super::editing::error_notification;
use super::element_core::ElementCore;
use super::{TabId, TabRequest, TabUpdate};
use crate::dialogs::attributes::AttributeEditor;
use crate::dialogs::chooser::ChooserPurpose;
use crate::helpers::{length_from_ui, length_to_ui, unit_to_ui};
use crate::models::{UiModel, model_rc, vec_model};
use crate::validation;

/// The page of the symbol variants.
const VARIANTS_PAGE: i32 = 2;

/// A row of one of the tab's list models written by the UI.
#[derive(Debug, Clone)]
pub enum ComponentRowEvent {
    /// A signal row (name, forced net name, required, delete).
    Signal(usize, ui::ComponentSignalData),
    /// A variant row (name, description, norm, actions).
    Variant(usize, ui::ComponentVariantData),
    /// A gate row of a variant (position, rotation, suffix, required,
    /// actions).
    Gate {
        /// Variant row.
        variant: usize,
        /// Gate row.
        gate: usize,
        /// The data.
        data: ui::ComponentGateData,
    },
    /// A pinout row of a gate (signal, display mode).
    Pinout {
        /// Variant row.
        variant: usize,
        /// Gate row.
        gate: usize,
        /// Pin row.
        pin: usize,
        /// The data.
        data: ui::ComponentPinoutData,
    },
}

/// Receives the rows written by the UI (deferred to the application).
pub type ComponentRowSink = Rc<dyn Fn(ComponentRowEvent)>;

/// Loads the latest version of a library element from the workspace
/// libraries.
pub fn load_element<E: LibraryElement>(db: &LibraryDb, kind: ElementKind, uuid: Uuid) -> Option<E> {
    let dir = db.latest(kind, uuid).ok().flatten()?;
    let fs = librepcb_core::fileio::TransactionalFileSystem::open_ro(&dir).ok()?;
    let dir = librepcb_core::fileio::TransactionalDirectory::new(std::sync::Arc::new(fs), "");
    E::open(dir).ok()
}

fn display_mode_to_ui(t: CmpSigPinDisplayType) -> ui::ComponentPinoutDisplayMode {
    match t {
        CmpSigPinDisplayType::None => ui::ComponentPinoutDisplayMode::None,
        CmpSigPinDisplayType::PinName => ui::ComponentPinoutDisplayMode::PinName,
        CmpSigPinDisplayType::ComponentSignal => ui::ComponentPinoutDisplayMode::SignalName,
        CmpSigPinDisplayType::NetSignal => ui::ComponentPinoutDisplayMode::NetName,
    }
}

fn display_mode_from_ui(m: ui::ComponentPinoutDisplayMode) -> CmpSigPinDisplayType {
    match m {
        ui::ComponentPinoutDisplayMode::None => CmpSigPinDisplayType::None,
        ui::ComponentPinoutDisplayMode::PinName => CmpSigPinDisplayType::PinName,
        ui::ComponentPinoutDisplayMode::SignalName => CmpSigPinDisplayType::ComponentSignal,
        ui::ComponentPinoutDisplayMode::NetName => CmpSigPinDisplayType::NetSignal,
    }
}

/// Upstream `validateComponentPrefix()`.
fn validate_prefix(input: &str, error: &mut String) -> Option<ComponentPrefix> {
    let value = ComponentPrefix::new(ComponentPrefix::clean(input)).ok();
    if value.is_some() && !input.trim().is_empty() {
        error.clear();
    } else if !input.trim().is_empty() {
        *error = tr!("SlintHelpers", "Invalid");
    } else {
        *error = tr!("SlintHelpers", "Recommended");
    }
    value
}

/// Upstream `validateComponentDefaultValue()`.
fn validate_default_value(input: &str, error: &mut String) {
    if input.trim().is_empty() {
        *error = tr!("SlintHelpers", "Recommended");
    } else {
        error.clear();
    }
}

/// The component editor tab (upstream `ComponentTab`).
pub struct ComponentTab {
    id: TabId,
    core: ElementCore<Component>,
    library_index: i32,
    sink: ComponentRowSink,
    // Component properties (committed with "apply").
    datasheet_url: String,
    datasheet_url_error: String,
    schematic_only: bool,
    prefix: String,
    prefix_error: String,
    prefix_parsed: Option<ComponentPrefix>,
    default_value: String,
    default_value_error: String,
    attributes: Rc<std::cell::RefCell<AttributeEditor>>,
    // Signals.
    signals: Rc<UiModel<ui::ComponentSignalData>>,
    signal_rows: Vec<Uuid>,
    signals_key: Vec<(Uuid, String, String, bool)>,
    new_signal_name: String,
    new_signal_name_error: String,
    /// Signals of the pinout combo boxes (first: unconnected).
    signal_names: Vec<(Option<Uuid>, String)>,
    // Variants.
    variants: Rc<UiModel<ui::ComponentVariantData>>,
    variants_state: Option<u64>,
    /// Symbols of the gates (loaded from the workspace libraries).
    symbols: HashMap<Uuid, Option<Symbol>>,
    /// Previews by scene index (variant * 1000 + gate).
    previews: HashMap<i32, Option<SymbolScene>>,
    font: Option<librepcb_core::font::StrokeFont>,
    frame: i32,
    pending_errors: Vec<String>,
    pending_requests: Vec<TabRequest>,
}

impl ComponentTab {
    /// Creates the tab for an element core; `sink` receives the rows of
    /// the list models written by the UI.
    pub fn new(core: ElementCore<Component>, sink: ComponentRowSink) -> Self {
        let signals = UiModel::shared(Vec::new());
        let s = Rc::clone(&sink);
        signals.set_handler(move |row, data| s(ComponentRowEvent::Signal(row, data)));
        let variants = UiModel::shared(Vec::new());
        let s = Rc::clone(&sink);
        variants.set_handler(move |row, data| s(ComponentRowEvent::Variant(row, data)));
        let attributes = AttributeEditor::new(core.editor.element().attributes());
        let mut tab = Self {
            id: TabId::new(),
            core,
            library_index: -1,
            sink,
            datasheet_url: String::new(),
            datasheet_url_error: String::new(),
            schematic_only: false,
            prefix: String::new(),
            prefix_error: String::new(),
            prefix_parsed: None,
            default_value: String::new(),
            default_value_error: String::new(),
            attributes,
            signals,
            signal_rows: Vec::new(),
            signals_key: Vec::new(),
            new_signal_name: String::new(),
            new_signal_name_error: String::new(),
            signal_names: Vec::new(),
            variants,
            variants_state: None,
            symbols: HashMap::new(),
            previews: HashMap::new(),
            font: librepcb_scene::default_stroke_font(),
            frame: 0,
            pending_errors: Vec::new(),
            pending_requests: Vec::new(),
        };
        tab.refresh();
        tab
    }

    /// Sets the tab identifier (to connect models before the tab exists).
    pub fn with_id(mut self, id: TabId) -> Self {
        self.id = id;
        self
    }

    /// The unique tab identifier.
    pub fn id(&self) -> TabId {
        self.id
    }

    /// The shared element state.
    pub fn core(&self) -> &ElementCore<Component> {
        &self.core
    }

    /// The shared element state, mutably.
    pub fn core_mut(&mut self) -> &mut ElementCore<Component> {
        &mut self.core
    }

    /// Sets the index of the library in `Data.libraries`.
    pub fn set_library_index(&mut self, index: i32) {
        self.library_index = index;
    }

    fn component(&self) -> &Component {
        self.core.editor.element()
    }

    /// The symbol of a gate (cached).
    fn symbol(&mut self, uuid: Uuid) -> Option<&Symbol> {
        let db = std::sync::Arc::clone(&self.core.db);
        self.symbols
            .entry(uuid)
            .or_insert_with(|| load_element::<Symbol>(&db, ElementKind::Symbol, uuid))
            .as_ref()
    }

    fn load_gate_symbols(&mut self) {
        let uuids: Vec<Uuid> = self
            .component()
            .symbol_variants()
            .iter()
            .flat_map(|v| v.symbol_items().iter().map(|g| g.symbol_uuid()))
            .collect();
        for uuid in uuids {
            self.symbol(uuid);
        }
    }

    /// Updates the UI data from the component (upstream `refreshUiData()`
    /// and the list models).
    fn refresh(&mut self) {
        self.core.refresh();
        let cmp = self.core.editor.element();
        self.schematic_only = cmp.schematic_only();
        self.prefix = cmp.prefixes().default_value().to_string();
        self.prefix_parsed = validate_prefix(&self.prefix, &mut self.prefix_error);
        self.default_value = cmp.default_value().clone();
        validate_default_value(&self.default_value, &mut self.default_value_error);
        self.datasheet_url = cmp
            .element_metadata()
            .resources()
            .iter()
            .next()
            .map(|r| r.url().clone())
            .unwrap_or_default();
        self.datasheet_url_error.clear();
        if self.attributes.borrow().list().ok().as_ref() != Some(cmp.attributes()) {
            self.attributes = AttributeEditor::new(cmp.attributes());
        }
        self.refresh_signals();
        self.refresh_variants();
    }

    fn refresh_signals(&mut self) {
        let cmp = self.core.editor.element();
        let key: Vec<(Uuid, String, String, bool)> = cmp
            .signals()
            .iter()
            .map(|s| {
                (
                    s.uuid(),
                    s.name().to_string(),
                    s.forced_net_name().clone(),
                    s.is_required(),
                )
            })
            .collect();
        // Sorted by name (upstream `mSignalsSorted`, numbers naturally).
        let mut sorted = key.clone();
        sorted.sort_by(|a, b| crate::dialogs::natural_cmp(&a.1, &b.1));
        self.signal_names = std::iter::once((
            None,
            format!(
                "({})",
                tr!(
                    "librepcb::editor::ComponentSignalNameListModel",
                    "unconnected"
                )
                .to_lowercase()
            ),
        ))
        .chain(sorted.iter().map(|s| (Some(s.0), s.1.clone())))
        .collect();
        if key != self.signals_key || self.signals.len() != key.len() {
            self.signal_rows = sorted.iter().map(|s| s.0).collect();
            self.signals.replace_all(
                sorted
                    .iter()
                    .enumerate()
                    .map(
                        |(i, (uuid, name, forced, required))| ui::ComponentSignalData {
                            id: uuid.to_string()[..8].into(),
                            name: name.as_str().into(),
                            name_error: Default::default(),
                            forced_net_name: forced.as_str().into(),
                            required: *required,
                            sort_index: i as i32,
                            delete: false,
                        },
                    )
                    .collect(),
            );
            self.signals_key = key;
        }
    }

    fn refresh_variants(&mut self) {
        let state = self.core.editor.state_id();
        if self.variants_state == Some(state) && !self.variants.is_empty() {
            return;
        }
        self.variants_state = Some(state);
        self.previews.clear();
        self.frame = self.frame.wrapping_add(1);
        self.load_gate_symbols();
        let cmp = self.core.editor.element();
        let mut rows = Vec::new();
        for (vi, variant) in cmp.symbol_variants().iter().enumerate() {
            let mut gates = Vec::new();
            for (gi, gate) in variant.symbol_items().iter().enumerate() {
                let symbol = self
                    .symbols
                    .get(&gate.symbol_uuid())
                    .and_then(Option::as_ref);
                let pinout_rows: Vec<ui::ComponentPinoutData> = gate
                    .pin_signal_map()
                    .iter()
                    .map(|item| ui::ComponentPinoutData {
                        pin_name: symbol
                            .and_then(|s| s.pins().by_uuid(&item.pin_uuid()))
                            .map_or_else(
                                || item.pin_uuid().to_string()[..8].to_owned(),
                                |p| p.name().to_string(),
                            )
                            .into(),
                        signal_index: self
                            .signal_names
                            .iter()
                            .position(|(u, _)| *u == item.signal_uuid())
                            .map_or(-1, |i| i as i32),
                        display_mode: display_mode_to_ui(item.display_type()),
                    })
                    .collect();
                let pinout = UiModel::shared(pinout_rows);
                let sink = Rc::clone(&self.sink);
                pinout.set_handler(move |pin, data| {
                    sink(ComponentRowEvent::Pinout {
                        variant: vi,
                        gate: gi,
                        pin,
                        data,
                    })
                });
                gates.push(ui::ComponentGateData {
                    id: gate.uuid().to_string()[..8].into(),
                    symbol_name: symbol
                        .map_or_else(
                            || gate.symbol_uuid().to_string(),
                            |s| s.metadata().name().to_string(),
                        )
                        .into(),
                    symbol_x: length_to_ui(gate.symbol_position().x),
                    symbol_y: length_to_ui(gate.symbol_position().y),
                    symbol_rotation: gate.symbol_rotation().to_micro_deg(),
                    required: gate.is_required(),
                    suffix: gate.suffix().to_string().into(),
                    pinout: model_rc(&pinout),
                    action: ui::ComponentGateAction::None,
                    frame_index: self.frame,
                });
            }
            let gates_model = UiModel::shared(gates);
            let sink = Rc::clone(&self.sink);
            gates_model.set_handler(move |gate, data| {
                sink(ComponentRowEvent::Gate {
                    variant: vi,
                    gate,
                    data,
                })
            });
            rows.push(ui::ComponentVariantData {
                id: variant.uuid().to_string()[..8].into(),
                name: variant.name().to_string().into(),
                description: variant.descriptions().default_value().as_str().into(),
                norm: variant.norm().as_str().into(),
                gates: model_rc(&gates_model),
                has_unassigned_signals: has_unassigned_signals(cmp, variant.uuid()),
                action: ui::ComponentVariantAction::None,
                frame_index: self.frame,
            });
        }
        self.variants.replace_all(rows);
    }

    /// Executes a component command, collecting errors.
    fn execute<C>(&mut self, cmd: C) -> Option<C::Output>
    where
        C: librepcb_editor::library_editor::ElementCommand<Component>,
    {
        match self.core.editor.execute(cmd) {
            Ok(output) => Some(output),
            Err(e) => {
                self.pending_errors.push(e.to_string());
                None
            }
        }
    }

    /// Updates after modifications (lists, checks) with the errors and
    /// requests collected since the last update.
    fn finish(&mut self) -> TabUpdate {
        self.refresh_signals_if_changed();
        self.refresh_variants();
        self.core.run_checks_if_modified();
        let mut update = TabUpdate {
            data_changed: true,
            repaint: true,
            ..TabUpdate::default()
        };
        for e in std::mem::take(&mut self.pending_errors) {
            update.requests.push(error_notification(e));
        }
        update
            .requests
            .extend(std::mem::take(&mut self.pending_requests));
        update
    }

    fn refresh_signals_if_changed(&mut self) {
        let state = self.core.editor.state_id();
        if self.variants_state != Some(state) {
            self.core.refresh();
            self.refresh_signals();
        }
    }

    // --- Lists ---

    /// A row of a list model was written by the UI.
    pub fn row_written(&mut self, event: ComponentRowEvent) -> TabUpdate {
        if !self.core.is_writable() {
            self.signals_key.clear();
            self.variants_state = None;
            return self.finish();
        }
        match event {
            ComponentRowEvent::Signal(row, data) => self.signal_row_written(row, data),
            ComponentRowEvent::Variant(row, data) => self.variant_row_written(row, data),
            ComponentRowEvent::Gate {
                variant,
                gate,
                data,
            } => self.gate_row_written(variant, gate, data),
            ComponentRowEvent::Pinout {
                variant,
                gate,
                pin,
                data,
            } => self.pinout_row_written(variant, gate, pin, data),
        }
        self.finish()
    }

    fn signal_row_written(&mut self, row: usize, data: ui::ComponentSignalData) {
        let Some(uuid) = self.signal_rows.get(row).copied() else {
            return;
        };
        if data.delete {
            self.execute(RemoveComponentSignal { signal: uuid });
            return;
        }
        // Keep the edited values until "apply" (upstream
        // `ComponentSignalListModel::set_row_data()`).
        let cmp = self.component();
        let Some(sig) = cmp.signals().by_uuid(&uuid) else {
            return;
        };
        let name = data.name.to_string();
        let duplicate = name != **sig.name()
            && cmp
                .signals()
                .contains_name(&CircuitIdentifier::clean(&name));
        let mut error = String::new();
        validation::circuit_identifier(&name, &mut error, duplicate);
        self.signals.set(
            row,
            ui::ComponentSignalData {
                name_error: error.into(),
                delete: false,
                ..data
            },
        );
    }

    fn variant_uuid(&self, row: usize) -> Option<Uuid> {
        self.component()
            .symbol_variants()
            .iter()
            .nth(row)
            .map(|v| v.uuid())
    }

    fn variant_row_written(&mut self, row: usize, data: ui::ComponentVariantData) {
        let Some(variant) = self.variant_uuid(row) else {
            return;
        };
        match data.action {
            ui::ComponentVariantAction::MoveUp => {
                self.execute(MoveSymbolVariantUp { variant });
            }
            ui::ComponentVariantAction::SetAsDefault => {
                self.execute(SetDefaultSymbolVariant { variant });
            }
            ui::ComponentVariantAction::Delete => {
                self.execute(RemoveSymbolVariant { variant });
            }
            ui::ComponentVariantAction::AddGate => {
                self.pending_requests
                    .push(TabRequest::ChooseElement(ChooserPurpose::AddGate {
                        variant,
                    }));
            }
            ui::ComponentVariantAction::AutoConnectPins => {
                self.load_gate_symbols();
                let symbols: HashMap<Uuid, &Symbol> = self
                    .symbols
                    .iter()
                    .filter_map(|(u, s)| s.as_ref().map(|s| (*u, s)))
                    .collect();
                let result = self.core.editor.execute(AutoConnectPins {
                    variant,
                    symbols: &symbols,
                });
                if let Err(e) = result {
                    self.pending_errors.push(e.to_string());
                }
            }
            ui::ComponentVariantAction::None => {
                let cmp = self.component();
                let Some(v) = cmp.symbol_variants().by_uuid(&variant) else {
                    return;
                };
                let mut cmd = EditSymbolVariant {
                    variant,
                    norm: None,
                    names: None,
                    descriptions: None,
                };
                let name = data.name.to_string();
                let mut duplicate = false;
                if name != **v.name()
                    && let Ok(parsed) = ElementName::new(ElementName::clean(&name))
                {
                    if cmp
                        .symbol_variants()
                        .iter()
                        .any(|o| o.uuid() != variant && *o.name() == parsed)
                    {
                        duplicate = true;
                    } else {
                        let mut names = v.names().clone();
                        names.set_default_value(parsed);
                        cmd.names = Some(names);
                    }
                }
                if data.description.as_str() != v.descriptions().default_value() {
                    let mut descriptions = v.descriptions().clone();
                    descriptions.set_default_value(data.description.trim().to_owned());
                    cmd.descriptions = Some(descriptions);
                }
                if data.norm.as_str() != v.norm() {
                    cmd.norm = Some(data.norm.trim().to_owned());
                }
                if duplicate {
                    self.pending_errors.push(tr!(
                        "librepcb::editor::ComponentVariantEditor",
                        "There is already a variant with the name \"{0}\".",
                        name
                    ));
                }
                if cmd.names.is_some() || cmd.descriptions.is_some() || cmd.norm.is_some() {
                    self.execute(cmd);
                } else {
                    self.variants_state = None;
                }
            }
        }
    }

    fn gate_uuids(&self, variant: usize, gate: usize) -> Option<(Uuid, Uuid)> {
        let v = self.component().symbol_variants().iter().nth(variant)?;
        let g = v.symbol_items().iter().nth(gate)?;
        Some((v.uuid(), g.uuid()))
    }

    fn gate_row_written(&mut self, variant: usize, gate: usize, data: ui::ComponentGateData) {
        let Some((variant, gate)) = self.gate_uuids(variant, gate) else {
            return;
        };
        match data.action {
            ui::ComponentGateAction::MoveUp => {
                self.execute(MoveGateUp { variant, gate });
            }
            ui::ComponentGateAction::Delete => {
                let remove_signals = self.core.wizard_mode;
                self.execute(RemoveGate {
                    variant,
                    gate,
                    remove_signals,
                });
            }
            ui::ComponentGateAction::ChooseSymbol => {
                self.pending_requests
                    .push(TabRequest::ChooseElement(ChooserPurpose::GateSymbol {
                        variant,
                        gate,
                    }));
            }
            ui::ComponentGateAction::None => {
                let Some(g) = self
                    .component()
                    .symbol_variants()
                    .by_uuid(&variant)
                    .and_then(|v| v.symbol_items().by_uuid(&gate))
                else {
                    return;
                };
                let position = Point::new(
                    length_from_ui(data.symbol_x.clone()),
                    length_from_ui(data.symbol_y.clone()),
                );
                let rotation = Angle::new(data.symbol_rotation);
                let suffix = (data.suffix.as_str() != g.suffix().as_str()).then(|| {
                    librepcb_core::library::cmp::ComponentSymbolVariantItemSuffix::new(
                        librepcb_core::library::cmp::ComponentSymbolVariantItemSuffix::clean(
                            &data.suffix,
                        ),
                    )
                    .unwrap_or_default()
                });
                let cmd = EditGate {
                    variant,
                    gate,
                    position: (position != g.symbol_position()).then_some(position),
                    rotation: (rotation != g.symbol_rotation()).then_some(rotation),
                    required: (data.required != g.is_required()).then_some(data.required),
                    suffix,
                };
                if cmd.position.is_some()
                    || cmd.rotation.is_some()
                    || cmd.required.is_some()
                    || cmd.suffix.is_some()
                {
                    self.execute(cmd);
                } else {
                    self.variants_state = None;
                }
            }
        }
    }

    fn pinout_row_written(
        &mut self,
        variant: usize,
        gate: usize,
        pin: usize,
        data: ui::ComponentPinoutData,
    ) {
        let Some((variant, gate)) = self.gate_uuids(variant, gate) else {
            return;
        };
        let Some(item) = self
            .component()
            .symbol_variants()
            .by_uuid(&variant)
            .and_then(|v| v.symbol_items().by_uuid(&gate))
            .and_then(|g| g.pin_signal_map().iter().nth(pin))
        else {
            return;
        };
        let signal = usize::try_from(data.signal_index)
            .ok()
            .and_then(|i| self.signal_names.get(i))
            .map(|(u, _)| *u);
        let display = display_mode_from_ui(data.display_mode);
        let cmd = SetPinSignal {
            variant,
            gate,
            pin: item.pin_uuid(),
            signal: signal.filter(|s| *s != item.signal_uuid()),
            display_type: (display != item.display_type()).then_some(display),
        };
        if cmd.signal.is_some() || cmd.display_type.is_some() {
            self.execute(cmd);
        } else {
            self.variants_state = None;
        }
    }

    /// A symbol was chosen for `purpose` (chooser dialog).
    pub fn element_chosen(&mut self, purpose: ChooserPurpose, uuid: Uuid) -> TabUpdate {
        self.symbols.remove(&uuid);
        let db = std::sync::Arc::clone(&self.core.db);
        let Some(symbol) = load_element::<Symbol>(&db, ElementKind::Symbol, uuid) else {
            self.pending_errors.push(tr!(
                "librepcb::editor::SymbolChooserDialog",
                "Could not load symbol"
            ));
            return self.finish();
        };
        match purpose {
            ChooserPurpose::AddVariant => {
                self.execute(AddSymbolVariant { symbol: &symbol });
            }
            ChooserPurpose::AddGate { variant } => {
                let create_signals = self.core.wizard_mode;
                self.execute(AddGate {
                    variant,
                    symbol: &symbol,
                    create_signals,
                });
            }
            ChooserPurpose::GateSymbol { variant, gate } => {
                self.execute(SetGateSymbol {
                    variant,
                    gate,
                    symbol: &symbol,
                });
            }
            ChooserPurpose::DeviceComponent | ChooserPurpose::DevicePackage => {}
        }
        self.symbols.insert(uuid, Some(symbol));
        self.variants_state = None;
        self.finish()
    }

    /// Applies the edited properties (upstream `commitUiData()`).
    fn commit(&mut self) -> Vec<TabRequest> {
        if self.core.editor.is_group_active() {
            return Vec::new();
        }
        let mut requests: Vec<TabRequest> = self.core.commit_metadata().into_iter().collect();
        let cmp = self.core.editor.element();
        // Datasheet (the first resource).
        let mut resources = cmp.element_metadata().resources().clone();
        let url = self.datasheet_url.trim().to_owned();
        let name = ElementName::new(ElementName::clean(&format!(
            "Datasheet {}",
            self.core.meta.name.trim()
        )))
        .ok();
        let valid_url = validation::parse_user_url(&url);
        let first = resources.iter().next().cloned();
        let mut resources_changed = false;
        match (valid_url, first) {
            (Some(u), None) => {
                if let Some(name) = name {
                    resources.push(Resource::new(name, "application/pdf", u.to_string()));
                    resources_changed = true;
                }
            }
            (None, Some(_)) => {
                resources.take(0);
                resources_changed = true;
            }
            (Some(u), Some(res)) if url != *res.url() => {
                if let Some(r) = resources.iter_mut().next() {
                    if let Some(name) = name {
                        r.set_name(name);
                    }
                    r.set_url(u.to_string());
                    resources_changed = true;
                }
            }
            _ => {}
        }
        if resources_changed {
            self.execute(EditElementMetadata {
                resources: Some(resources),
                ..EditElementMetadata::default()
            });
        }
        let cmp = self.core.editor.element();
        let mut cmd = EditComponent::default();
        if self.schematic_only != cmp.schematic_only() {
            cmd.schematic_only = Some(self.schematic_only);
        }
        if let Some(prefix) = &self.prefix_parsed
            && prefix != cmp.prefixes().default_value()
        {
            let mut prefixes = cmp.prefixes().clone();
            prefixes.set_default_value(prefix.clone());
            cmd.prefixes = Some(prefixes);
        }
        if self.default_value != *cmp.default_value() {
            cmd.default_value = Some(self.default_value.trim().to_owned());
        }
        let attributes = self.attributes.borrow().list();
        match attributes {
            Ok(list) if list != *cmp.attributes() => cmd.attributes = Some(list),
            Ok(_) => {}
            Err(e) => self.pending_errors.push(e),
        }
        if cmd != EditComponent::default() {
            self.execute(cmd);
        }
        let cmp = self.core.editor.element();
        // Signals (upstream `ComponentSignalListModel::apply()`).
        let mut edits = Vec::new();
        for (row, uuid) in self.signal_rows.iter().enumerate() {
            let (Some(data), Some(sig)) = (self.signals.get(row), cmp.signals().by_uuid(uuid))
            else {
                continue;
            };
            let name = (data.name.as_str() != sig.name().as_str() && data.name_error.is_empty())
                .then(|| CircuitIdentifier::new(CircuitIdentifier::clean(&data.name)).ok())
                .flatten();
            let forced = (data.forced_net_name.as_str() != sig.forced_net_name())
                .then(|| clean_forced_net_name(&data.forced_net_name));
            let required = (data.required != sig.is_required()).then_some(data.required);
            if name.is_some() || forced.is_some() || required.is_some() {
                edits.push(EditComponentSignal {
                    signal: *uuid,
                    name,
                    role: None,
                    forced_net_name: forced,
                    required,
                    negated: None,
                    clock: None,
                });
            }
        }
        for edit in edits {
            self.execute(edit);
        }
        self.signals_key.clear();
        self.variants_state = None;
        self.refresh();
        self.core.run_checks_if_modified();
        requests.extend(
            std::mem::take(&mut self.pending_errors)
                .into_iter()
                .map(error_notification),
        );
        requests
    }

    // --- UI data ---

    /// The `TabData`.
    pub fn ui_data(&self) -> ui::TabData {
        self.core.tab_data()
    }

    /// The `ComponentTabData`.
    pub fn derived_ui_data(&self) -> ui::ComponentTabData {
        let m = &self.core.meta;
        ui::ComponentTabData {
            library_index: self.library_index,
            path: self.core.directory_path().as_str().into(),
            wizard_mode: self.core.wizard_mode,
            page_index: self.core.page_index,
            name: m.name.as_str().into(),
            name_error: m.name_error.as_str().into(),
            description: m.description.as_str().into(),
            keywords: m.keywords.as_str().into(),
            author: m.author.as_str().into(),
            age_days: m.age_days,
            version: m.version.as_str().into(),
            version_error: m.version_error.as_str().into(),
            deprecated: m.deprecated,
            categories: m.categories_model(),
            categories_tree: m.categories_tree(),
            choose_category: m.choose_category,
            datasheet_url: self.datasheet_url.as_str().into(),
            datasheet_url_error: self.datasheet_url_error.as_str().into(),
            schematic_only: self.schematic_only,
            prefix: self.prefix.as_str().into(),
            prefix_error: self.prefix_error.as_str().into(),
            default_value: self.default_value.as_str().into(),
            default_value_error: self.default_value_error.as_str().into(),
            attributes: self.attributes.borrow().model_rc(),
            cmp_signals: model_rc(&self.signals),
            new_signal_name: self.new_signal_name.as_str().into(),
            new_signal_name_error: self.new_signal_name_error.as_str().into(),
            signal_names: vec_model(
                self.signal_names
                    .iter()
                    .map(|(_, n)| n.as_str().into())
                    .collect(),
            ),
            variants: model_rc(&self.variants),
            checks: self.core.checks_data(),
            unit: unit_to_ui(LengthUnit::Millimeters),
            interface_broken_msg: self.core.is_interface_broken(),
            element_duplicated_msg: self.core.element_duplicated && !m.deprecated,
            new_category: Default::default(),
        }
    }

    /// Applies data written by the UI (upstream `setDerivedUiData()`).
    pub fn set_derived_ui_data(&mut self, data: &ui::ComponentTabData) -> TabUpdate {
        let mut update = TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        };
        self.core.page_index = data.page_index;
        let category_added = self.core.meta.set_from_ui(
            &data.name,
            &data.description,
            &data.keywords,
            &data.author,
            &data.version,
            data.deprecated,
            &data.new_category,
            data.choose_category,
        );
        if category_added {
            update.requests.extend(self.core.commit_metadata());
        }
        self.datasheet_url = data.datasheet_url.to_string();
        validation::url(&self.datasheet_url, &mut self.datasheet_url_error, true);
        self.schematic_only = data.schematic_only;
        self.prefix = data.prefix.to_string();
        if let Some(p) = validate_prefix(&self.prefix, &mut self.prefix_error) {
            self.prefix_parsed = Some(p);
        }
        self.default_value = data.default_value.to_string();
        validate_default_value(&self.default_value, &mut self.default_value_error);
        if data.new_signal_name.as_str() != self.new_signal_name {
            self.new_signal_name = data.new_signal_name.to_string();
            let names = toolbox::expand_ranges_in_string(&self.new_signal_name);
            let cmp = self.component();
            let duplicate = names
                .iter()
                .any(|n| cmp.signals().contains_name(&CircuitIdentifier::clean(n)));
            if self.new_signal_name.trim().is_empty() {
                self.new_signal_name_error.clear();
            } else {
                let first = names.first().cloned().unwrap_or_default();
                validation::circuit_identifier(&first, &mut self.new_signal_name_error, duplicate);
            }
        }
        if data.deprecated || !data.element_duplicated_msg {
            self.core.element_duplicated = false;
        }
        update
    }

    /// A category row was written by the UI (removal).
    pub fn category_row_written(
        &mut self,
        row: usize,
        data: &ui::LibraryElementCategoryData,
    ) -> TabUpdate {
        let mut update = TabUpdate::default();
        if self.core.meta.category_row_written(row, data) {
            update.requests.extend(self.core.commit_metadata());
            update.data_changed = true;
        }
        update
    }

    /// A check message row was written by the UI.
    pub fn check_row_written(&mut self, row: usize, data: &ui::RuleCheckMessageData) -> TabUpdate {
        use librepcb_editor::library_editor::checks::FixRequest;
        let (request, mut update) = self.core.check_row_written(row, data);
        match request {
            Some(FixRequest::ChooseCategories) => {
                self.core.page_index = 0;
                self.core.meta.choose_category = true;
            }
            Some(_) => self.core.page_index = VARIANTS_PAGE,
            None => {}
        }
        self.refresh();
        let after = self.finish();
        update.requests.extend(after.requests);
        update
    }

    // --- Actions ---

    /// Handles a tab action (upstream `ComponentTab::trigger()`).
    pub fn trigger(&mut self, action: ui::TabAction) -> TabUpdate {
        use ui::TabAction as A;
        let mut requests = Vec::new();
        if matches!(
            action,
            A::Apply | A::Save | A::Undo | A::Redo | A::Next | A::SaveAs
        ) {
            requests.extend(self.commit());
        }
        if matches!(action, A::Back) && self.core.wizard_mode && self.core.page_index > 0 {
            self.core.page_index -= 1;
            return TabUpdate {
                data_changed: true,
                ..TabUpdate::default()
            };
        }
        if let Some(mut update) = self.core.trigger_common(action) {
            self.signals_key.clear();
            self.variants_state = None;
            self.refresh();
            let after = self.finish();
            requests.extend(update.requests);
            update.requests = requests;
            update.requests.extend(after.requests);
            return update;
        }
        match action {
            A::ComponentAddSignals => {
                let names = if self.new_signal_name.trim().is_empty() {
                    // Upstream: the next free number is not proposed for
                    // signals; nothing to add.
                    return TabUpdate::default();
                } else {
                    self.new_signal_name.clone()
                };
                if self.execute(AddComponentSignals { names }).is_some() {
                    self.new_signal_name.clear();
                    self.new_signal_name_error.clear();
                }
            }
            A::ComponentAddVariant => {
                self.pending_requests
                    .push(TabRequest::ChooseElement(ChooserPurpose::AddVariant));
            }
            A::OpenDatasheet => {
                if let Some(url) = validation::parse_user_url(&self.datasheet_url) {
                    self.pending_requests
                        .push(TabRequest::OpenUrl(url.to_string()));
                }
            }
            A::SaveAs => {
                self.pending_requests
                    .push(TabRequest::DuplicateLibraryElement);
            }
            _ => return TabUpdate::default(),
        }
        let mut update = self.finish();
        requests.extend(update.requests);
        update.requests = requests;
        update
    }

    // --- Symbol previews ---

    /// Renders the preview `scene` (`variant * 1000 + gate`; the gate
    /// index equal to the number of gates is the whole variant).
    pub fn render_scene(
        &mut self,
        width: f32,
        height: f32,
        scale: f32,
        scene: i32,
    ) -> slint::Image {
        if width < 1.0 || height < 1.0 {
            return slint::Image::default();
        }
        if !self.previews.contains_key(&scene) {
            let preview = self.build_preview(scene);
            self.previews.insert(scene, preview);
        }
        let Some(Some(preview)) = self.previews.get(&scene) else {
            return slint::Image::default();
        };
        let options = RenderOptions {
            size: RenderSize::Fit {
                width: (width * scale).round() as u32,
                height: (height * scale).round() as u32,
            },
            margin: 10.0 * f64::from(scale),
            background: None,
        };
        render_scene(
            preview.scene(),
            preview.content_bounds(),
            false,
            preview.background(),
            &options,
        )
        .map(|image| crate::dialogs::add_component::to_slint_image(&image))
        .unwrap_or_default()
    }

    /// A symbol with the gates of a variant (one gate at the origin, or all
    /// gates at their positions), pins named like the displayed signals.
    fn build_preview(&mut self, scene: i32) -> Option<SymbolScene> {
        let variant = usize::try_from(scene / 1000).ok()?;
        let gate = usize::try_from(scene % 1000).ok()?;
        self.load_gate_symbols();
        let cmp = self.core.editor.element();
        let var = cmp.symbol_variants().iter().nth(variant)?;
        let gate = (gate < var.symbol_items().len()).then_some(gate);
        let out = assemble_variant_symbol(cmp, variant, gate, &self.symbols, true)?;
        Some(SymbolScene::build(
            &out,
            self.font.as_ref(),
            &ColorScheme::SCHEMATIC_LIGHT,
        ))
    }

    /// The loaded symbols (tests).
    pub fn symbol_count(&self) -> usize {
        self.symbols.values().filter(|s| s.is_some()).count()
    }

    /// Adds a new attribute list (tests): replaces the attribute rows.
    pub fn set_attributes(&mut self, list: &AttributeList) {
        self.attributes = AttributeEditor::new(list);
    }
}

/// A symbol with the gates of a variant of `cmp` (one gate at the origin,
/// or all gates at their positions if `gate` is `None`); with
/// `label_signals`, the pins are named like the displayed signals.
pub fn assemble_variant_symbol(
    cmp: &Component,
    variant: usize,
    gate: Option<usize>,
    symbols: &HashMap<Uuid, Option<Symbol>>,
    label_signals: bool,
) -> Option<Symbol> {
    let var = cmp.symbol_variants().iter().nth(variant)?;
    let metadata = BaseMetadata::new(
        Uuid::new_random(),
        "0.1".parse::<Version>().ok()?,
        "",
        Utc::now(),
        ElementName::new("Preview").ok()?,
        "",
        "",
    );
    let mut out = Symbol::new(metadata).ok()?;
    for (gi, g) in var.symbol_items().iter().enumerate() {
        if gate.is_some_and(|g| g != gi) {
            continue;
        }
        let Some(Some(sym)) = symbols.get(&g.symbol_uuid()) else {
            continue;
        };
        let (rotation, offset) = if gate.is_none() {
            (g.symbol_rotation(), g.symbol_position())
        } else {
            (Angle::DEG0, Point::ORIGIN)
        };
        let place = |o: &mut dyn Transformable| {
            o.rotate(rotation, Point::ORIGIN);
            o.translate(offset);
        };
        for pin in sym.pins().iter() {
            let mut pin = pin.clone();
            let item = g
                .pin_signal_map()
                .iter()
                .find(|i| i.pin_uuid() == pin.uuid());
            let signal = item
                .and_then(|i| i.signal_uuid())
                .and_then(|u| cmp.signals().by_uuid(&u));
            let text = match (item.map(|i| i.display_type()), signal) {
                (Some(CmpSigPinDisplayType::ComponentSignal), Some(s)) => Some(s.name().clone()),
                (Some(CmpSigPinDisplayType::NetSignal), Some(s)) => CircuitIdentifier::new(
                    CircuitIdentifier::clean(if s.forced_net_name().is_empty() {
                        s.name().as_str()
                    } else {
                        s.forced_net_name()
                    }),
                )
                .ok(),
                _ => None,
            };
            if label_signals && let Some(text) = text {
                pin.set_name(text);
            }
            place(&mut pin);
            out.pins_mut().push(pin);
        }
        for p in sym.polygons().iter() {
            let mut p = p.clone();
            place(&mut p);
            out.polygons_mut().push(p);
        }
        for c in sym.circles().iter() {
            let mut c = c.clone();
            place(&mut c);
            out.circles_mut().push(c);
        }
        for t in sym.texts().iter() {
            let mut t = t.clone();
            place(&mut t);
            out.texts_mut().push(t);
        }
    }
    Some(out)
}
