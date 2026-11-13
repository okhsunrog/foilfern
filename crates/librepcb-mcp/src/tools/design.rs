//! Declarative schematic creation and edits with a reviewed patch.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Weak};

use librepcb_core::attribute::{Attribute, AttributeKey, AttributeType};
use librepcb_core::geometry::Text;
use librepcb_core::project::schematic::SchematicSymbol;
use librepcb_core::project::{
    ComponentInstanceId, ComponentSignalRef, LibraryElementKind, Mutation, NetSignalId, Project,
    SchematicId, SchematicMutation, SymbolRef,
};
use librepcb_core::types::{
    Alignment, Angle, ElementName, Layer, Length, Point, PositiveLength, Uuid,
};
use librepcb_editor::commands::{
    AddComponent, AddLibraryElement, AddNet, AutoPlaceSymbols, BlockDirection, DrawWirePaths,
    EditComponent, MoveSymbol, PlaceSchematicBlock, Rect, SymbolPlacement, SymbolStyle,
    WireCoordinate, WireGroup, WirePathPoint, WirePathSpec, WireWaypoint, connect_design_pin,
    new_schematic,
};
use librepcb_editor::design_preview::{DesignEdit, DesignPreview, DesignPreviewJob};
use librepcb_editor::{LibraryElementSource, OpenProject, Transaction};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::session::Session;
use crate::tools::write::Written;
use crate::{resolve, units, units::PointMm, views};

mod erc;
mod no_connect;
mod text_placement;
mod topology;
mod work;
pub use work::CancelArgs;
pub(crate) use work::{Stage, WorkQueue};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManagedValue {
    /// Value at the last known baseline. A different current value is a conflict.
    expected: String,
    /// Desired value. If already present, the edit is a no-op.
    set: String,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManagedPosition {
    /// Baseline position in millimeters.
    expected: PointMm,
    /// Desired position in millimeters.
    set: PointMm,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManagedRotation {
    /// Baseline angle in degrees.
    expected: f64,
    /// Desired angle in degrees.
    set: f64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExistingComponent {
    /// UUID of an existing component instance. Supply exactly one of uuid and designator.
    uuid: Option<String>,
    /// Designator of an existing component. Use uuid when the document must survive renames.
    designator: Option<String>,
    /// Managed designator. Bind by UUID to replay across the rename.
    name: Option<ManagedValue>,
    /// Managed value. Omission preserves the current value.
    value: Option<ManagedValue>,
    /// Managed position of its single symbol on this sheet.
    /// Omission preserves manual placement.
    position: Option<ManagedPosition>,
    /// Managed rotation of its single symbol on this sheet, in degrees.
    /// Omission preserves manual rotation.
    rotation: Option<ManagedRotation>,
    /// Managed transforms keyed by placed symbol UUID from schematic_get.
    #[serde(default)]
    symbols: BTreeMap<String, ManagedSymbol>,
    /// Managed absolute positions of symbol fields keyed by text UUID from schematic_get.
    #[serde(default)]
    text_positions: BTreeMap<String, ManagedPosition>,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ManagedSymbol {
    position: Option<ManagedPosition>,
    rotation: Option<ManagedRotation>,
    mirrored: Option<ManagedMirror>,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ManagedMirror {
    expected: bool,
    set: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DesignDocument {
    /// Schema version. Currently 1.
    version: u32,
    /// Schematic name or UUID for the preview.
    schematic: String,
    /// If true, interpret schematic as an exact page name and create it if missing.
    #[serde(default)]
    create_schematic: bool,
    /// Optional time budget in milliseconds, checked between preview stages.
    pub(crate) budget_ms: Option<u64>,
    /// Include before and after PNGs at a shared scale and origin.
    #[serde(default)]
    compare_images: bool,
    /// Optional crop for the paired preview images, in sheet millimeters.
    image_region: Option<super::output::SchematicRegion>,
    /// Stable document keys bound to existing components by UUID or designator.
    #[serde(default)]
    components: BTreeMap<String, ExistingComponent>,
    /// Components to create if their stable document key has no instance.
    #[serde(default)]
    add_components: BTreeMap<String, NewComponent>,
    /// Named nets to ensure before adding parts. Existing nets keep their identity and class.
    #[serde(default)]
    add_nets: Vec<String>,
    /// Rename existing nets by UUID, preserving their connections and geometry.
    #[serde(default)]
    rename_nets: BTreeMap<String, ManagedValue>,
    /// Existing component UUIDs to remove. A missing UUID is already removed.
    #[serde(default)]
    remove_components: Vec<String>,
    /// Existing pins with explicit before and after net names; null means unconnected.
    #[serde(default)]
    pin_nets: BTreeMap<String, topology::ManagedNet>,
    /// Explicit intentional no-connect markers by key.pin. True marks an open
    /// pin, false clears its marker. Omitted pins keep their current markers.
    #[serde(default)]
    no_connects: BTreeMap<String, bool>,
    /// Required pin memberships, using `key.pin` (for example `pullup.2`).
    /// These are assertions against the existing circuit.
    #[serde(default)]
    nets: BTreeMap<String, Vec<String>>,
    /// Local physical connections on the selected sheet between existing members of a net.
    #[serde(default)]
    wire_groups: Vec<LocalWireGroup>,
    /// Add page notes. An identical note at the same position already satisfies the request.
    #[serde(default)]
    notes: Vec<PageNote>,
    /// Optional artwork for rectangular placeholders, keyed by embedded library
    /// symbol UUID (see component_get). All instances using that symbol change.
    #[serde(default)]
    symbol_styles: BTreeMap<String, Artwork>,
    /// Optional annotation-overlap review focused on these component keys.
    #[serde(default)]
    review_components: Vec<String>,
    /// Find a position for one symbol field within a caller-supplied area.
    place_text: Option<text_placement::PlaceText>,
    /// Bounded row/column placement of selected existing components.
    block: Option<SchematicBlock>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SchematicBlock {
    /// Ordered component keys to move. Omitted components stay fixed.
    members: Vec<String>,
    /// Bottom-left and top-right corners in sheet coordinates.
    min: PointMm,
    max: PointMm,
    direction: LayoutDirection,
    /// Minimum gap between members in millimeters.
    gap: f64,
    /// Optional Y (row) or X (column) coordinate in millimeters to align
    /// symbol origins, with each symbol retaining its current grid phase.
    baseline: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum LayoutDirection {
    Row,
    Column,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NewComponent {
    /// Designator used on first creation.
    designator: String,
    library_component: String,
    /// Exact device UUID. Omit for a schematic-only library component.
    device: Option<String>,
    /// Optional exact symbol variant UUID. Defaults to the project's norm.
    symbol_variant: Option<String>,
    value: String,
    /// Initial position in mm. Subsequent manual moves are retained.
    position: Option<PointMm>,
    /// Initial rotation in degrees.
    rotation: Option<f64>,
    /// Mirror the initial symbol placement.
    #[serde(default)]
    mirrored: bool,
    /// Initial fields of a single placed symbol, keyed by name, value, or text UUID.
    #[serde(default)]
    fields: BTreeMap<String, InitialField>,
    /// Maps exact physical package pads (or exact signal names for schematic-only parts) to nets.
    #[serde(default)]
    connections: BTreeMap<String, String>,
    /// Assign connections without stubs when the document supplies explicit wire groups.
    #[serde(default)]
    assign_only: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InitialField {
    /// Offset from the final symbol origin in sheet axes, in mm.
    offset: Option<PointMm>,
    /// Absolute angle in degrees.
    rotation: Option<f64>,
    /// Text height in millimeters.
    height: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, JsonSchema)]
#[serde(untagged)]
enum Artwork {
    Simple(SimpleArtwork),
    WithRoles(ArtworkWithRoles),
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SimpleArtwork {
    Capacitor,
    NormallyOpenPushButton,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, JsonSchema)]
#[serde(tag = "style", rename_all = "snake_case", deny_unknown_fields)]
enum ArtworkWithRoles {
    Crystal {
        /// Two library symbol pin UUIDs for the resonator terminals.
        resonator_pins: [String; 2],
        /// Two library symbol pin UUIDs for the case terminals.
        case_pins: [String; 2],
    },
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LocalWireGroup {
    /// Name of an existing circuit net.
    net: String,
    /// Pins as `key.pin`, each with one symbol pin on this sheet. Required for
    /// automatic wiring. Explicit paths can infer pins from their string vertices.
    pins: Option<Vec<String>>,
    /// Optional explicit paths. Each vertex is a pin name from pins or {x,y}
    /// in mm (Y up). Use orthogonal edges. End a branch on another path to
    /// create a junction. Each group describes one connected segment.
    paths: Option<Vec<Vec<WireVertex>>>,
    /// Label position on an explicit wire. Omit to draw no label.
    label: Option<PathWaypoint>,
}

#[derive(Debug, Deserialize, serde::Serialize, JsonSchema)]
#[serde(untagged)]
enum WireVertex {
    Pin(String),
    Point(PathWaypoint),
}

#[derive(Debug, Deserialize, serde::Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PathWaypoint {
    x: PathCoordinate,
    y: PathCoordinate,
}

#[derive(Debug, Deserialize, serde::Serialize, JsonSchema)]
#[serde(untagged)]
enum PathCoordinate {
    Absolute(f64),
    Relative(RelativeCoordinate),
}

#[derive(Debug, Deserialize, serde::Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RelativeCoordinate {
    /// Pin from this group's pins. This axis uses that pin's final position.
    pin: String,
    /// Optional signed offset in mm along this axis.
    #[serde(default)]
    offset: f64,
}

impl PathWaypoint {
    fn resolve(&self, pins: &[&str]) -> ToolResult<WireWaypoint> {
        let coordinate = |value: &PathCoordinate| -> ToolResult<WireCoordinate> {
            match value {
                PathCoordinate::Absolute(value) => {
                    Ok(WireCoordinate::Absolute(units::length(*value)?))
                }
                PathCoordinate::Relative(relative) => Ok(WireCoordinate::Relative {
                    pin: wire_pin_index(pins, &relative.pin)?,
                    offset: units::length(relative.offset)?,
                }),
            }
        };
        Ok(WireWaypoint {
            x: coordinate(&self.x)?,
            y: coordinate(&self.y)?,
        })
    }
}

impl LocalWireGroup {
    fn pin_names(&self) -> ToolResult<Vec<&str>> {
        if let Some(pins) = &self.pins {
            return Ok(pins.iter().map(String::as_str).collect());
        }
        let paths = self
            .paths
            .as_ref()
            .ok_or_else(|| ToolError::invalid("Automatic wire groups need a pins list."))?;
        let mut seen = BTreeSet::new();
        Ok(paths
            .iter()
            .flatten()
            .filter_map(|point| match point {
                WireVertex::Pin(pin) if seen.insert(pin.as_str()) => Some(pin.as_str()),
                _ => None,
            })
            .collect())
    }
}

fn wire_pin_index(pins: &[&str], pin: &str) -> ToolResult<usize> {
    pins.iter().position(|name| *name == pin).ok_or_else(|| {
        ToolError::invalid(format!(
            "Wire path pin {pin:?} is absent from the group pins."
        ))
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PageNote {
    text: String,
    /// Bottom-left anchor in sheet coordinates in mm.
    position: PointMm,
    /// Text height in mm.
    height: f64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DesignPreviewArgs {
    /// The ID returned by design_preview in this open project.
    preview: String,
}

/// Arguments accepted by `design_apply`.
pub type DesignApplyArgs = DesignPreviewArgs;

pub(crate) struct PreviewJob {
    source_project: Weak<parking_lot::Mutex<OpenProject>>,
    source_library: Arc<dyn LibraryElementSource>,
    snapshot: DesignPreviewJob,
    document: DesignDocument,
}

pub(crate) struct PreparedPreview {
    source_project: Weak<parking_lot::Mutex<OpenProject>>,
    source_library: Arc<dyn LibraryElementSource>,
    preview: DesignPreview,
    output: ToolOutput,
}

/// Synchronous entry point for clients which already own the session lock.
pub fn preview(session: &mut Session, doc: DesignDocument) -> ToolResult<ToolOutput> {
    let mut guard = Arc::new(WorkQueue::default()).try_start(doc.budget_ms)?;
    let job = capture(session, doc)?;
    let result = prepare(job, |stage| guard.work.checkpoint(stage)).and_then(|prepared| {
        guard.work.checkpoint(Stage::Publish)?;
        publish(session, prepared)
    });
    guard.record(&result);
    result
}

pub(crate) fn capture(session: &mut Session, doc: DesignDocument) -> ToolResult<PreviewJob> {
    if doc.version != 1 {
        return Err(ToolError::invalid("Expected design document version 1."));
    }
    let project = session.shared_project().ok_or_else(ToolError::no_project)?;
    let editor = &session.project()?.editor;
    Ok(PreviewJob {
        source_project: Arc::downgrade(&project),
        source_library: editor.source().clone(),
        snapshot: DesignPreviewJob::capture(editor)?,
        document: doc,
    })
}

pub(crate) fn prepare(
    job: PreviewJob,
    checkpoint: impl Fn(Stage) -> ToolResult<()>,
) -> ToolResult<PreparedPreview> {
    checkpoint(Stage::Plan)?;
    let PreviewJob {
        source_project,
        source_library,
        snapshot,
        document: doc,
    } = job;
    let region = doc
        .image_region
        .map(super::output::SchematicRegion::points)
        .transpose()?;
    let p = snapshot.project();
    let mut new_page = None;
    let schematic = if doc.create_schematic {
        let name = ElementName::new(doc.schematic.trim())?;
        match p.schematic_by_name(name.as_str()) {
            Some(schematic) => schematic,
            None => new_page.insert(new_schematic(p, name)?),
        }
    } else {
        resolve::schematic(p, Some(&doc.schematic))?.2
    };
    let page = schematic.id();
    let removals = doc
        .remove_components
        .iter()
        .map(|id| resolve::required_uuid(id, "component to remove").map(ComponentInstanceId))
        .collect::<ToolResult<BTreeSet<_>>>()?;
    let mut bindings = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut edits = Vec::new();
    let mut diff = Vec::new();
    for key in &doc.review_components {
        if !doc.components.contains_key(key) && !doc.add_components.contains_key(key) {
            return Err(ToolError::invalid(format!(
                "Unknown review component key {key:?}."
            )));
        }
    }
    let mut reviewed = BTreeSet::new();
    let mut target_names = BTreeSet::new();
    for key in doc.add_components.keys() {
        if key.is_empty() || key.contains('.') || doc.components.contains_key(key) {
            return Err(ToolError::invalid(format!(
                "Invalid or duplicate component key {key:?}."
            )));
        }
    }
    for (key, spec) in &doc.components {
        checkpoint(Stage::Plan)?;
        if key.is_empty() || key.contains('.') {
            return Err(ToolError::invalid(format!(
                "Invalid component key {key:?}."
            )));
        }
        let (id, component) = match (&spec.uuid, &spec.designator) {
            (Some(uuid), None) => {
                let id = ComponentInstanceId(resolve::required_uuid(uuid, "component")?);
                (
                    id,
                    p.circuit().component_instance(id).ok_or_else(|| {
                        ToolError::not_found(format!("Component {key} ({id}) not found."))
                    })?,
                )
            }
            (None, Some(name)) => resolve::component(p, name)?,
            _ => {
                return Err(ToolError::invalid(format!(
                    "Component {key:?} needs exactly one of uuid and designator."
                )));
            }
        };
        if !seen.insert(id) {
            return Err(ToolError::invalid(format!("Duplicate binding for {id}.")));
        }
        if removals.contains(&id) {
            return Err(ToolError::invalid(format!(
                "Component {key:?} is also listed for removal."
            )));
        }
        bindings.insert(key.as_str(), id);
        if doc.review_components.contains(key) {
            reviewed.insert(id);
        }
        let mut edit = EditComponent {
            component: id.into(),
            name: None,
            value: None,
            attributes: None,
            assembly_options: None,
            lock_assembly: None,
        };
        if let Some(name) = &spec.name {
            if spec.uuid.is_none() {
                return Err(ToolError::invalid(format!(
                    "Component {key:?} must bind by UUID when managing its designator."
                )));
            }
            let desired = super::schematic_edit::identifier(&name.set, "designator")?;
            if !target_names.insert(desired.clone())
                || p.circuit()
                    .component_instance_by_name(desired.as_str())
                    .is_some_and(|(owner, _)| owner != id)
            {
                return Err(ToolError::new(
                    ErrorKind::Conflict,
                    format!(
                        "Designator {desired:?} is already in use or requested by more than one component."
                    ),
                ));
            }
            let current = component.name().as_str();
            if current != desired.as_str() {
                if current != name.expected {
                    return Err(ToolError::new(
                        ErrorKind::Conflict,
                        format!(
                            "{key}.name: expected {:?}, found {current:?}, desired {:?}.",
                            name.expected, name.set
                        ),
                    ));
                }
                diff.push(json!({"component":key,"uuid":id,"field":"name","before":current,"after":desired.as_str()}));
                edit.name = Some(desired);
            }
        }
        if let Some(value) = &spec.value
            && component.value() != &value.set
        {
            if component.value() != &value.expected {
                return Err(ToolError::new(
                    ErrorKind::Conflict,
                    format!(
                        "{key}.value: expected {:?}, found {:?}, desired {:?}.",
                        value.expected,
                        component.value(),
                        value.set
                    ),
                ));
            }
            diff.push(json!({"component":key,"uuid":id,"field":"value","before":component.value(),"after":value.set}));
            edit.value = Some(value.set.clone());
        }
        if edit.name.is_some() || edit.value.is_some() {
            edits.push(DesignEdit::Component(edit));
        }
        let shorthand = spec.position.is_some() || spec.rotation.is_some();
        if shorthand && !spec.symbols.is_empty() {
            return Err(ToolError::invalid(format!(
                "{key}: use either per-symbol or component-level placement fields."
            )));
        }
        if !spec.text_positions.is_empty() && shorthand {
            return Err(ToolError::invalid(format!(
                "{key}: preview symbol transforms and absolute text positions in separate edits."
            )));
        }
        let mut selected = BTreeMap::new();
        for (selector, managed) in &spec.symbols {
            let uuid = resolve::required_uuid(selector, "placed symbol")?;
            let symbol = schematic
                .symbols()
                .get(&uuid.into())
                .filter(|symbol| symbol.component() == id)
                .ok_or_else(|| {
                    ToolError::not_found(format!(
                        "Symbol {uuid} of {key} is not on the selected sheet."
                    ))
                })?;
            if selected.insert(symbol.id(), (symbol, *managed)).is_some() {
                return Err(ToolError::invalid(format!(
                    "{key}: duplicate symbol UUID {uuid}."
                )));
            }
        }
        for (text, position) in &spec.text_positions {
            let uuid = resolve::required_uuid(text, "symbol text")?;
            let (symbol, text) = schematic
                .symbols()
                .values()
                .filter(|symbol| symbol.component() == id)
                .find_map(|symbol| Some((symbol.id(), symbol.texts().get(&uuid)?)))
                .ok_or_else(|| {
                    ToolError::not_found(format!("Text {uuid} was not found on {key}."))
                })?;
            if selected.contains_key(&symbol) {
                return Err(ToolError::invalid(format!(
                    "{key}: preview symbol transforms and absolute text positions in separate edits for symbol {symbol}."
                )));
            }
            let desired = position.set.to_point()?;
            if text.position() == desired {
                continue;
            }
            if text.position() != position.expected.to_point()? {
                return Err(ToolError::new(
                    ErrorKind::Conflict,
                    format!("{key}: text {uuid} has moved since the expected position."),
                ));
            }
            diff.push(json!({"component":key,"field":"text_position","text":uuid,
                "before":views::point(text.position()),"after":views::point(desired)}));
            let mut text = text.clone();
            text.set_position(desired);
            edits.push(DesignEdit::SymbolText {
                symbol: SymbolRef {
                    schematic: page,
                    symbol,
                },
                text,
            });
        }
        if shorthand {
            let mut symbols = schematic.symbols().values().filter(|s| s.component() == id);
            let symbol = symbols
                .next()
                .filter(|_| symbols.next().is_none())
                .ok_or_else(|| {
                    ToolError::invalid(format!("{key} must have exactly one symbol on this sheet."))
                })?;
            plan_symbol_move(
                key,
                symbol,
                ManagedSymbol {
                    position: spec.position,
                    rotation: spec.rotation,
                    mirrored: None,
                },
                &mut diff,
                &mut edits,
            )?;
        }
        for (symbol, managed) in selected.into_values() {
            plan_symbol_move(key, symbol, managed, &mut diff, &mut edits)?;
        }
    }
    let mut block_members = BTreeSet::new();
    let mut block_designators = BTreeMap::new();
    let mut block = if let Some(block) = &doc.block {
        let mut symbols = Vec::new();
        for member in &block.members {
            let id = bindings.get(member.as_str()).ok_or_else(|| {
                ToolError::invalid(format!(
                    "Block member {member:?} must be an existing component key."
                ))
            })?;
            if !block_members.insert(*id) {
                return Err(ToolError::invalid(format!(
                    "{member:?} appears more than once in the block."
                )));
            }
            let spec = &doc.components[member];
            if spec.position.is_some()
                || spec.rotation.is_some()
                || !spec.text_positions.is_empty()
                || !spec.symbols.is_empty()
            {
                return Err(ToolError::invalid(format!(
                    "{member:?} combines block placement with explicit symbol or text placement."
                )));
            }
            let mut instances = schematic
                .symbols()
                .values()
                .filter(|symbol| symbol.component() == *id);
            let symbol = instances
                .next()
                .filter(|_| instances.next().is_none())
                .ok_or_else(|| {
                    ToolError::invalid(format!(
                        "{member:?} must have exactly one symbol on this sheet."
                    ))
                })?;
            symbols.push(symbol.id());
            block_designators.insert(symbol.id(), resolve::designator(p, *id));
            reviewed.insert(*id);
        }
        Some(PlaceSchematicBlock {
            schematic: page,
            symbols,
            area: Rect {
                min: block.min.to_point()?,
                max: block.max.to_point()?,
            },
            direction: match block.direction {
                LayoutDirection::Row => BlockDirection::Row,
                LayoutDirection::Column => BlockDirection::Column,
            },
            gap: librepcb_core::types::UnsignedLength::new(units::length(block.gap)?)?,
            baseline: block.baseline.map(units::length).transpose()?,
            redrawn_segments: BTreeSet::new(),
        })
    } else {
        None
    };
    let (preview, before, project, (added, verified, topology, text_placement)) = snapshot.prepare(
        |tx| -> ToolResult<_> {
            let mut added = Vec::new();
            if let Some(schematic) = new_page {
                added.push(
                    json!({"field":"add_schematic","name":schematic.name().as_str(),"uuid":page}),
                );
                tx.apply(Mutation::AddSchematic {
                    schematic,
                    index: None,
                })?;
            }
            for edit in edits {
                edit.execute(tx)?;
            }
            topology::rename_nets(tx, &doc.rename_nets, &mut added)?;
            let mut topology = topology::TopologyEdits::default();
            topology::remove_components(tx, &removals, &mut added, &mut topology)?;
            for name in &doc.add_nets {
                if tx.project().circuit().net_signal_by_name(name).is_none() {
                    let id = tx.run(AddNet {
                        name: Some(super::schematic_edit::identifier(name, "net name")?),
                        net_class: None,
                    })?;
                    added.push(json!({"field":"add_net","net":name,"uuid":id}));
                }
            }
            for (key, spec) in &doc.add_components {
                checkpoint(Stage::Plan)?;
                let (id, change) = add_component(tx, page, key, spec)?;
                bindings.insert(key.as_str(), id);
                if doc.review_components.contains(key) {
                    reviewed.insert(id);
                }
                if let Some(change) = change {
                    added.push(change);
                }
            }
            topology::set_pin_nets(
                tx,
                page,
                &bindings,
                &doc.pin_nets,
                &mut added,
                &mut topology,
            )?;
            style_symbols(tx, &doc.symbol_styles, &mut added)?;
            let p = tx.project();
            let mut verified = BTreeSet::new();
            for (name, pins) in &doc.nets {
                let (_, signals) =
                    resolve_memberships(p, &bindings, name, pins.iter().map(String::as_str))?;
                verified.extend(signals);
            }
            let mut grouped = BTreeSet::new();
            let mut wire_edits = Vec::new();
            let mut explicit = Vec::new();
            let mut explicit_diffs = Vec::new();
            for group in &doc.wire_groups {
                checkpoint(Stage::Plan)?;
                let names = group.pin_names()?;
                let (net, pins) =
                    resolve_memberships(p, &bindings, &group.net, names.iter().copied())?;
                for (pin, name) in pins.iter().zip(&names) {
                    if block_members.contains(&pin.component) && group.paths.is_none() {
                        return Err(ToolError::invalid(format!(
                            "{name} is a movable block member. Supply explicit paths to redraw its wires in this preview."
                        )));
                    }
                    if !grouped.insert(*pin) {
                        return Err(ToolError::invalid(format!(
                            "{name} appears more than once in wire groups."
                        )));
                    }
                }
                verified.extend(pins.iter().copied());
                let command = WireGroup {
                    schematic: page,
                    net,
                    pins,
                };
                if let Some(paths) = &group.paths {
                    let mut converted = Vec::new();
                    for path in paths {
                        let mut vertices = Vec::new();
                        for vertex in path {
                            vertices.push(match vertex {
                                WireVertex::Pin(pin) => {
                                    WirePathPoint::Pin(wire_pin_index(&names, pin)?)
                                }
                                WireVertex::Point(point) => {
                                    WirePathPoint::Waypoint(point.resolve(&names)?)
                                }
                            });
                        }
                        converted.push(vertices);
                    }
                    let spec = WirePathSpec {
                        group: command,
                        paths: converted,
                        label: group
                            .label
                            .as_ref()
                            .map(|point| point.resolve(&names))
                            .transpose()?,
                    };
                    if let Some(block) = &mut block
                        && spec
                            .group
                            .pins
                            .iter()
                            .any(|pin| block_members.contains(&pin.component))
                    {
                        block.redrawn_segments.extend(spec.replaceable_segments(p)?);
                    }
                    explicit.push(spec);
                    explicit_diffs.push(json!({"field":"wire_paths","net":group.net,"pins":names,
                    "paths":paths,"label":group.label}));
                } else if group.label.is_some() {
                    return Err(ToolError::invalid(
                        "A label position requires explicit wire paths.",
                    ));
                } else if let Some(segments) = command.segments_to_replace(p)? {
                    added.push(json!({
                    "field":"wire_group","net":group.net,"pins":names,"replaced_segments":segments,
                }));
                    // Wire groups run after explicit placement of their endpoints.
                    wire_edits.push(DesignEdit::WireGroup(command));
                }
            }
            for edit in wire_edits {
                edit.execute(tx)?;
            }
            if let Some(block) = block {
                for (symbol, before, after) in tx.run(block)? {
                    added.push(json!({"field":"block_position","symbol":symbol,
                    "component":block_designators[&symbol],
                    "before":views::point(before),"after":views::point(after)}));
                }
            }
            if !explicit.is_empty() {
                let changes = tx.run(DrawWirePaths {
                    schematic: page,
                    groups: explicit,
                })?;
                for (mut change, replaced) in explicit_diffs.into_iter().zip(changes) {
                    if let Some(replaced) = replaced {
                        change["replaced_segments"] = json!(replaced);
                        added.push(change);
                    }
                }
            }
            for note in &doc.notes {
                if note.text.trim().is_empty() {
                    return Err(ToolError::invalid("A page note needs text."));
                }
                let position = note.position.to_point()?;
                let height = PositiveLength::new(units::length(note.height)?)?;
                let sheet = tx
                    .project()
                    .schematic(page)
                    .ok_or_else(|| ToolError::not_found("Schematic not found."))?;
                if sheet.texts().values().any(|text| {
                    text.layer() == Layer::SCHEMATIC_COMMENTS
                        && text.text() == &note.text
                        && text.position() == position
                        && text.height() == height
                        && text.rotation() == Angle::DEG0
                        && text.align() == Alignment::default()
                }) {
                    continue;
                }
                let uuid = Uuid::new_random();
                tx.apply(Mutation::Schematic(SchematicMutation::AddText {
                    schematic: page,
                    text: Text::new(
                        uuid,
                        Layer::SCHEMATIC_COMMENTS,
                        &note.text,
                        position,
                        Angle::DEG0,
                        height,
                        Alignment::default(),
                        false,
                    ),
                }))?;
                added.push(
                json!({"field":"page_note","uuid":uuid,"text":note.text,"position":note.position,"height":note.height}),
            );
            }
            no_connect::update(tx, page, &bindings, &doc.no_connects, &mut added)?;
            topology.remove_unused_nets(tx, &doc.add_nets)?;
            let text_placement = doc.place_text.as_ref().map(|placement| {
                checkpoint(Stage::Plan)?;
                let evidence = placement.place(tx, page, &bindings, &doc.components, || checkpoint(Stage::Plan), &mut added)?;
                if let Some(component) = bindings.get(placement.component()) {
                    reviewed.insert(*component);
                }
                Ok::<_, ToolError>(evidence)
            }).transpose()?;
            Ok((added, verified.len(), topology, text_placement))
        },
    )?;
    checkpoint(Stage::Check)?;
    diff.extend(added);
    let schematic = project
        .schematic(page)
        .ok_or_else(|| ToolError::not_found("Preview schematic not found."))?;
    let mut payload = json!({
        "preview": preview.id, "base_revision": preview.revision, "diff": diff,
        "verified_pins": verified, "erc": erc::compare(&before, &project), "schematic": page,
    });
    if let Some(placement) = text_placement {
        payload["text_placement"] = placement;
    }
    let copper = topology::copper_impact(&before, &project, &topology);
    if !copper.is_empty() {
        payload["board_impact"] = json!(copper);
    }
    if !reviewed.is_empty() {
        let after = super::layout::annotation_review(&project, schematic, &reviewed, true)?;
        checkpoint(Stage::Check)?;
        let baseline = before
            .schematic(page)
            .map(|sheet| super::layout::annotation_review(&before, sheet, &reviewed, false))
            .transpose()?;
        checkpoint(Stage::Check)?;
        payload["annotation_changes"] = after.changes_since(baseline);
        payload["annotation_overlaps"] = json!(after.overlaps().collect::<Vec<_>>());
    }
    let findings = preview.requirement_findings();
    let blockers = findings
        .iter()
        .filter(|finding| finding.blocks_apply())
        .count();
    if !findings.is_empty() {
        payload["requirements"] =
            super::requirements::evidence(&project, findings, preview.revision, Some(preview.id));
    }
    let mut summary = format!("{} schematic changes.", diff.len());
    if blockers > 0 {
        summary.push_str(&format!(
            " {blockers} schematic requirement(s) block apply."
        ));
    }
    checkpoint(Stage::Render)?;
    let output = if doc.compare_images || region.is_some() {
        let images = super::output::render_schematic_comparison(
            &before,
            &project,
            schematic,
            (1600, 1200),
            region,
            || checkpoint(Stage::Render),
        )?;
        payload["images"] = json!(["after", "before"]);
        if let Some((min, max)) = region {
            payload["image_region"] = json!({"min":views::point(min),"max":views::point(max)});
        }
        let mut output = ToolOutput::new(
            format!("{summary} PNGs are ordered after, before."),
            payload,
        )?;
        output.images = images;
        output
    } else {
        let image = super::output::render_schematic_preview(&project, schematic, 1600, 1200, None)?;
        ToolOutput::new(summary, payload)?.image(image)
    };
    Ok(PreparedPreview {
        source_project,
        source_library,
        preview,
        output,
    })
}

pub(crate) fn publish(session: &mut Session, prepared: PreparedPreview) -> ToolResult<ToolOutput> {
    let stale = || {
        ToolError::new(
            ErrorKind::StaleRevision,
            "The project changed while the schematic preview was prepared. Retry design_preview.",
        )
    };
    let current = session.shared_project().ok_or_else(stale)?;
    if !Weak::ptr_eq(&prepared.source_project, &Arc::downgrade(&current)) {
        return Err(stale());
    }
    let open = session.project_mut()?;
    if !prepared.preview.matches(open.project())
        || !Arc::ptr_eq(&prepared.source_library, open.editor.source())
        || open.editor.undo_stack().is_group_active()
    {
        return Err(stale());
    }
    open.design_preview = Some(prepared.preview);
    Ok(prepared.output)
}

const DESIGN_KEY: &str = "FOILFERN_DESIGN_KEY";

fn plan_symbol_move(
    key: &str,
    symbol: &SchematicSymbol,
    managed: ManagedSymbol,
    diff: &mut Vec<serde_json::Value>,
    edits: &mut Vec<DesignEdit>,
) -> ToolResult<()> {
    let conflict = |field: &str, expected: serde_json::Value, found: serde_json::Value| {
        ToolError::new(
            ErrorKind::Conflict,
            format!(
                "{key} symbol {} {field}: expected {expected}, found {found}.",
                symbol.id()
            ),
        )
    };
    let mut record = |field: &str, before: serde_json::Value, after: serde_json::Value| {
        diff.push(json!({"component":key,"uuid":symbol.component(),"symbol":symbol.id(),"field":field,"before":before,"after":after}));
    };
    let mut command = MoveSymbol {
        symbol: symbol.id(),
        position: None,
        rotation: None,
        mirrored: None,
    };
    if let Some(position) = managed.position {
        let desired = position.set.to_point()?;
        let current = symbol.position();
        if current != desired {
            if current != position.expected.to_point()? {
                return Err(conflict(
                    "position",
                    json!(position.expected),
                    views::point(current),
                ));
            }
            record("position", views::point(current), views::point(desired));
            command.position = Some(desired);
        }
    }
    if let Some(rotation) = managed.rotation {
        let desired = units::angle(rotation.set)?.mapped_to_0_360deg();
        let current = symbol.rotation().mapped_to_0_360deg();
        if current != desired {
            if current != units::angle(rotation.expected)?.mapped_to_0_360deg() {
                return Err(conflict(
                    "rotation",
                    json!(rotation.expected),
                    json!(units::deg(current)),
                ));
            }
            record(
                "rotation",
                json!(units::deg(current)),
                json!(units::deg(desired)),
            );
            command.rotation = Some(desired);
        }
    }
    if let Some(mirrored) = managed.mirrored
        && symbol.mirrored() != mirrored.set
    {
        if symbol.mirrored() != mirrored.expected {
            return Err(conflict(
                "mirrored",
                json!(mirrored.expected),
                json!(symbol.mirrored()),
            ));
        }
        record("mirrored", json!(symbol.mirrored()), json!(mirrored.set));
        command.mirrored = Some(mirrored.set);
    }
    if command.position.is_some() || command.rotation.is_some() || command.mirrored.is_some() {
        edits.push(DesignEdit::Symbol(command));
    }
    Ok(())
}

fn style_symbols(
    tx: &mut Transaction<'_>,
    styles: &BTreeMap<String, Artwork>,
    diff: &mut Vec<serde_json::Value>,
) -> ToolResult<()> {
    let mut styled = BTreeSet::new();
    for (key, style) in styles {
        let p = tx.project();
        let id = resolve::required_uuid(key, "library symbol")?;
        if !styled.insert(id) {
            return Err(ToolError::invalid(format!(
                "Duplicate style for symbol {id}."
            )));
        }
        let symbol = p.library().symbol(&id).ok_or_else(|| {
            ToolError::not_found(format!("Project library symbol {id} not found."))
        })?;
        let template = match style {
            Artwork::Simple(SimpleArtwork::Capacitor) => SymbolStyle::Capacitor,
            Artwork::Simple(SimpleArtwork::NormallyOpenPushButton) => {
                SymbolStyle::NormallyOpenPushButton
            }
            Artwork::WithRoles(ArtworkWithRoles::Crystal {
                resonator_pins: [a, b],
                case_pins: [c, d],
            }) => SymbolStyle::Crystal {
                resonator_pins: [
                    resolve::required_uuid(a, "resonator pin")?,
                    resolve::required_uuid(b, "resonator pin")?,
                ],
                case_pins: [
                    resolve::required_uuid(c, "case pin")?,
                    resolve::required_uuid(d, "case pin")?,
                ],
            },
        };
        if let Some(mutation) = template.plan(symbol)? {
            let mut affected: Vec<_> = p
                .circuit()
                .component_instances()
                .values()
                .filter(|component| {
                    p.library()
                        .component(&component.lib_component())
                        .and_then(|lib| lib.symbol_variants().by_uuid(&component.lib_variant()))
                        .is_some_and(|variant| {
                            variant
                                .symbol_items()
                                .iter()
                                .any(|gate| gate.symbol_uuid() == id)
                        })
                })
                .map(|component| component.name().as_str())
                .collect();
            affected.sort_unstable();
            diff.push(json!({
                "field":"symbol_style", "library_symbol":id, "before":"box", "after":style,
                "affected_components":affected,
            }));
            tx.apply(Mutation::SetLibrarySymbolArtwork(mutation))?;
        }
    }
    Ok(())
}

fn add_component(
    tx: &mut Transaction<'_>,
    page: SchematicId,
    key: &str,
    spec: &NewComponent,
) -> ToolResult<(ComponentInstanceId, Option<serde_json::Value>)> {
    let component = resolve::required_uuid(&spec.library_component, "library component")?;
    let device = spec
        .device
        .as_deref()
        .map(|id| resolve::required_uuid(id, "device"))
        .transpose()?;
    let variant = spec
        .symbol_variant
        .as_deref()
        .map(|id| resolve::required_uuid(id, "symbol variant"))
        .transpose()?;
    let name = super::schematic_edit::identifier(&spec.designator, "designator")?;
    let p = tx.project();
    let mut bound = p
        .circuit()
        .component_instances()
        .values()
        .filter(|instance| {
            instance
                .attributes()
                .by_name(DESIGN_KEY, true)
                .is_some_and(|attribute| attribute.value() == key)
        });
    let existing = bound.next();
    let conflict = |message: String| ToolError::new(ErrorKind::Conflict, message);
    if bound.next().is_some() {
        return Err(conflict(format!(
            "More than one instance owns design key {key:?}."
        )));
    }
    let (id, created) = if let Some(instance) = existing {
        if instance.lib_component() != component
            || instance.default_device() != device
            || variant.is_some_and(|variant| variant != instance.lib_variant())
            || instance.value() != &spec.value
        {
            return Err(conflict(format!(
                "Component {key:?} differs in library identity or value."
            )));
        }
        if !p.component_uses(instance.id()).is_some_and(|uses| {
            uses.symbols
                .values()
                .any(|(schematic, _)| *schematic == page)
        }) {
            return Err(conflict(format!(
                "Component {key:?} is already bound to a different sheet or has no placed symbols."
            )));
        }
        (instance.id(), false)
    } else {
        if p.circuit()
            .component_instance_by_name(name.as_str())
            .is_some()
        {
            return Err(conflict(format!("Designator {name} is already in use.")));
        }
        if let Some(device) = device {
            tx.run(AddLibraryElement::new(LibraryElementKind::Device, device))?;
            let lib_device = tx
                .project()
                .library()
                .device(&device)
                .ok_or_else(|| ToolError::not_found(format!("Device {device} not found.")))?;
            if lib_device.component_uuid() != component {
                return Err(ToolError::invalid(format!(
                    "Device {device} does not implement component {component}."
                )));
            }
        } else {
            tx.run(AddLibraryElement::new(
                LibraryElementKind::Component,
                component,
            ))?;
        }
        let added = tx.run(AddComponent {
            component,
            device,
            name: Some(name),
            value: Some(spec.value.clone()),
            symbol_variant: variant,
            place: Some(SymbolPlacement {
                schematic: Some(page),
                position: spec
                    .position
                    .map(PointMm::to_point)
                    .transpose()?
                    .unwrap_or_default(),
                rotation: spec
                    .rotation
                    .map(units::angle)
                    .transpose()?
                    .unwrap_or_default(),
                mirrored: spec.mirrored,
                gate_offset: None,
            }),
        })?;
        if spec.position.is_none() {
            tx.run(AutoPlaceSymbols {
                symbols: added.symbols.clone(),
                margin: None,
            })?;
        }
        if !spec.fields.is_empty() {
            let [id] = added.symbols.as_slice() else {
                return Err(ToolError::invalid(format!(
                    "Initial fields of {key:?} require one placed symbol."
                )));
            };
            let symbol = tx
                .project()
                .schematic(page)
                .and_then(|sheet| sheet.symbols().get(id))
                .ok_or_else(|| {
                    ToolError::not_found(format!("Placed symbol for {key:?} not found."))
                })?;
            let origin = symbol.position();
            let reference = SymbolRef {
                schematic: page,
                symbol: *id,
            };
            let mut fields = BTreeSet::new();
            let mut edits = Vec::new();
            for (selector, field) in &spec.fields {
                let mut text = resolve::symbol_text(symbol, selector)?.clone();
                if !fields.insert(text.uuid()) {
                    return Err(ToolError::invalid(format!(
                        "Multiple selectors refer to the same field of {key:?}."
                    )));
                }
                if let Some(offset) = field.offset {
                    let offset = offset.to_point()?;
                    let add = |a: Length, b: Length| {
                        a.to_nm()
                            .checked_add(b.to_nm())
                            .map(Length::new)
                            .ok_or_else(|| {
                                ToolError::invalid(format!(
                                    "Field {selector:?} of {key:?} is out of range."
                                ))
                            })
                    };
                    text.set_position(Point::new(
                        add(origin.x, offset.x)?,
                        add(origin.y, offset.y)?,
                    ));
                }
                if let Some(rotation) = field.rotation {
                    text.set_rotation(units::angle(rotation)?);
                }
                if let Some(height) = field.height {
                    text.set_height(PositiveLength::new(units::length(height)?)?);
                }
                edits.push(DesignEdit::SymbolText {
                    symbol: reference,
                    text,
                });
            }
            for edit in edits {
                edit.execute(tx)?;
            }
        }
        let mut attrs = tx
            .project()
            .circuit()
            .component_instance(added.component)
            .ok_or_else(|| ToolError::internal("Added component is missing."))?
            .attributes()
            .clone();
        attrs.push(
            Attribute::new(
                AttributeKey::new(DESIGN_KEY)?,
                AttributeType::String,
                key,
                None,
            )
            .map_err(|e| ToolError::invalid(e.to_string()))?,
        );
        tx.run(EditComponent {
            component: added.component.into(),
            name: None,
            value: None,
            attributes: Some(attrs),
            assembly_options: None,
            lock_assembly: None,
        })?;
        (added.component, true)
    };
    let p = tx.project();
    let library = p.library();
    let instance = p
        .circuit()
        .component_instance(id)
        .ok_or_else(|| ToolError::internal("Design component is missing."))?;
    let lib_component = resolve::lib_component(p, instance)?;
    if device.is_none() && !lib_component.schematic_only() {
        return Err(ToolError::invalid(format!(
            "Component {key:?} needs an exact device UUID."
        )));
    }
    let device_info = device
        .map(|id| {
            let device = library
                .device(&id)
                .ok_or_else(|| ToolError::not_found(format!("Device {id} not found.")))?;
            let package = library
                .package(&device.package_uuid())
                .ok_or_else(|| ToolError::not_found("Device package not found."))?;
            Ok::<_, ToolError>((device, package))
        })
        .transpose()?;
    let designator = instance.name().as_str().to_owned();
    let mut signals = BTreeMap::new();
    let mut endpoints = Vec::new();
    for (pad, net_name) in &spec.connections {
        let (signal, endpoint) = if let Some((device, package)) = device_info {
            let physical = package.pads().by_name(pad, true).ok_or_else(|| {
                ToolError::not_found(format!("Pad {pad:?} not found on {designator}."))
            })?;
            let signal = device
                .pad_signal_map()
                .by_uuid(&physical.uuid())
                .and_then(|mapping| mapping.signal_uuid())
                .ok_or_else(|| {
                    ToolError::invalid(format!("Pad {pad:?} has no component signal."))
                })?;
            (
                signal,
                json!({"pad":pad,"pad_uuid":physical.uuid(),"signal":signal,"net":net_name}),
            )
        } else {
            let signal = lib_component
                .signals()
                .by_name(pad, true)
                .ok_or_else(|| {
                    ToolError::not_found(format!("Signal {pad:?} not found on {designator}."))
                })?
                .uuid();
            (
                signal,
                json!({"signal_name":pad,"signal":signal,"net":net_name}),
            )
        };
        let (net, _) = p
            .circuit()
            .net_signal_by_name(net_name)
            .ok_or_else(|| ToolError::not_found(format!("Net {net_name:?} not found.")))?;
        if signals
            .insert(signal, net)
            .is_some_and(|previous| previous != net)
        {
            return Err(conflict(format!(
                "Pads of {designator} assign one signal to different nets."
            )));
        }
        if !created && instance.signal(&signal).and_then(|signal| signal.net()) != Some(net) {
            return Err(conflict(format!(
                "{designator}.{pad} is no longer on {net_name}."
            )));
        }
        endpoints.push(endpoint);
    }
    if !created {
        return Ok((id, None));
    }
    let package = device_info.map(|(device, _)| device.package_uuid());
    for (signal, net) in signals {
        connect_design_pin(
            tx,
            page,
            ComponentSignalRef {
                component: id,
                signal,
            },
            net,
            spec.assign_only,
        )?;
    }
    Ok((
        id,
        Some(
            json!({"field":"add_component","component":key,"designator":designator,
        "uuid":id,"library_component":component,"device":device,
        "package":package,"value":spec.value,"connections":endpoints}),
        ),
    ))
}

fn resolve_memberships<'a>(
    p: &Project,
    bindings: &BTreeMap<&str, ComponentInstanceId>,
    name: &str,
    pins: impl IntoIterator<Item = &'a str>,
) -> ToolResult<(NetSignalId, Vec<ComponentSignalRef>)> {
    let (net, _) = p
        .circuit()
        .net_signal_by_name(name)
        .ok_or_else(|| ToolError::not_found(format!("Net {name} not found.")))?;
    let mut signals = Vec::new();
    for endpoint in pins {
        let (key, pin) = resolve::split_pin_ref(endpoint)?;
        let id = *bindings
            .get(key)
            .ok_or_else(|| ToolError::invalid(format!("Unknown component key {key:?}.")))?;
        let resolved = resolve::pin(p, &format!("{id}.{pin}"))?;
        let connected = p
            .circuit()
            .component_instance(id)
            .and_then(|c| c.signal(&resolved.signal.signal))
            .and_then(|s| s.net());
        if connected != Some(net) {
            return Err(ToolError::new(
                ErrorKind::Conflict,
                format!(
                    "{endpoint} ({}) is not connected to {name}.",
                    resolved.label
                ),
            ));
        }
        signals.push(resolved.signal);
    }
    Ok((net, signals))
}

pub fn apply(session: &mut Session, args: DesignApplyArgs) -> ToolResult<ToolOutput> {
    let open = session.project_mut()?;
    let id = resolve::required_uuid(&args.preview, "preview")?;
    let preview = open
        .design_preview
        .as_ref()
        .filter(|p| p.id == id)
        .ok_or_else(|| ToolError::not_found("Unknown design preview."))?;
    if !preview.matches(open.project()) {
        return Err(ToolError::new(
            ErrorKind::StaleRevision,
            "Recompute the design preview.",
        ));
    }
    let count = preview.mutation_count();
    let operations = preview.operation_count();
    let revision_before = preview.revision;
    let preview = open
        .design_preview
        .take()
        .ok_or_else(|| ToolError::not_found("Unknown design preview."))?;
    preview.apply(&mut open.editor)?;
    Written {
        value: (),
        revision_before,
    }
    .output(
        open,
        format!("Applied {operations} design operations."),
        json!({"mutation_count": count, "operation_count": operations}),
    )
}

pub fn discard(session: &mut Session, args: DesignPreviewArgs) -> ToolResult<ToolOutput> {
    let id = resolve::required_uuid(&args.preview, "preview")?;
    let open = session.project_mut()?;
    if open
        .design_preview
        .take_if(|preview| preview.id == id)
        .is_none()
    {
        return Err(ToolError::not_found("Unknown design preview."));
    }
    ToolOutput::new("Discarded the schematic preview.", json!({"preview": id}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::project::{ProjectCreateArgs, project_create};

    #[test]
    fn schematic_preview_admission_is_shared_and_released() {
        let state = crate::McpState::new(Arc::new(crate::StandaloneHost));
        let client = state.clone();
        let permit = state.schematic_previews().try_start(None).unwrap();
        assert!(client.schematic_previews().try_start(None).is_err());
        drop(permit);
        assert!(client.schematic_previews().try_start(None).is_ok());
    }

    #[test]
    fn detached_preview_refuses_changed_and_reopened_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let mut session = Session::new();
        let args: ProjectCreateArgs = serde_json::from_value(json!({
            "name":"Preview source", "directory":tmp.path().join("project")
        }))
        .unwrap();
        let created = project_create(&mut session, args).unwrap();
        let path = created.result["path"].as_str().unwrap();
        let doc = || {
            serde_json::from_value(json!({"version":1,"schematic":"Main","notes":[
                {"text":"preview note","position":{"x":0,"y":0},"height":1.5}
            ]}))
            .unwrap()
        };

        let prepared = prepare(capture(&mut session, doc()).unwrap(), |_| Ok(())).unwrap();
        session
            .project_mut()
            .unwrap()
            .editor
            .execute_atomic("Edit net", |tx| {
                tx.run(AddNet::default())?;
                Ok::<_, librepcb_editor::Error>(())
            })
            .unwrap();
        assert_eq!(
            publish(&mut session, prepared).unwrap_err().kind,
            ErrorKind::StaleRevision
        );
        assert!(session.project().unwrap().design_preview.is_none());
        session.project_mut().unwrap().save().unwrap();
        session.project = None;
        session.open_project(path).unwrap();

        let prepared = prepare(capture(&mut session, doc()).unwrap(), |_| Ok(())).unwrap();
        session.project = None;
        session.open_project(path).unwrap();
        assert_eq!(
            prepared.preview.revision,
            session.project().unwrap().project().revision()
        );
        assert!(!Weak::ptr_eq(
            &prepared.source_project,
            &Arc::downgrade(&session.shared_project().unwrap())
        ));
        assert_eq!(
            publish(&mut session, prepared).unwrap_err().kind,
            ErrorKind::StaleRevision
        );
        assert!(session.project().unwrap().design_preview.is_none());

        let prepared = prepare(capture(&mut session, doc()).unwrap(), |_| Ok(())).unwrap();
        let output = publish(&mut session, prepared).unwrap();
        assert_eq!(output.result["diff"].as_array().unwrap().len(), 1);
        assert!(
            session.project().unwrap().project().schematics()[0]
                .texts()
                .is_empty()
        );
    }
}
