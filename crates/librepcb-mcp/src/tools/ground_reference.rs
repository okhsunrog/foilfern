//! Read-only ground coverage and before/after checks of frozen routing patches.

use librepcb_core::project::board::{ReferenceCoverage, ReferenceGap, ReferenceSelection};
use librepcb_core::project::{BoardId, Mutation, Project};
use librepcb_core::types::Uuid;
use librepcb_editor::ProjectEditor;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{board_edit, write};
use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::resolve;
use crate::session::Session;
use crate::views;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReferenceArgs {
    /// Signal net names or UUIDs. Only traces adjacent to the reference layer are measured.
    pub signal_nets: Vec<String>,
    /// Net name or UUID of the reference pour, e.g. GND.
    pub reference_net: String,
    /// Copper layer of the reference pour, e.g. bottom or inner1.
    pub reference_layer: String,
    /// Fragment interval offset within each net's gap list.
    #[serde(default)]
    pub gap_offset: usize,
    /// Max intervals per gap list (1..200, default 64). Totals cover the entire list.
    pub gap_limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReferenceCoverageArgs {
    pub reference: ReferenceArgs,
    /// Board name, index or UUID (default: first board).
    pub board: Option<String>,
    pub expected_revision: Option<u64>,
}

pub struct ReferenceJob {
    snapshot: ProjectEditor,
    project: Uuid,
    revision: u64,
    board: BoardId,
    selection: ReferenceSelection,
    offset: usize,
    limit: usize,
}

#[derive(Serialize)]
struct GapList {
    items: Vec<Value>,
    count: usize,
    length_mm: f64,
    next_offset: Option<usize>,
}

impl ReferenceJob {
    pub(super) fn new(
        source: &Project,
        snapshot: ProjectEditor,
        board: BoardId,
        args: ReferenceArgs,
    ) -> ToolResult<Self> {
        let (reference_net, _) = resolve::net(source, &args.reference_net)?;
        let reference_layer = board_edit::copper_layer(&args.reference_layer)?;
        if !source
            .board(board)
            .is_some_and(|board| board.copper_layers().contains(&reference_layer))
        {
            return Err(ToolError::invalid(
                "Reference layer is disabled on this board.",
            ));
        }
        let signal_nets = args
            .signal_nets
            .iter()
            .map(|key| resolve::net(source, key).map(|(net, _)| net))
            .collect::<ToolResult<_>>()?;
        if args.signal_nets.is_empty() {
            return Err(ToolError::invalid("Select at least one signal net."));
        }
        let limit = args.gap_limit.unwrap_or(64);
        if !(1..=200).contains(&limit) {
            return Err(ToolError::invalid("gap_limit must be between 1 and 200."));
        }
        Ok(Self {
            snapshot,
            project: source.uuid(),
            revision: source.revision(),
            board,
            selection: ReferenceSelection {
                signal_nets,
                reference_net,
                reference_layer,
            },
            offset: args.gap_offset,
            limit,
        })
    }

    fn measure(&self) -> ToolResult<ReferenceCoverage> {
        Ok(self
            .snapshot
            .project()
            .reference_coverage(self.board, &self.selection)?)
    }

    fn selection_json(&self) -> Value {
        json!({
            "project": self.project, "base_revision": self.revision, "board": self.board,
            "reference_net": resolve::net_name(self.snapshot.project(), self.selection.reference_net),
            "reference_layer": self.selection.reference_layer.id(),
        })
    }

    fn gap_list(&self, gaps: &[ReferenceGap]) -> GapList {
        let items: Vec<_> = gaps
            .iter()
            .skip(self.offset)
            .take(self.limit)
            .map(|gap| {
                json!({
                    "trace": gap.trace, "layer": gap.layer.id(),
                    "from": views::point(gap.from), "to": views::point(gap.to),
                    "length_mm": gap.length_mm(),
                })
            })
            .collect();
        let next = self.offset.saturating_add(items.len());
        GapList {
            items,
            count: gaps.len(),
            length_mm: gaps.iter().fold(0.0, |total, gap| total + gap.length_mm()),
            next_offset: (next < gaps.len()).then_some(next),
        }
    }

    fn coverage_json(&self, coverage: &ReferenceCoverage) -> Value {
        let nets: Vec<_> = coverage
            .nets
            .iter()
            .map(|(id, net)| {
                let unchecked: Vec<_> = net
                    .unchecked_layers
                    .iter()
                    .map(|(layer, count)| json!({"layer": layer.id(), "trace_count": count}))
                    .collect();
                let gaps = self.gap_list(&net.gaps);
                let uncovered = gaps.length_mm;
                json!({
                    "net": resolve::net_name(self.snapshot.project(), *id), "net_uuid": id,
                    "checked_traces": net.checked_traces, "checked_mm": net.checked_mm,
                    "unchecked_layers": unchecked,
                    "covered_fraction": (net.checked_mm > 0.0)
                        .then(|| (1.0 - uncovered / net.checked_mm).clamp(0.0, 1.0)),
                    "gaps": gaps,
                })
            })
            .collect();
        json!({"connected_reference_fragments": coverage.connected_fragments, "nets": nets})
    }

    pub(super) fn compare(mut self, patch: Vec<Mutation>) -> ToolResult<Value> {
        let before = self.measure()?;
        self.snapshot
            .apply_mutations("Preview reference coverage", patch)?;
        let after = self.measure()?;
        let changes: Vec<_> = before
            .nets
            .iter()
            .map(|(id, prior)| {
                let next = &after.nets[id];
                let diff = |a: &[ReferenceGap], b: &[ReferenceGap]| {
                    let gaps: Vec<_> = a.iter().flat_map(|gap| gap.difference(b)).collect();
                    self.gap_list(&gaps)
                };
                json!({
                    "net": resolve::net_name(self.snapshot.project(), *id), "net_uuid": id,
                    "resolved_gaps": diff(&prior.gaps, &next.gaps),
                    "new_gaps": diff(&next.gaps, &prior.gaps),
                })
            })
            .collect();
        let mut result = self.selection_json();
        result["before"] = self.coverage_json(&before);
        result["after"] = self.coverage_json(&after);
        result["changes"] = json!(changes);
        Ok(result)
    }
}

pub fn prepare(session: &Session, args: ReferenceCoverageArgs) -> ToolResult<ReferenceJob> {
    let source = session.project()?.project();
    write::check_revision(source, args.expected_revision)?;
    let board = board_edit::board_id(source, args.board.as_deref())?;
    ReferenceJob::new(
        source,
        ProjectEditor::new(source.snapshot()?),
        board,
        args.reference,
    )
}

pub fn run(job: ReferenceJob) -> ToolResult<ToolOutput> {
    let mut result = job.selection_json();
    result["coverage"] = job.coverage_json(&job.measure()?);
    ToolOutput::new(
        "Measured signal centerline coverage over the selected reference pour.",
        result,
    )
}

pub fn finish(session: &Session, report: ToolOutput) -> ToolResult<ToolOutput> {
    let project = session.project()?.project();
    if report.result["project"] != json!(project.uuid()) {
        return Err(ToolError::new(
            ErrorKind::StaleRevision,
            "The open project changed during reference analysis.",
        ));
    }
    write::check_revision(project, report.result["base_revision"].as_u64())?;
    Ok(report)
}
