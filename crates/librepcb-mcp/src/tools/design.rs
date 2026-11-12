//! Declarative edits to existing schematic components with a reviewed patch.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::attribute::{Attribute, AttributeKey, AttributeType};
use librepcb_core::project::{
    ComponentInstanceId, ComponentSignalRef, LibraryElementKind, NetSignalId, Project, SchematicId,
};
use librepcb_editor::Transaction;
use librepcb_editor::commands::{
    AddComponent, AddLibraryElement, AutoPlaceSymbols, EditComponent, MoveSymbol, SymbolPlacement,
    SymbolStyle, WireGroup, connect_design_pin,
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
    /// UUID of an existing component instance.
    uuid: String,
    /// Managed value. Omission preserves the current value.
    value: Option<ManagedValue>,
    /// Managed position of its single symbol on this sheet.
    /// Omission preserves manual placement.
    position: Option<ManagedPosition>,
    /// Managed rotation of its single symbol on this sheet, in degrees.
    /// Omission preserves manual rotation.
    rotation: Option<ManagedRotation>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DesignDocument {
    /// Schema version. Currently 1.
    version: u32,
    /// Schematic name or UUID for the preview.
    schematic: String,
    /// Stable document keys bound to existing component UUIDs.
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
    /// Optional artwork for rectangular placeholders, keyed by embedded library
    /// symbol UUID (see component_get). All instances using that symbol change.
    #[serde(default)]
    symbol_styles: BTreeMap<String, Artwork>,
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
    /// At least two pins as `key.pin`, each with one symbol pin on this sheet.
    pins: Vec<String>,
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
        let id = ComponentInstanceId(resolve::required_uuid(&spec.uuid, "component")?);
        if !seen.insert(id) {
            return Err(ToolError::invalid(format!("Duplicate binding for {id}.")));
        }
        let component = p
            .circuit()
            .component_instance(id)
            .ok_or_else(|| ToolError::not_found(format!("Component {key} ({id}) not found.")))?;
        bindings.insert(key.as_str(), id);
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
    let mut grouped = BTreeSet::new();
    for group in &doc.wire_groups {
        let (net, pins) = resolve_memberships(p, &bindings, &group.net, &group.pins)?;
        for (pin, name) in pins.iter().zip(&group.pins) {
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
        if let Some(segments) = command.segments_to_replace(p)? {
            diff.push(json!({
                "field":"wire_group","net":group.net,"pins":group.pins,"replaced_segments":segments,
            }));
            edits.push(DesignEdit::WireGroup(command));
        }
    }
    let (preview, project, added) = DesignPreview::prepare(&open.editor, |tx| -> ToolResult<_> {
        for edit in edits {
            edit.execute(tx)?;
        }
        let mut added = Vec::new();
        for (key, spec) in &doc.add_components {
            if let Some(change) = add_component(tx, page, key, spec)? {
                added.push(change);
            }
        }
        Ok(added)
    })?;
    diff.extend(added);
    let schematic = project
        .schematic(page)
        .ok_or_else(|| ToolError::not_found("Preview schematic not found."))?;
    let image = super::output::render_schematic_preview(&project, schematic, 1600, 1200)?;
    let result = ToolOutput::new(
        format!("{} schematic changes.", diff.len()),
        json!({
            "preview": preview.id, "base_revision": preview.revision, "diff": diff,
            "verified_pins": verified,
        }),
    )?
    .image(image);
    open.design_preview = Some(preview);
    Ok(result)
}

const DESIGN_KEY: &str = "FOILFERN_DESIGN_KEY";

fn add_component(
    tx: &mut Transaction<'_>,
    page: SchematicId,
    key: &str,
    spec: &NewComponent,
) -> ToolResult<Option<serde_json::Value>> {
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
        return Ok(None);
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
    Ok(Some(
        json!({"field":"add_component","component":key,"designator":designator,
        "uuid":id,"library_component":component,"device":device,"package":package_id,"value":spec.value,"connections":endpoints}),
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
