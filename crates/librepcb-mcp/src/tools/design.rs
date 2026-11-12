//! Declarative edits to existing schematic components with a reviewed patch.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::project::ComponentInstanceId;
use librepcb_editor::commands::{EditComponent, MoveSymbol};
use librepcb_editor::design_preview::{DesignEdit, DesignPreview};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::session::Session;
use crate::tools::write::Written;
use crate::{resolve, units::PointMm, views};

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
pub struct ExistingComponent {
    /// UUID of an existing component instance.
    uuid: String,
    /// Managed value. Omission preserves the current value.
    value: Option<ManagedValue>,
    /// Managed position of its single symbol on this sheet.
    /// Omission preserves manual placement.
    position: Option<ManagedPosition>,
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
    /// Required pin memberships, using `key.pin` (for example `pullup.2`).
    /// These are assertions against the existing circuit.
    #[serde(default)]
    nets: BTreeMap<String, Vec<String>>,
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
        if let Some(position) = &spec.position {
            let mut symbols = schematic.symbols().values().filter(|s| s.component() == id);
            let symbol = symbols
                .next()
                .filter(|_| symbols.next().is_none())
                .ok_or_else(|| {
                    ToolError::invalid(format!("{key} must have exactly one symbol on this sheet."))
                })?;
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
                edits.push(DesignEdit::Symbol(MoveSymbol {
                    symbol: symbol.id(),
                    position: Some(desired),
                    rotation: None,
                    mirrored: None,
                }));
            }
        }
    }
    let mut verified = 0;
    for (name, pins) in &doc.nets {
        let (net, _) = p
            .circuit()
            .net_signal_by_name(name)
            .ok_or_else(|| ToolError::not_found(format!("Net {name} not found.")))?;
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
            verified += 1;
        }
    }
    let (preview, project) = DesignPreview::prepare(&open.editor, edits)?;
    let image = librepcb_scene::render_schematic_png(&project, page, &Default::default())?;
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
    let revision_before = preview.revision;
    preview.apply(&mut open.editor)?;
    open.design_preview = None;
    Written {
        value: (),
        revision_before,
    }
    .output(
        open,
        format!("Applied {count} design mutations."),
        json!({"mutation_count": count}),
    )
}
