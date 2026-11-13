//! Schematic requirements with stable IDs and project-revision evidence.

use librepcb_core::fileio::FileSystem;
use librepcb_core::project::schematic_requirements::{
    Check, Finding, Measured, Observation, PATH, Requirement, Requirements, Severity,
};
use librepcb_core::project::{ComponentSignalRef, Project, SymbolRef};
use librepcb_core::types::Uuid;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::ToolResult;
use crate::outcome::ToolOutput;
use crate::resolve::required_uuid;
use crate::session::Session;
use crate::tools::write;
use crate::units::{PointMm, angle, deg};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetArgs {
    /// The full document to store. An empty requirements array clears it.
    document: Document,
    /// Revision returned by requirements_get. The replacement requires a fresh read.
    expected_revision: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Document {
    /// Schema version. Currently 1.
    version: u32,
    requirements: Vec<RequirementInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RequirementInput {
    /// Stable, unique ID chosen by the author.
    id: String,
    /// Where the requirement comes from, such as a design note or data sheet section.
    source: String,
    severity: SeverityInput,
    check: CheckInput,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SeverityInput {
    /// Blocks design_apply when failed or unresolved.
    Hard,
    /// Reports the result without blocking apply.
    Warning,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CheckInput {
    /// Exact symbol origin, in mm on the sheet.
    SymbolPosition {
        symbol: SymbolInput,
        position: PointMm,
    },
    /// Required symbol orientation in degrees and mirror state.
    SymbolOrientation {
        symbol: SymbolInput,
        rotation: f64,
        mirrored: bool,
    },
    /// Inclusive allowed rectangle for a symbol origin, in mm on the sheet.
    SymbolRegion {
        symbol: SymbolInput,
        min: PointMm,
        max: PointMm,
    },
    /// Required component signal membership.
    PinNet {
        pin: PinInput,
        /// Net UUID from net_list, or explicit null for an unconnected signal.
        #[serde(deserialize_with = "Deserialize::deserialize")]
        net: Option<String>,
    },
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SymbolInput {
    /// Sheet UUID from schematic_get.
    schematic: String,
    /// Symbol instance UUID from schematic_get.
    symbol: String,
}

impl SymbolInput {
    fn resolve(self) -> ToolResult<SymbolRef> {
        Ok(SymbolRef {
            schematic: required_uuid(&self.schematic, "schematic")?.into(),
            symbol: required_uuid(&self.symbol, "symbol")?.into(),
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PinInput {
    /// Component instance UUID from component_get.
    component: String,
    /// Library signal UUID in the component_get signals array.
    signal: String,
}

impl CheckInput {
    fn resolve(self) -> ToolResult<Check> {
        Ok(match self {
            Self::SymbolPosition { symbol, position } => Check::SymbolPosition {
                symbol: symbol.resolve()?,
                position: position.to_point()?,
            },
            Self::SymbolOrientation {
                symbol,
                rotation,
                mirrored,
            } => Check::SymbolOrientation {
                symbol: symbol.resolve()?,
                rotation: angle(rotation)?.mapped_to_0_360deg(),
                mirrored,
            },
            Self::SymbolRegion { symbol, min, max } => Check::SymbolRegion {
                symbol: symbol.resolve()?,
                min: min.to_point()?,
                max: max.to_point()?,
            },
            Self::PinNet { pin, net } => Check::PinNet {
                pin: ComponentSignalRef {
                    component: required_uuid(&pin.component, "component")?.into(),
                    signal: required_uuid(&pin.signal, "signal")?,
                },
                net: net
                    .map(|net| required_uuid(&net, "net").map(Into::into))
                    .transpose()?,
            },
        })
    }
}

impl RequirementInput {
    fn resolve(self) -> ToolResult<Requirement> {
        Ok(Requirement {
            id: self.id,
            source: self.source,
            severity: match self.severity {
                SeverityInput::Hard => Severity::Hard,
                SeverityInput::Warning => Severity::Warning,
            },
            check: self.check.resolve()?,
        })
    }
}

fn requirement_json(requirement: &Requirement) -> Value {
    let check = match requirement.check {
        Check::SymbolPosition { symbol, position } => json!({
            "kind":"symbol_position", "symbol":symbol, "position":PointMm::from(position),
        }),
        Check::SymbolOrientation {
            symbol,
            rotation,
            mirrored,
        } => json!({
            "kind":"symbol_orientation", "symbol":symbol, "rotation":deg(rotation), "mirrored":mirrored,
        }),
        Check::SymbolRegion { symbol, min, max } => json!({
            "kind":"symbol_region", "symbol":symbol, "min":PointMm::from(min), "max":PointMm::from(max),
        }),
        Check::PinNet { pin, net } => json!({"kind":"pin_net", "pin":pin, "net":net}),
    };
    json!({"id":requirement.id, "source":requirement.source, "severity":requirement.severity, "check":check})
}

fn measured_json(measured: Measured) -> Value {
    match measured {
        Measured::Position(position) => json!({"position":PointMm::from(position)}),
        Measured::Orientation { rotation, mirrored } => {
            json!({"rotation":deg(rotation),"mirrored":mirrored})
        }
        Measured::Net(net) => json!({"net":net}),
    }
}

fn load(project: &Project) -> ToolResult<Requirements> {
    Ok(Requirements::load(project).map_err(librepcb_editor::Error::from)?)
}

fn document_json(document: &Requirements) -> Value {
    json!({"version":document.version(), "requirements":document.items().iter().map(requirement_json).collect::<Vec<_>>()})
}

pub fn get(session: &Session) -> ToolResult<ToolOutput> {
    let project = session.project()?.project();
    let document = load(project)?;
    ToolOutput::new(
        format!("{} schematic requirements.", document.items().len()),
        json!({"document":document_json(&document), "project":project.uuid(), "revision":project.revision(), "position_unit":"mm"}),
    )
}

pub fn set(session: &mut Session, args: SetArgs) -> ToolResult<ToolOutput> {
    let document = Requirements::new(
        args.document.version,
        args.document
            .requirements
            .into_iter()
            .map(RequirementInput::resolve)
            .collect::<ToolResult<_>>()?,
    )
    .map_err(librepcb_editor::Error::from)?;
    let asset = document.asset().map_err(librepcb_editor::Error::from)?;
    let written = write::write(
        session,
        "requirements_set",
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
        format!("Stored {} schematic requirements.", document.items().len()),
        json!({"document":document_json(&document)}),
    )
}

pub fn check(session: &Session) -> ToolResult<ToolOutput> {
    let project = session.project()?.project();
    let findings = load(project)?.evaluate(project);
    ToolOutput::new(
        format!("Checked {} schematic requirements.", findings.len()),
        evidence(project, &findings, project.revision(), None),
    )
}

pub(super) fn evidence(
    project: &Project,
    findings: &[Finding],
    revision: u64,
    preview: Option<Uuid>,
) -> Value {
    let rows: Vec<_> = findings
        .iter()
        .map(|finding| {
            let mut row = requirement_json(&finding.requirement);
            match finding.observation {
                Observation::Pass(measured) => {
                    row["status"] = json!("pass");
                    row["measured"] = measured_json(measured);
                }
                Observation::Fail(measured) => {
                    row["status"] = json!("fail");
                    row["measured"] = measured_json(measured);
                }
                Observation::Unresolved(reference) => {
                    row["status"] = json!("unresolved");
                    row["missing_reference"] = json!(reference);
                }
            }
            row
        })
        .collect();
    let blockers: Vec<_> = findings
        .iter()
        .filter(|finding| finding.blocks_apply())
        .map(|finding| finding.requirement.id.as_str())
        .collect();
    let artifact = match preview {
        Some(preview) => {
            json!({"kind":"design_preview", "base_revision":revision, "preview":preview})
        }
        None => json!({"kind":"project", "revision":revision}),
    };
    json!({"project":project.uuid(), "artifact":artifact, "position_unit":"mm", "findings":rows, "blocking_requirements":blockers})
}
