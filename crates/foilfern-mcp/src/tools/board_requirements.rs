//! Transactional board requirements and revision-bound measurements.

use std::collections::BTreeSet;

use foilfern_core::fileio::FileSystem;
use foilfern_core::project::board::Board;
use foilfern_core::project::board_requirements::{
    Check, PATH, Pad, Requirement, Requirements, Severity,
};
use foilfern_core::project::{BoardId, Project};
use foilfern_core::types::Layer;
use foilfern_editor::board_requirements::{Finding, check};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::ToolResult;
use crate::outcome::ToolOutput;
use crate::resolve::{self, required_uuid};
use crate::session::Session;
use crate::tools::{board_edit, trace_path, write};
use crate::units::PointMm;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetArgs {
    /// Replace the full document. An empty requirements list clears it.
    document: Document,
    expected_revision: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    requirements: Vec<RuleInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RuleInput {
    pub(super) id: String,
    pub(super) source: String,
    pub(super) severity: SeverityInput,
    /// Board name, index or UUID.
    pub(super) board: String,
    pub(super) check: CheckInput,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum SeverityInput {
    Hard,
    Warning,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum CheckInput {
    /// Required rectangle for a placed device origin in board millimeters.
    DeviceRegion {
        component: String,
        min: PointMm,
        max: PointMm,
    },
    /// Maximum board distance between two footprint pad centers.
    PadDistance {
        from: trace_path::PadAddress,
        to: trace_path::PadAddress,
        max_distance_mm: f64,
    },
    /// Bounds on the shortest explicit same-net trace path.
    TracePath {
        from: trace_path::PadAddress,
        to: trace_path::PadAddress,
        /// Net name or UUID. Omit to resolve the shared net of the pads.
        net: Option<String>,
        max_length_mm: Option<f64>,
        max_layer_changes: Option<usize>,
        layers: Option<Vec<String>>,
    },
    /// Per-net layer and via restrictions, with optional priority in builtin search.
    NetRouting {
        net: String,
        layers: Option<Vec<String>>,
        allow_vias: Option<bool>,
        #[serde(default)]
        priority: bool,
    },
    /// Preserve the geometry and UUID of an existing trace or via.
    ProtectedCopper { object: String },
}

fn layers(input: Option<Vec<String>>) -> ToolResult<Option<BTreeSet<Layer>>> {
    input
        .map(|layers| {
            layers
                .iter()
                .map(|layer| board_edit::copper_layer(layer))
                .collect()
        })
        .transpose()
}

fn board(project: &Project, id: BoardId) -> ToolResult<&Board> {
    project
        .board(id)
        .ok_or_else(|| crate::error::ToolError::not_found("Requirement board is missing."))
}

fn pad(project: &Project, id: BoardId, address: &trace_path::PadAddress) -> ToolResult<Pad> {
    if let trace_path::PadAddress::Identity(identity) = address
        && let (Some(component), Some(pad)) = (
            resolve::parse_uuid(&identity.component),
            resolve::parse_uuid(&identity.pad),
        )
    {
        return Ok(Pad {
            component: component.into(),
            pad,
        });
    }
    let (component, pad) = trace_path::footprint_pad_id(project, board(project, id)?, address)?;
    Ok(Pad { component, pad })
}

impl RuleInput {
    pub(super) fn resolve(self, project: &Project) -> ToolResult<Requirement> {
        let board_id = match resolve::parse_uuid(&self.board) {
            Some(id) => id.into(),
            None => board_edit::board_id(project, Some(&self.board))?,
        };
        let net = |key: &str| match resolve::parse_uuid(key) {
            Some(id) => Ok(id.into()),
            None => resolve::net(project, key).map(|(id, _)| id),
        };
        let check = match self.check {
            CheckInput::DeviceRegion {
                component,
                min,
                max,
            } => Check::DeviceRegion {
                component: match resolve::parse_uuid(&component) {
                    Some(id) => id.into(),
                    None => resolve::component(project, &component)?.0,
                },
                min: min.to_point()?,
                max: max.to_point()?,
            },
            CheckInput::PadDistance {
                from,
                to,
                max_distance_mm,
            } => Check::PadDistance {
                from: pad(project, board_id, &from)?,
                to: pad(project, board_id, &to)?,
                max_distance: board_edit::positive(max_distance_mm, "max_distance_mm")?,
            },
            CheckInput::TracePath {
                from,
                to,
                net: name,
                max_length_mm,
                max_layer_changes,
                layers: allowed,
            } => {
                let net = match name {
                    Some(name) => net(&name)?,
                    None => {
                        trace_path::ResolvedTracePath::new(
                            project,
                            board(project, board_id)?,
                            &trace_path::TracePathPair {
                                from: from.clone(),
                                to: to.clone(),
                            },
                        )?
                        .preference(None, None)
                        .net
                    }
                };
                Check::TracePath {
                    from: pad(project, board_id, &from)?,
                    to: pad(project, board_id, &to)?,
                    net,
                    max_length: max_length_mm
                        .map(|limit| board_edit::positive(limit, "max_length_mm"))
                        .transpose()?,
                    max_layer_changes,
                    layers: layers(allowed)?,
                }
            }
            CheckInput::NetRouting {
                net: name,
                layers: allowed,
                allow_vias,
                priority,
            } => Check::NetRouting {
                net: net(&name)?,
                layers: layers(allowed)?,
                allow_vias,
                priority,
            },
            CheckInput::ProtectedCopper { object } => Check::ProtectedCopper {
                object: required_uuid(&object, "trace or via")?,
            },
        };
        Ok(Requirement {
            id: self.id,
            source: self.source,
            severity: match self.severity {
                SeverityInput::Hard => Severity::Hard,
                SeverityInput::Warning => Severity::Warning,
            },
            board: board_id,
            check,
        })
    }
}

pub(super) fn requirement_json(rule: &Requirement) -> Value {
    let check = match &rule.check {
        Check::DeviceRegion {
            component,
            min,
            max,
        } => json!({
            "kind":"device_region","component":component,"min":PointMm::from(*min),"max":PointMm::from(*max)
        }),
        Check::PadDistance {
            from,
            to,
            max_distance,
        } => json!({
            "kind":"pad_distance","from":from,"to":to,"max_distance_mm":max_distance.to_mm()
        }),
        Check::TracePath {
            from,
            to,
            net,
            max_length,
            max_layer_changes,
            layers,
        } => json!({
            "kind":"trace_path","from":from,"to":to,"net":net,"max_length_mm":max_length.map(|limit| limit.to_mm()),
            "max_layer_changes":max_layer_changes,"layers":layers
        }),
        Check::NetRouting {
            net,
            layers,
            allow_vias,
            priority,
        } => json!({
            "kind":"net_routing","net":net,"layers":layers,"allow_vias":allow_vias,"priority":priority
        }),
        Check::ProtectedCopper { object } => json!({"kind":"protected_copper","object":object}),
    };
    json!({"id":rule.id,"board":rule.board,"source":rule.source,"severity":rule.severity,"check":check})
}

fn document_json(document: &Requirements) -> Value {
    json!({"version":document.version(),"requirements":document.items().iter().map(requirement_json).collect::<Vec<_>>()})
}

pub fn get(session: &Session) -> ToolResult<ToolOutput> {
    let project = session.project()?.project();
    let document = Requirements::load(project).map_err(foilfern_editor::Error::from)?;
    ToolOutput::new(
        format!("{} board requirements.", document.items().len()),
        json!({"document":document_json(&document),"project":project.uuid(),"revision":project.revision(),"position_unit":"mm"}),
    )
}

pub fn set(session: &mut Session, args: SetArgs) -> ToolResult<ToolOutput> {
    let project = session.project()?.project();
    write::check_revision(project, Some(args.expected_revision))?;
    let document = Requirements::new(
        args.document.version,
        args.document
            .requirements
            .into_iter()
            .map(|rule| rule.resolve(project))
            .collect::<ToolResult<_>>()?,
    )
    .map_err(foilfern_editor::Error::from)?;
    let asset = document.asset().map_err(foilfern_editor::Error::from)?;
    let written = write::write(
        session,
        "board_requirements_set",
        Some(args.expected_revision),
        |tx| {
            if tx.project().directory().read_if_exists(PATH)? != asset {
                tx.write_file(PATH, asset)?;
            }
            Ok(())
        },
    )?;
    written.output(
        session.project()?,
        format!("Stored {} board requirements.", document.items().len()),
        json!({"document":document_json(&document)}),
    )
}

pub fn evaluate(session: &Session) -> ToolResult<ToolOutput> {
    let project = session.project()?.project();
    let findings = check(project)?;
    let blockers: Vec<_> = findings
        .iter()
        .filter(|finding| finding.blocks_routing())
        .map(|finding| &finding.id)
        .collect();
    let pending: Vec<_> = findings
        .iter()
        .filter(|finding| finding.status == foilfern_editor::board_requirements::Status::Pending)
        .map(|finding: &Finding| &finding.id)
        .collect();
    ToolOutput::new(
        format!("Checked {} board requirements.", findings.len()),
        json!({"project":project.uuid(),"revision":project.revision(),"rule_length_unit":"nm","findings":findings,"blocking_requirements":blockers,"pending_requirements":pending}),
    )
}
