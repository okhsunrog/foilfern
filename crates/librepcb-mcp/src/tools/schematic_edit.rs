//! Circuit and schematic write tools: `component_add`, `component_remove`,
//! `component_update`, `symbol_move`, `connect`, `disconnect`,
//! `net_rename`, `net_class_set`, `schematic_add`, `schematic_tidy`.
//!
//! Every tool is one undo group of the editor (see [`write`]) built from
//! the editor commands; the response re-reads the affected entities.

use std::collections::BTreeMap;

use librepcb_core::attribute::{Attribute, AttributeKey, AttributeType};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::project::{ComponentInstanceId, ComponentSignalRef, SchematicId, SymbolId};
use librepcb_core::types::{
    Angle, CircuitIdentifier, ElementName, Point, PositiveLength, UnsignedLength, Uuid,
};
use librepcb_core::workspace::{ElementKind, SearchQuery};
use librepcb_editor::commands::{
    AddComponent, AddNetClass, AddSchematic, AutoPlaceSymbols, ConnectNet, DisconnectSignals,
    EditComponent, EditNet, EditNetClass, MoveSymbol, NetRef, RemoveComponent, SymbolPlacement,
    TidySchematic,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::resolve;
use crate::session::Session;
use crate::tools::circuit::{component_detail, nets_json};
use crate::tools::write::write;
use crate::units::{PointMm, angle, length};
use crate::views;

/// Arguments of `component_add`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComponentAddArgs {
    /// The part: UUID of a library device or component, or a name/keyword
    /// (e.g. "LED 3mm red", "Resistor", "GND") looked up like
    /// library_search: an exact name match wins, devices are preferred.
    pub part: String,
    /// UUID of the device (footprint) if `part` is a component; default:
    /// the only (or first) device of the component in the libraries.
    #[serde(default)]
    pub device: Option<String>,
    /// Designator (default: automatic, e.g. "R1").
    #[serde(default)]
    pub designator: Option<String>,
    /// Value (default: the component's default value, e.g. "{{RESISTANCE}}").
    #[serde(default)]
    pub value: Option<String>,
    /// Schematic page name, index or UUID (default: first page).
    #[serde(default)]
    pub schematic: Option<String>,
    /// Position of the first gate in mm (default: a free place on the page).
    #[serde(default)]
    pub position: Option<PointMm>,
    /// Rotation in degrees (default 0).
    #[serde(default)]
    pub rotation: Option<f64>,
    /// Mirror the symbols.
    #[serde(default)]
    pub mirror: bool,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// Arguments of `component_remove`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComponentRemoveArgs {
    /// Designator or UUID.
    pub component: String,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// Arguments of `component_update`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComponentUpdateArgs {
    /// Designator or UUID.
    pub component: String,
    /// New designator.
    #[serde(default)]
    pub designator: Option<String>,
    /// New value (e.g. "330R", "100nF").
    #[serde(default)]
    pub value: Option<String>,
    /// Attributes to set (key -> value; null removes the attribute). Keys
    /// are upper case, e.g. "MPN", "TOLERANCE"; existing attributes keep
    /// their type and unit, new ones are plain strings.
    #[serde(default)]
    pub attributes: Option<BTreeMap<String, Option<String>>>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// Arguments of `symbol_move`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SymbolMoveArgs {
    /// Designator or UUID of the component, or UUID of a symbol.
    pub component: String,
    /// Gate of multi-gate components: suffix (e.g. "A"), index ("0") or
    /// gate UUID (default: the only placed gate).
    #[serde(default)]
    pub gate: Option<String>,
    /// New position in mm.
    #[serde(default)]
    pub position: Option<PointMm>,
    /// New rotation in degrees.
    #[serde(default)]
    pub rotation: Option<f64>,
    /// New mirror state.
    #[serde(default)]
    pub mirror: Option<bool>,
    /// Instead of `position`: move to a free place on the page.
    #[serde(default)]
    pub auto: bool,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// Arguments of `connect`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectArgs {
    /// Net name (created if it does not exist; nets of the pins are merged
    /// into it). Default: forced name of a supply pin, else the existing
    /// net of a pin, else an automatic name. Supply symbols (GND, VCC)
    /// force their net name.
    #[serde(default)]
    pub net: Option<String>,
    /// Pins "R1.1" (pad name) or "U1.VCC" (signal name); a designator
    /// alone ("GND1") for single-pin components like supply symbols.
    /// Unconnected supply symbols whose net name equals the net (e.g.
    /// "GND") are attached automatically.
    pub pins: Vec<String>,
    /// Pins on the same page closer than this (mm, Manhattan) are wired
    /// directly, others get a stub wire with a net label (default 25.4; 0:
    /// labels only).
    #[serde(default)]
    pub max_wire_length: Option<f64>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// Arguments of `disconnect`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DisconnectArgs {
    /// Pins "R1.1" / "U1.VCC" to disconnect from their nets.
    pub pins: Vec<String>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// Arguments of `net_rename`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NetRenameArgs {
    /// Current net name or UUID.
    pub net: String,
    /// New name.
    pub name: String,
    /// If a net with the new name exists: merge into it (default: error).
    #[serde(default)]
    pub merge: bool,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// Arguments of `net_class_set`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NetClassSetArgs {
    /// Net class name or UUID (created if no such class exists).
    pub net_class: String,
    /// Nets (names or UUIDs) to move into the net class.
    #[serde(default)]
    pub nets: Vec<String>,
    /// Default trace width of the class in mm (autorouter and trace_add
    /// use it).
    #[serde(default)]
    pub default_trace_width: Option<f64>,
    /// Default via drill diameter of the class in mm.
    #[serde(default)]
    pub default_via_drill: Option<f64>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// Arguments of `schematic_add`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchematicAddArgs {
    /// Name of the page.
    pub name: String,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// Arguments of `schematic_tidy`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchematicTidyArgs {
    /// Schematic page name, index or UUID (default: first page).
    #[serde(default)]
    pub schematic: Option<String>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

fn positive(mm: f64, what: &str) -> ToolResult<PositiveLength> {
    PositiveLength::new(length(mm)?)
        .map_err(|_| ToolError::invalid(format!("{what} must be greater than zero")))
}

fn identifier(s: &str, what: &str) -> ToolResult<CircuitIdentifier> {
    CircuitIdentifier::new(s.trim())
        .map_err(|e| ToolError::invalid(format!("invalid {what} \"{s}\": {e}")))
}

/// A library part chosen for `component_add`.
#[derive(Debug)]
struct Part {
    component: Uuid,
    device: Option<Uuid>,
    name: String,
    devices_available: Vec<Value>,
    warnings: Vec<String>,
}

/// The component of a device (project library or workspace).
fn device_component(session: &Session, device: Uuid) -> ToolResult<Option<Uuid>> {
    if let Some(open) = &session.project
        && let Some(d) = open.project().library().device(&device)
    {
        return Ok(Some(d.component_uuid()));
    }
    if let Some(ws) = &session.workspace
        && let Some(dir) = ws.library_db().element_dir(ElementKind::Device, device)?
    {
        return Ok(ws
            .library_db()
            .device_metadata(&dir)?
            .map(|i| i.component_uuid));
    }
    Ok(None)
}

fn is_component(session: &Session, uuid: Uuid) -> ToolResult<bool> {
    if let Some(open) = &session.project
        && open.project().library().component(&uuid).is_some()
    {
        return Ok(true);
    }
    match &session.workspace {
        Some(ws) => Ok(ws
            .library_db()
            .element_dir(ElementKind::Component, uuid)?
            .is_some()),
        None => Ok(false),
    }
}

/// Name and deprecation of a library element.
fn element_name(session: &Session, kind: ElementKind, uuid: Uuid) -> (String, bool) {
    if let Some(open) = &session.project {
        let lib = open.project().library();
        let name = match kind {
            ElementKind::Device => lib
                .device(&uuid)
                .map(|e| e.metadata().name().as_str().to_owned()),
            ElementKind::Component => lib
                .component(&uuid)
                .map(|e| e.metadata().name().as_str().to_owned()),
            _ => None,
        };
        if let Some(name) = name {
            return (name, false);
        }
    }
    if let Some(ws) = &session.workspace {
        let locale = ws.settings().library_locale_order.get().clone();
        if let Ok(Some(s)) = ws.library_db().element_summary(kind, uuid, &locale) {
            return (s.name, s.deprecated);
        }
    }
    (uuid.to_string(), false)
}

/// Resolves the `part` (and `device`) of `component_add`.
fn resolve_part(session: &Session, part: &str, device: Option<&str>) -> ToolResult<Part> {
    let mut warnings = Vec::new();
    let explicit_device = device
        .map(|d| resolve::required_uuid(d, "device"))
        .transpose()?;
    let (component, mut device) = match resolve::parse_uuid(part) {
        Some(uuid) => {
            if let Some(cmp) = device_component(session, uuid)? {
                (cmp, Some(uuid))
            } else if is_component(session, uuid)? {
                (uuid, None)
            } else {
                return Err(ToolError::not_found(format!(
                    "There is no library component or device {uuid} (use library_search; \
                     workspace libraries must be installed and scanned)."
                )));
            }
        }
        None => search_part(session, part)?,
    };
    if let Some(d) = explicit_device {
        match device_component(session, d)? {
            Some(c) if c == component => device = Some(d),
            Some(_) => {
                return Err(ToolError::invalid(format!(
                    "The device {d} is not a device of the component {component}."
                )));
            }
            None => return Err(ToolError::not_found(format!("There is no device {d}."))),
        }
    }
    // Devices of the component.
    let mut candidates: Vec<Uuid> = Vec::new();
    if let Some(open) = &session.project {
        candidates.extend(
            open.project()
                .library()
                .devices_of_component(&component)
                .iter()
                .map(|d| d.metadata().uuid()),
        );
    }
    if let Some(ws) = &session.workspace {
        candidates.extend(ws.library_db().component_devices(component)?);
    }
    candidates.sort();
    candidates.dedup();
    let mut named: Vec<(String, bool, Uuid)> = candidates
        .into_iter()
        .map(|u| {
            let (name, deprecated) = element_name(session, ElementKind::Device, u);
            (name, deprecated, u)
        })
        .collect();
    named.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| views::natural_cmp(&a.0, &b.0)));
    let devices_available: Vec<Value> = named
        .iter()
        .map(|(n, dep, u)| json!({ "uuid": u, "name": n, "deprecated": dep }))
        .collect();
    if device.is_none()
        && let Some((name, _, uuid)) = named.first()
    {
        device = Some(*uuid);
        if named.len() > 1 {
            warnings.push(format!(
                "{} devices (footprints) are available for this component, \"{name}\" was \
                 chosen; pass `device` (see devices_available) to choose another one.",
                named.len()
            ));
        }
    }
    let (name, deprecated) = match device {
        Some(d) => element_name(session, ElementKind::Device, d),
        None => element_name(session, ElementKind::Component, component),
    };
    if deprecated {
        warnings.push(format!("\"{name}\" is deprecated."));
    }
    Ok(Part {
        component,
        device,
        name,
        devices_available,
        warnings,
    })
}

/// Finds a part by name: exact name match (devices first), else the only
/// device or component found.
fn search_part(session: &Session, query: &str) -> ToolResult<(Uuid, Option<Uuid>)> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Err(ToolError::invalid("empty part"));
    }
    // (kind, uuid, name, deprecated, component of a device)
    let mut found: Vec<(ElementKind, Uuid, String, bool, Option<Uuid>)> = Vec::new();
    if let Some(open) = &session.project {
        let lib = open.project().library();
        for d in lib.devices().values() {
            let name = d.metadata().name().as_str();
            if name.to_lowercase().contains(&needle) {
                found.push((
                    ElementKind::Device,
                    d.metadata().uuid(),
                    name.to_owned(),
                    false,
                    Some(d.component_uuid()),
                ));
            }
        }
        for c in lib.components().values() {
            let name = c.metadata().name().as_str();
            if name.to_lowercase().contains(&needle) {
                found.push((
                    ElementKind::Component,
                    c.metadata().uuid(),
                    name.to_owned(),
                    false,
                    None,
                ));
            }
        }
    }
    if let Some(ws) = &session.workspace {
        let q = SearchQuery {
            keyword: query.trim().to_owned(),
            kinds: vec![ElementKind::Device, ElementKind::Component],
            include_parts: true,
            locale_order: ws.settings().library_locale_order.get().clone(),
            limit: Some(100),
        };
        for s in ws.library_db().search(&q)? {
            if found.iter().any(|f| f.0 == s.kind && f.1 == s.uuid) {
                continue;
            }
            let component = s.device.as_ref().map(|d| d.component_uuid);
            found.push((s.kind, s.uuid, s.name, s.deprecated, component));
        }
    }
    let pick = |f: &(ElementKind, Uuid, String, bool, Option<Uuid>)| match f.0 {
        ElementKind::Device => f.4.map(|c| (c, Some(f.1))),
        _ => Some((f.1, None)),
    };
    // Exact name matches, devices first, not deprecated first.
    let mut exact: Vec<_> = found
        .iter()
        .filter(|f| f.2.to_lowercase() == needle)
        .collect();
    exact.sort_by_key(|f| (f.0 != ElementKind::Device, f.3));
    if let Some(result) = exact.first().and_then(|f| pick(f)) {
        return Ok(result);
    }
    let active: Vec<_> = found.iter().filter(|f| !f.3).collect();
    let devices: Vec<_> = active
        .iter()
        .filter(|f| f.0 == ElementKind::Device)
        .collect();
    let components: Vec<_> = active
        .iter()
        .filter(|f| f.0 == ElementKind::Component)
        .collect();
    let single = match (devices.as_slice(), components.as_slice()) {
        ([d], _) => pick(d),
        ([], [c]) => pick(c),
        _ => None,
    };
    if let Some(result) = single {
        return Ok(result);
    }
    if found.is_empty() {
        return Err(ToolError::not_found(format!(
            "No library device or component matches \"{query}\" (use library_search; install \
             libraries with library_install or copy them into the workspace and call \
             library_rescan)."
        )));
    }
    let list: Vec<String> = found
        .iter()
        .take(12)
        .map(|f| {
            format!(
                "{} \"{}\" {}",
                match f.0 {
                    ElementKind::Device => "device",
                    _ => "component",
                },
                f.2,
                f.1
            )
        })
        .collect();
    Err(ToolError::invalid(format!(
        "\"{query}\" is ambiguous ({} matches); pass one of these UUIDs as part: {}",
        found.len(),
        list.join("; ")
    )))
}

/// `component_add`.
pub fn component_add(session: &mut Session, args: ComponentAddArgs) -> ToolResult<ToolOutput> {
    let part = resolve_part(session, &args.part, args.device.as_deref())?;
    let p = session.project()?.project();
    let schematic: SchematicId = resolve::schematic(p, args.schematic.as_deref())
        .map_err(|e| {
            ToolError::new(
                e.kind,
                format!("{} Add a page with schematic_add first.", e.message),
            )
        })?
        .1;
    let name = args
        .designator
        .as_deref()
        .map(|d| identifier(d, "designator"))
        .transpose()?;
    let position = args.position.map(PointMm::to_point).transpose()?;
    let rotation = args.rotation.map(angle).transpose()?.unwrap_or(Angle::DEG0);
    let done = write(session, "component_add", args.expected_revision, |editor| {
        let added = editor.execute(AddComponent {
            component: part.component,
            symbol_variant: None,
            name,
            value: args.value.clone(),
            device: part.device,
            place: Some(SymbolPlacement {
                schematic: Some(schematic),
                position: position.unwrap_or(Point::ORIGIN),
                rotation,
                mirrored: args.mirror,
                gate_offset: None,
            }),
        })?;
        if position.is_none() {
            editor.execute(AutoPlaceSymbols {
                symbols: added.symbols.clone(),
                margin: None,
            })?;
        }
        Ok(added)
    })?;
    let open = session.project()?;
    let (summary, mut detail) = component_detail(open.project(), done.value.component)?;
    detail["part"] = json!(part.name);
    detail["devices_available"] = json!(part.devices_available);
    detail["library_elements_added"] = json!(
        done.value
            .library_elements
            .iter()
            .map(|e| json!({ "kind": format!("{:?}", e.kind).to_lowercase(), "uuid": e.uuid }))
            .collect::<Vec<_>>()
    );
    Ok(done
        .output(open, format!("Added {summary}"), detail)?
        .warnings(part.warnings))
}

/// `component_remove`.
pub fn component_remove(
    session: &mut Session,
    args: ComponentRemoveArgs,
) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let (id, c) = resolve::component(p, &args.component)?;
    let designator = c.name().as_str().to_owned();
    let done = write(
        session,
        "component_remove",
        args.expected_revision,
        |editor| {
            Ok(editor.execute(RemoveComponent {
                component: id.into(),
            })?)
        },
    )?;
    let open = session.project()?;
    done.output(
        open,
        format!("Removed {designator} (with its symbols, devices and wires)."),
        json!({ "removed": designator, "uuid": id.0 }),
    )
}

/// `component_update`.
pub fn component_update(
    session: &mut Session,
    args: ComponentUpdateArgs,
) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let (id, c) = resolve::component(p, &args.component)?;
    let name = args
        .designator
        .as_deref()
        .map(|d| identifier(d, "designator"))
        .transpose()?;
    let attributes = match &args.attributes {
        None => None,
        Some(changes) => {
            let mut list = c.attributes().clone();
            for (key, value) in changes {
                let key_obj = AttributeKey::new(key.trim()).map_err(|e| {
                    ToolError::invalid(format!("invalid attribute key \"{key}\": {e}"))
                })?;
                let index = list.iter().position(|a| a.key() == &key_obj);
                let existing = index.and_then(|i| list.take(i));
                if let Some(value) = value {
                    let attribute = existing
                        .as_ref()
                        .and_then(|a| {
                            Attribute::new(key_obj.clone(), a.attribute_type(), value, a.unit())
                                .ok()
                        })
                        .map(Ok)
                        .unwrap_or_else(|| {
                            Attribute::new(key_obj.clone(), AttributeType::String, value, None)
                        })
                        .map_err(|e| ToolError::invalid(format!("attribute {key}: {e}")))?;
                    match index {
                        Some(i) => {
                            list.insert(i, attribute);
                        }
                        None => {
                            list.push(attribute);
                        }
                    }
                }
            }
            Some(list)
        }
    };
    let done = write(
        session,
        "component_update",
        args.expected_revision,
        |editor| {
            Ok(editor.execute(EditComponent {
                component: id.into(),
                name,
                value: args.value.clone(),
                attributes,
                assembly_options: None,
                lock_assembly: None,
            })?)
        },
    )?;
    let open = session.project()?;
    let (summary, detail) = component_detail(open.project(), id)?;
    done.output(open, format!("Updated {summary}"), detail)
}

/// Resolves the symbol of `symbol_move`.
fn resolve_symbol(
    session: &Session,
    component: &str,
    gate: Option<&str>,
) -> ToolResult<(ComponentInstanceId, SymbolId)> {
    let p = session.project()?.project();
    if let Some(uuid) = resolve::parse_uuid(component) {
        for s in p.schematics() {
            if let Some(sym) = s.symbols().get(&SymbolId(uuid)) {
                return Ok((sym.component(), sym.id()));
            }
        }
    }
    let (id, c) = resolve::component(p, component)?;
    let lib = resolve::lib_component(p, c)?;
    let items = lib
        .symbol_variants()
        .by_uuid(&c.lib_variant())
        .map(|v| v.symbol_items().iter().collect::<Vec<_>>())
        .unwrap_or_default();
    let uses = p.component_uses(id).cloned().unwrap_or_default();
    let placed: Vec<(Uuid, SymbolId)> = items
        .iter()
        .filter_map(|i| uses.symbols.get(&i.uuid()).map(|(_, s)| (i.uuid(), *s)))
        .collect();
    let chosen = match gate {
        None => match placed.as_slice() {
            [(_, s)] => Some(*s),
            [] => {
                return Err(ToolError::invalid(format!(
                    "{component} has no placed symbol."
                )));
            }
            _ => {
                return Err(ToolError::invalid(format!(
                    "{component} has {} gates; pass `gate` (suffix, index or UUID).",
                    placed.len()
                )));
            }
        },
        Some(g) => {
            let g = g.trim();
            items
                .iter()
                .enumerate()
                .find(|(i, item)| {
                    item.suffix().as_str() == g
                        || item.uuid().to_string() == g
                        || g.parse::<usize>().ok() == Some(*i)
                })
                .and_then(|(_, item)| uses.symbols.get(&item.uuid()).map(|(_, s)| *s))
        }
    };
    chosen
        .map(|s| (id, s))
        .ok_or_else(|| ToolError::not_found(format!("{component} has no placed gate {gate:?}.")))
}

/// `symbol_move`.
pub fn symbol_move(session: &mut Session, args: SymbolMoveArgs) -> ToolResult<ToolOutput> {
    let (component, symbol) = resolve_symbol(session, &args.component, args.gate.as_deref())?;
    let position = args.position.map(PointMm::to_point).transpose()?;
    let rotation = args.rotation.map(angle).transpose()?;
    if !args.auto && position.is_none() && rotation.is_none() && args.mirror.is_none() {
        return Err(ToolError::invalid(
            "Nothing to do: pass position, rotation, mirror or auto=true.",
        ));
    }
    let done = write(session, "symbol_move", args.expected_revision, |editor| {
        editor.execute(MoveSymbol {
            symbol,
            position,
            rotation,
            mirrored: args.mirror,
        })?;
        if args.auto {
            editor.execute(AutoPlaceSymbols {
                symbols: vec![symbol],
                margin: None,
            })?;
        }
        Ok(())
    })?;
    let open = session.project()?;
    let (summary, detail) = component_detail(open.project(), component)?;
    done.output(open, format!("Moved symbol of {summary}"), detail)
}

/// Resolves pin references to component signals.
fn resolve_pins(session: &Session, pins: &[String]) -> ToolResult<Vec<ComponentSignalRef>> {
    let p = session.project()?.project();
    if pins.is_empty() {
        return Err(ToolError::invalid("No pins given."));
    }
    pins.iter()
        .map(|pin| Ok(resolve::pin(p, pin)?.signal))
        .collect()
}

/// `connect`.
pub fn connect(session: &mut Session, args: ConnectArgs) -> ToolResult<ToolOutput> {
    let signals = resolve_pins(session, &args.pins)?;
    let net = args
        .net
        .as_deref()
        .filter(|n| !n.trim().is_empty())
        .map(|n| identifier(n, "net name"))
        .transpose()?;
    let max_wire_length = args
        .max_wire_length
        .map(|l| {
            UnsignedLength::new(length(l)?)
                .map_err(|_| ToolError::invalid("max_wire_length must not be negative"))
        })
        .transpose()?;
    let done = write(session, "connect", args.expected_revision, |editor| {
        Ok(editor.execute(ConnectNet {
            net,
            signals,
            max_wire_length,
        })?)
    })?;
    let open = session.project()?;
    let r = &done.value;
    let nets = nets_json(open.project(), r.net);
    let mut result = nets.into_iter().next().unwrap_or_else(|| json!({}));
    result["wires"] = json!(r.wires);
    result["labels"] = json!(r.labels);
    result["merged_nets"] = json!(r.merged_nets);
    result["attached_supplies"] = json!(r.attached_supplies);
    let summary = format!(
        "Net {}: {} ({} wire(s), {} label(s)){}{}.",
        r.name,
        result["pins"]
            .as_array()
            .map(|a| a
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "))
            .unwrap_or_default(),
        r.wires,
        r.labels,
        if r.merged_nets.is_empty() {
            String::new()
        } else {
            format!(", merged {}", r.merged_nets.join(", "))
        },
        if r.attached_supplies.is_empty() {
            String::new()
        } else {
            format!(
                ", attached supply symbol(s) {}",
                r.attached_supplies.join(", ")
            )
        }
    );
    Ok(done
        .output(open, summary, result)?
        .warnings(r.warnings.clone()))
}

/// `disconnect`.
pub fn disconnect(session: &mut Session, args: DisconnectArgs) -> ToolResult<ToolOutput> {
    let signals = resolve_pins(session, &args.pins)?;
    let done = write(session, "disconnect", args.expected_revision, |editor| {
        Ok(editor.execute(DisconnectSignals { signals })?)
    })?;
    let open = session.project()?;
    done.output(
        open,
        format!("Disconnected {} pin(s).", done.value),
        json!({ "disconnected": done.value, "pins": args.pins }),
    )
}

/// `net_rename`.
pub fn net_rename(session: &mut Session, args: NetRenameArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let (id, _) = resolve::net(p, &args.net)?;
    let name = identifier(&args.name, "net name")?;
    let done = write(session, "net_rename", args.expected_revision, |editor| {
        Ok(editor.execute(EditNet {
            net: NetRef::Id(id),
            name: Some(name),
            net_class: None,
            merge: args.merge,
        })?)
    })?;
    let open = session.project()?;
    let result = nets_json(open.project(), Some(done.value))
        .into_iter()
        .next()
        .unwrap_or_else(|| json!({}));
    done.output(
        open,
        format!("Net {} renamed to {}.", args.net, args.name),
        result,
    )
}

/// `net_class_set`.
pub fn net_class_set(session: &mut Session, args: NetClassSetArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let key = args.net_class.trim();
    let existing = p
        .circuit()
        .net_class_by_name(key)
        .map(|(id, _)| id)
        .or_else(|| {
            let uuid = resolve::parse_uuid(key)?;
            p.circuit()
                .net_classes()
                .keys()
                .find(|id| id.0 == uuid)
                .copied()
        });
    let name = ElementName::new(key)
        .map_err(|e| ToolError::invalid(format!("invalid net class name \"{key}\": {e}")))?;
    let nets: Vec<_> = args
        .nets
        .iter()
        .map(|n| resolve::net(p, n).map(|(id, _)| id))
        .collect::<ToolResult<_>>()?;
    let width = args
        .default_trace_width
        .map(|w| positive(w, "default_trace_width"))
        .transpose()?;
    let drill = args
        .default_via_drill
        .map(|d| positive(d, "default_via_drill"))
        .transpose()?;
    let done = write(session, "net_class_set", args.expected_revision, |editor| {
        let class = match existing {
            Some(id) => {
                if width.is_some() || drill.is_some() {
                    editor.execute(EditNetClass {
                        net_class: id,
                        name: None,
                        default_trace_width: width,
                        default_via_drill: drill,
                        inherit_defaults: false,
                    })?;
                }
                id
            }
            None => editor.execute(AddNetClass {
                name,
                default_trace_width: width,
                default_via_drill: drill,
            })?,
        };
        for net in nets {
            editor.execute(EditNet {
                net: NetRef::Id(net),
                name: None,
                net_class: Some(class),
                merge: false,
            })?;
        }
        Ok(class)
    })?;
    let open = session.project()?;
    let p = open.project();
    let nc = p.circuit().net_class(done.value);
    let members: Vec<&str> = p
        .circuit()
        .net_signals()
        .values()
        .filter(|n| n.net_class() == done.value)
        .map(|n| n.name().as_str())
        .collect();
    let result = json!({
        "uuid": done.value.0,
        "name": nc.map(|n| n.name().as_str()),
        "default_trace_width": nc.and_then(|n| n.default_trace_width()).map(crate::units::mm),
        "default_via_drill": nc.and_then(|n| n.default_via_drill()).map(crate::units::mm),
        "nets": members,
    });
    done.output(
        open,
        format!("Net class {}: {} net(s).", key, members.len()),
        result,
    )
}

/// `schematic_add`.
pub fn schematic_add(session: &mut Session, args: SchematicAddArgs) -> ToolResult<ToolOutput> {
    let name = ElementName::new(args.name.trim())
        .map_err(|e| ToolError::invalid(format!("invalid page name: {e}")))?;
    let done = write(session, "schematic_add", args.expected_revision, |editor| {
        Ok(editor.execute(AddSchematic { name, index: None })?)
    })?;
    let open = session.project()?;
    let p = open.project();
    let index = p
        .schematics()
        .iter()
        .position(|s| s.id() == done.value)
        .unwrap_or(0);
    done.output(
        open,
        format!("Added schematic page {index} \"{}\".", args.name.trim()),
        json!({ "index": index, "uuid": done.value.0, "name": args.name.trim() }),
    )
}

/// `schematic_tidy`.
pub fn schematic_tidy(session: &mut Session, args: SchematicTidyArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let (index, id, _) = resolve::schematic(p, args.schematic.as_deref())?;
    let done = write(
        session,
        "schematic_tidy",
        args.expected_revision,
        |editor| {
            Ok(editor.execute(TidySchematic {
                schematic: Some(id),
            })?)
        },
    )?;
    let open = session.project()?;
    let r = &done.value;
    done.output(
        open,
        format!(
            "Re-arranged schematic page {index}: {} symbol(s) by connectivity, {} supply \
             symbol(s); {} net(s) drawn with {} wire(s) and {} label(s).",
            r.placed, r.supplies, r.nets, r.wires, r.labels
        ),
        json!({
            "index": index,
            "placed": r.placed,
            "supplies": r.supplies,
            "nets": r.nets,
            "wires": r.wires,
            "labels": r.labels,
        }),
    )
    .map(|out| out.warnings(r.warnings.clone()))
}
