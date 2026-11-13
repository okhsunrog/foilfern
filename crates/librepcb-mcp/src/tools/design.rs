//! Declarative edits to existing schematic components with a reviewed patch.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::attribute::{Attribute, AttributeKey, AttributeType};
use librepcb_core::geometry::Text;
use librepcb_core::project::{
    ComponentInstanceId, ComponentSignalRef, LibraryElementKind, Mutation, NetSignalId, Project,
    SchematicId, SchematicMutation, SymbolRef,
};
use librepcb_core::types::{Alignment, Angle, Layer, PositiveLength, Uuid};
use librepcb_editor::Transaction;
use librepcb_editor::commands::{
    AddComponent, AddLibraryElement, AutoPlaceSymbols, BlockDirection, DrawWirePaths,
    EditComponent, MoveSymbol, PlaceSchematicBlock, Rect, SymbolPlacement, SymbolStyle,
    WireCoordinate, WireGroup, WirePathPoint, WirePathSpec, WireWaypoint, connect_design_pin,
};
use librepcb_editor::design_preview::{DesignEdit, DesignPreview};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::session::Session;
use crate::tools::write::Written;
use crate::{resolve, units, units::PointMm, views};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManagedValue {
    /// Value at the last known baseline. A different current value is a conflict.
    expected: String,
    /// Desired value. If already present, the edit is a no-op.
    set: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManagedPosition {
    /// Baseline position in millimeters.
    expected: PointMm,
    /// Desired position in millimeters.
    set: PointMm,
}

#[derive(Debug, Deserialize, JsonSchema)]
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
    /// Managed value. Omission preserves the current value.
    value: Option<ManagedValue>,
    /// Managed position of its single symbol on this sheet.
    /// Omission preserves manual placement.
    position: Option<ManagedPosition>,
    /// Managed rotation of its single symbol on this sheet, in degrees.
    /// Omission preserves manual rotation.
    rotation: Option<ManagedRotation>,
    /// Managed absolute positions of symbol fields keyed by text UUID from schematic_get.
    #[serde(default)]
    text_positions: BTreeMap<String, ManagedPosition>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DesignDocument {
    /// Schema version. Currently 1.
    version: u32,
    /// Schematic name or UUID for the preview.
    schematic: String,
    /// Stable document keys bound to existing components by UUID or designator.
    #[serde(default)]
    components: BTreeMap<String, ExistingComponent>,
    /// Components to create if their stable document key has no instance.
    #[serde(default)]
    add_components: BTreeMap<String, NewComponent>,
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
    device: String,
    value: String,
    /// Initial position in mm. Subsequent manual moves are retained.
    position: Option<PointMm>,
    /// Initial rotation in degrees.
    rotation: Option<f64>,
    /// Maps exact physical package pad names to existing net names.
    connections: BTreeMap<String, String>,
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
    /// Pins as `key.pin`, each with one symbol pin on this sheet. At least two
    /// for automatic wiring. An explicit path can draw a single-pin stub.
    pins: Vec<String>,
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
    fn resolve(&self, group: &LocalWireGroup) -> ToolResult<WireWaypoint> {
        let coordinate = |value: &PathCoordinate| -> ToolResult<WireCoordinate> {
            match value {
                PathCoordinate::Absolute(value) => {
                    Ok(WireCoordinate::Absolute(units::length(*value)?))
                }
                PathCoordinate::Relative(relative) => Ok(WireCoordinate::Relative {
                    pin: group.pin_index(&relative.pin)?,
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
    fn pin_index(&self, pin: &str) -> ToolResult<usize> {
        self.pins
            .iter()
            .position(|name| name == pin)
            .ok_or_else(|| {
                ToolError::invalid(format!("Wire path pin {pin:?} is absent from pins."))
            })
    }
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
pub struct DesignApplyArgs {
    /// The ID returned by design_preview in this open project.
    preview: String,
}

pub fn preview(session: &mut Session, doc: DesignDocument) -> ToolResult<ToolOutput> {
    if doc.version != 1 {
        return Err(ToolError::invalid("Expected design document version 1."));
    }
    let open = session.project_mut()?;
    let p = open.project();
    let (_, page, schematic) = resolve::schematic(p, Some(&doc.schematic))?;
    let mut bindings = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut edits = Vec::new();
    let mut diff = Vec::new();
    let mut styled = BTreeSet::new();
    for key in &doc.review_components {
        if !doc.components.contains_key(key) && !doc.add_components.contains_key(key) {
            return Err(ToolError::invalid(format!(
                "Unknown review component key {key:?}."
            )));
        }
    }
    let mut reviewed = BTreeSet::new();
    for key in doc.add_components.keys() {
        if key.is_empty() || key.contains('.') || doc.components.contains_key(key) {
            return Err(ToolError::invalid(format!(
                "Invalid or duplicate component key {key:?}."
            )));
        }
    }
    for (key, style) in &doc.symbol_styles {
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
            edits.push(DesignEdit::LibrarySymbolArtwork(mutation));
        }
    }
    for (key, spec) in &doc.components {
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
        bindings.insert(key.as_str(), id);
        if doc.review_components.contains(key) {
            reviewed.insert(id);
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
            edits.push(DesignEdit::Component(EditComponent {
                component: id.into(),
                name: None,
                value: Some(value.set.clone()),
                attributes: None,
                assembly_options: None,
                lock_assembly: None,
            }));
        }
        if !spec.text_positions.is_empty() && (spec.position.is_some() || spec.rotation.is_some()) {
            return Err(ToolError::invalid(format!(
                "{key}: preview symbol transforms and absolute text positions in separate edits."
            )));
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
        if spec.position.is_some() || spec.rotation.is_some() {
            let mut symbols = schematic.symbols().values().filter(|s| s.component() == id);
            let symbol = symbols
                .next()
                .filter(|_| symbols.next().is_none())
                .ok_or_else(|| {
                    ToolError::invalid(format!("{key} must have exactly one symbol on this sheet."))
                })?;
            let mut command = MoveSymbol {
                symbol: symbol.id(),
                position: None,
                rotation: None,
                mirrored: None,
            };
            if let Some(position) = &spec.position {
                let desired = position.set.to_point()?;
                let current = symbol.position();
                if current != desired {
                    if current != position.expected.to_point()? {
                        return Err(ToolError::new(
                            ErrorKind::Conflict,
                            format!(
                                "{key}.position: expected {:?}, found {:?}.",
                                position.expected,
                                views::point(current)
                            ),
                        ));
                    }
                    diff.push(json!({"component":key,"uuid":id,"field":"position","before":views::point(current),"after":views::point(desired)}));
                    command.position = Some(desired);
                }
            }
            if let Some(rotation) = &spec.rotation {
                let desired = units::angle(rotation.set)?.mapped_to_0_360deg();
                let current = symbol.rotation().mapped_to_0_360deg();
                if current != desired {
                    if current != units::angle(rotation.expected)?.mapped_to_0_360deg() {
                        return Err(ToolError::new(
                            ErrorKind::Conflict,
                            format!(
                                "{key}.rotation: expected {}°, found {}°.",
                                rotation.expected,
                                units::deg(current)
                            ),
                        ));
                    }
                    diff.push(json!({"component":key,"uuid":id,"field":"rotation","before":units::deg(current),"after":units::deg(desired)}));
                    command.rotation = Some(desired);
                }
            }
            if command.position.is_some() || command.rotation.is_some() {
                edits.push(DesignEdit::Symbol(command));
            }
        }
    }
    let mut verified = 0;
    for (name, pins) in &doc.nets {
        let (_, signals) = resolve_memberships(p, &bindings, name, pins)?;
        verified += signals.len();
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
            if spec.position.is_some() || spec.rotation.is_some() || !spec.text_positions.is_empty()
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
    let mut grouped = BTreeSet::new();
    let mut wire_edits = Vec::new();
    let mut explicit = Vec::new();
    let mut explicit_diffs = Vec::new();
    for group in &doc.wire_groups {
        let (net, pins) = resolve_memberships(p, &bindings, &group.net, &group.pins)?;
        for (pin, name) in pins.iter().zip(&group.pins) {
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
        verified += pins.len();
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
                        WireVertex::Pin(pin) => WirePathPoint::Pin(group.pin_index(pin)?),
                        WireVertex::Point(point) => WirePathPoint::Waypoint(point.resolve(group)?),
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
                    .map(|point| point.resolve(group))
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
            explicit_diffs.push(
                json!({"field":"wire_paths","net":group.net,"pins":group.pins,
                "paths":paths,"label":group.label}),
            );
        } else if group.label.is_some() {
            return Err(ToolError::invalid(
                "A label position requires explicit wire paths.",
            ));
        } else if let Some(segments) = command.segments_to_replace(p)? {
            diff.push(json!({
                "field":"wire_group","net":group.net,"pins":group.pins,"replaced_segments":segments,
            }));
            // Wire groups run after explicit placement of their endpoints.
            wire_edits.push(DesignEdit::WireGroup(command));
        }
    }
    let (preview, project, added) = DesignPreview::prepare(&open.editor, |tx| -> ToolResult<_> {
        for edit in edits {
            edit.execute(tx)?;
        }
        let mut added = Vec::new();
        for edit in wire_edits {
            edit.execute(tx)?;
        }
        for (key, spec) in &doc.add_components {
            let (id, change) = add_component(tx, page, key, spec)?;
            if doc.review_components.contains(key) {
                reviewed.insert(id);
            }
            if let Some(change) = change {
                added.push(change);
            }
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
        Ok(added)
    })?;
    diff.extend(added);
    let schematic = project
        .schematic(page)
        .ok_or_else(|| ToolError::not_found("Preview schematic not found."))?;
    let image = super::output::render_schematic_preview(&project, schematic, 1600, 1200)?;
    let mut payload = json!({
        "preview": preview.id, "base_revision": preview.revision, "diff": diff,
        "verified_pins": verified,
    });
    if !reviewed.is_empty() {
        payload["annotation_overlaps"] = review_annotations(&project, schematic, &reviewed)?;
    }
    let result =
        ToolOutput::new(format!("{} schematic changes.", diff.len()), payload)?.image(image);
    open.design_preview = Some(preview);
    Ok(result)
}

const DESIGN_KEY: &str = "FOILFERN_DESIGN_KEY";

fn review_annotations(
    project: &Project,
    schematic: &librepcb_core::project::schematic::Schematic,
    components: &BTreeSet<ComponentInstanceId>,
) -> ToolResult<serde_json::Value> {
    use librepcb_scene::{ColorScheme, SchematicObject, SchematicScene, annotation_overlaps};
    let symbols: BTreeSet<_> = schematic
        .symbols()
        .iter()
        .filter(|(_, symbol)| components.contains(&symbol.component()))
        .map(|(id, _)| *id)
        .collect();
    for component in components {
        if !schematic
            .symbols()
            .values()
            .any(|symbol| symbol.component() == *component)
        {
            return Err(ToolError::invalid(format!(
                "Review component {} has no symbols on the selected sheet.",
                resolve::designator(project, *component)
            )));
        }
    }
    let segments = schematic
        .net_segments()
        .iter()
        .filter(|(_, segment)| {
            segment
                .connected_pins()
                .iter()
                .any(|(symbol, _)| symbols.contains(symbol))
        })
        .map(|(id, _)| *id)
        .collect();
    let scene = SchematicScene::build(project, schematic.id(), &ColorScheme::SCHEMATIC_LIGHT)?;
    let describe = |object| match object {
        SchematicObject::SymbolText(symbol, text) => {
            let placed = &schematic.symbols()[&symbol];
            let layer = placed.texts().get(&text).map(|t| t.layer().id());
            json!({"kind":"symbol_text","component":resolve::designator(project, placed.component()),"symbol":symbol,"text":text,"layer":layer})
        }
        SchematicObject::NetLabel(segment, label) => {
            let net = schematic.net_segments()[&segment].net();
            json!({"kind":"net_label","net":resolve::net_name(project, net),"segment":segment,"label":label})
        }
        SchematicObject::Text(text) => json!({"kind":"schematic_text","text":text}),
        _ => unreachable!("overlaps only contain annotations"),
    };
    Ok(json!(annotation_overlaps(&scene, Some((&symbols, &segments))).into_iter().map(|overlap|
        json!({"first":describe(overlap.first),"second":describe(overlap.second),"bounds_mm":overlap.bounds})
    ).collect::<Vec<_>>()))
}

fn add_component(
    tx: &mut Transaction<'_>,
    page: SchematicId,
    key: &str,
    spec: &NewComponent,
) -> ToolResult<(ComponentInstanceId, Option<serde_json::Value>)> {
    let component = resolve::required_uuid(&spec.library_component, "library component")?;
    let device = resolve::required_uuid(&spec.device, "device")?;
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
            || instance.default_device() != Some(device)
            || instance.value() != &spec.value
        {
            return Err(conflict(format!(
                "Component {key:?} differs in library identity or value."
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
        let added = tx.run(AddComponent {
            component,
            device: Some(device),
            name: Some(name),
            value: Some(spec.value.clone()),
            symbol_variant: None,
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
                mirrored: false,
                gate_offset: None,
            }),
        })?;
        if spec.position.is_none() {
            tx.run(AutoPlaceSymbols {
                symbols: added.symbols,
                margin: None,
            })?;
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
    let lib_device = library
        .device(&device)
        .ok_or_else(|| ToolError::not_found(format!("Device {device} not found.")))?;
    let package = library
        .package(&lib_device.package_uuid())
        .ok_or_else(|| ToolError::not_found("Device package not found."))?;
    let instance = p
        .circuit()
        .component_instance(id)
        .ok_or_else(|| ToolError::internal("Design component is missing."))?;
    let designator = instance.name().as_str().to_owned();
    let mut signals = BTreeMap::new();
    let mut endpoints = Vec::new();
    for (pad, net_name) in &spec.connections {
        let physical = package.pads().by_name(pad, true).ok_or_else(|| {
            ToolError::not_found(format!("Pad {pad:?} not found on {designator}."))
        })?;
        let signal = lib_device
            .pad_signal_map()
            .by_uuid(&physical.uuid())
            .and_then(|m| m.signal_uuid())
            .ok_or_else(|| ToolError::invalid(format!("Pad {pad:?} has no component signal.")))?;
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
        endpoints
            .push(json!({"pad":pad,"pad_uuid":physical.uuid(),"signal":signal,"net":net_name}));
    }
    if !created {
        return Ok((id, None));
    }
    let package_id = lib_device.package_uuid();
    for (signal, net) in signals {
        connect_design_pin(
            tx,
            page,
            ComponentSignalRef {
                component: id,
                signal,
            },
            net,
        )?;
    }
    Ok((
        id,
        Some(
            json!({"field":"add_component","component":key,"designator":designator,
        "uuid":id,"library_component":component,"device":device,"package":package_id,"value":spec.value,"connections":endpoints}),
        ),
    ))
}

fn resolve_memberships(
    p: &Project,
    bindings: &BTreeMap<&str, ComponentInstanceId>,
    name: &str,
    pins: &[String],
) -> ToolResult<(NetSignalId, Vec<ComponentSignalRef>)> {
    let (net, _) = p
        .circuit()
        .net_signal_by_name(name)
        .ok_or_else(|| ToolError::not_found(format!("Net {name} not found.")))?;
    let mut signals = Vec::with_capacity(pins.len());
    for endpoint in pins {
        let (key, pin) = resolve::split_pin_ref(endpoint)?;
        let id = bindings
            .get(key)
            .ok_or_else(|| ToolError::invalid(format!("Unknown component key {key:?}.")))?;
        let resolved = resolve::pin(p, &format!("{id}.{pin}"))?;
        let connected = p
            .circuit()
            .component_instance(*id)
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
