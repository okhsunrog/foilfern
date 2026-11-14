//! Proposal and authorization for replacing branches disturbed by a placement.

use std::collections::{BTreeMap, BTreeSet};

use foilfern_core::project::board_requirements::Requirements;
use foilfern_core::project::{BoardId, Project};
use foilfern_core::types::{Length, Uuid};
use foilfern_editor::commands::autoroute::{RoutingObject, RoutingObjectKind};
use foilfern_editor::routing_service::{
    PlacementRepair, PlacementRepairTrial, RegionalRepairProposal,
};
use foilfern_editor::routing_validation::RoutingValidation;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ToolError, ToolResult};
use crate::resolve;
use crate::tools::layout;
use crate::tools::routing::{RoutingEditRouteArgs, RoutingRegionArgs, routing_error};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tools::design) struct RepairInput {
    /// Bounds for new copper in board millimeters.
    region: RoutingRegionArgs,
    /// Smaller rectangle for original copper eligible for removal.
    edit_region: Option<RoutingRegionArgs>,
    /// Nets eligible for suggestions and authorization.
    editable_nets: Vec<String>,
    /// Exact trace and via UUIDs per branch, copied or adapted from the proposals.
    #[serde(default)]
    branches: Vec<Vec<String>>,
    /// Base revision of the reviewed placement. Required when authorizing branches.
    expected_revision: Option<u64>,
    /// Optional builtin settings. Nets are derived from the authorized branches.
    route: Option<RoutingEditRouteArgs>,
}

pub(super) struct Repair {
    plan: PlacementRepair,
}

impl RepairInput {
    pub(super) fn resolve(
        self,
        source: &Project,
        board: BoardId,
        revision: u64,
        moved: bool,
    ) -> ToolResult<Repair> {
        if self.editable_nets.is_empty() || self.editable_nets.len() > 16 {
            return Err(ToolError::invalid("Specify 1–16 editable nets."));
        }
        if self.branches.len() > 8
            || self
                .branches
                .iter()
                .any(|ids| !(1..=16).contains(&ids.len()))
        {
            return Err(ToolError::invalid(
                "Supply up to eight branches, each with 1–16 copper UUIDs.",
            ));
        }
        if !self.branches.is_empty() && self.expected_revision.is_none() {
            return Err(ToolError::invalid(
                "Authorizing branches requires expected_revision from the placement preview.",
            ));
        }
        if moved
            && let Some(expected) = self.expected_revision
            && expected != revision
        {
            return Err(ToolError::new(
                crate::error::ErrorKind::StaleRevision,
                format!("Expected revision {expected}, current revision is {revision}."),
            ));
        }
        let editable = self
            .editable_nets
            .iter()
            .map(|name| resolve::net(source, name).map(|(id, _)| id))
            .collect::<ToolResult<_>>()?;
        let region = self.region.bounds()?;
        let edit_region = self
            .edit_region
            .as_ref()
            .map(RoutingRegionArgs::bounds)
            .transpose()?
            .unwrap_or(region);
        let area = region.region()?;
        if ![edit_region.min, edit_region.max]
            .into_iter()
            .all(|point| area.contains(point, Length::ZERO))
        {
            return Err(ToolError::invalid("edit_region must lie inside region."));
        }
        let mut seen = BTreeSet::new();
        let branches = self
            .branches
            .into_iter()
            .map(|ids| {
                ids.into_iter()
                    .map(|id| {
                        let id = resolve::required_uuid(&id, "trace or via")?;
                        if !seen.insert(id) {
                            return Err(ToolError::invalid(format!(
                                "Duplicate repair object {id}."
                            )));
                        }
                        Ok(id)
                    })
                    .collect::<ToolResult<_>>()
            })
            .collect::<ToolResult<_>>()?;
        if self
            .route
            .as_ref()
            .is_some_and(|route| !route.nets.is_empty())
        {
            return Err(ToolError::invalid(
                "Repair derives route nets from the authorized branches.",
            ));
        }
        let route = self
            .route
            .map(|route| route.command(source, board))
            .transpose()?
            .unwrap_or_default();
        Ok(Repair {
            plan: PlacementRepair {
                region,
                edit_region,
                editable_nets: editable,
                branches,
                route,
            },
        })
    }
}

impl Repair {
    pub(super) fn authorized(&self) -> bool {
        !self.plan.branches.is_empty()
    }

    pub(super) fn run(
        &self,
        source: &Project,
        proposed: &Project,
        id: BoardId,
        cancelled: impl Fn() -> bool,
    ) -> ToolResult<PlacementRepairTrial> {
        self.plan
            .run(source, proposed, id, cancelled)
            .map_err(routing_error)
    }

    pub(super) fn proposals(
        &self,
        project: &Project,
        placed: &Project,
        id: BoardId,
        revision: u64,
        moved: &[Value],
        validation: &RoutingValidation,
    ) -> ToolResult<Value> {
        let board = super::board(project, id)?;
        let mut seen = BTreeSet::new();
        let mut objects: Vec<_> = validation
            .new_errors
            .iter()
            .flat_map(|error| &error.objects)
            .filter(|object| {
                matches!(
                    object.kind,
                    RoutingObjectKind::Trace | RoutingObjectKind::Via
                )
            })
            .filter(|object| seen.insert(object.uuid))
            .copied()
            .collect();
        let conflicts = seen.clone();
        objects.extend(moved.iter().filter_map(|trace| {
            let uuid = trace["uuid"].as_str()?.parse().ok()?;
            seen.insert(uuid).then_some(RoutingObject {
                kind: RoutingObjectKind::Trace,
                uuid,
                device_uuid: None,
                pad_uuid: None,
            })
        }));
        let mut proposals = RegionalRepairProposal::for_placement(
            project,
            board,
            self.plan.edit_region,
            &objects,
            &self.plan.editable_nets,
        )?;
        let proposed_rules = Requirements::load(placed).map_err(foilfern_editor::Error::from)?;
        proposals.retain(|p| {
            p.traces
                .union(&p.vias)
                .all(|id_| !proposed_rules.protects(id, *id_))
        });
        proposals
            .sort_by_key(|proposal| !proposal.blockers.iter().any(|id| conflicts.contains(id)));
        let covered: BTreeSet<_> = proposals
            .iter()
            .take(8)
            .flat_map(|proposal| proposal.blockers.iter().copied())
            .collect();
        let sources: BTreeMap<_, _> = board
            .net_segments()
            .values()
            .flat_map(|segment| {
                segment
                    .traces()
                    .keys()
                    .chain(segment.vias().keys())
                    .map(move |&id| (id, segment.net()))
            })
            .filter(|(uuid, _)| conflicts.contains(uuid))
            .collect();
        let mut unresolved = Vec::new();
        for object in &objects {
            if !conflicts.contains(&object.uuid) || covered.contains(&object.uuid) {
                continue;
            }
            let net = sources.get(&object.uuid).copied().flatten();
            let reason = if !sources.contains_key(&object.uuid) {
                "not_source_copper"
            } else if proposed_rules.protects(id, object.uuid) {
                "protected"
            } else if !net.is_some_and(|net| self.plan.editable_nets.contains(&net)) {
                "outside_editable_nets"
            } else {
                "no_branch_in_scope"
            };
            unresolved.push(json!({"object":object,"net":net,
                "net_name":net.map(|net| resolve::net_name(project, net)), "reason":reason}));
        }
        let proposals = proposals
            .into_iter()
            .take(8)
            .map(|proposal| {
                let anchors: Vec<_> = proposal
                    .anchors
                    .iter()
                    .map(|&(anchor, position)| {
                        layout::trace_anchor(project, board, Some(position), anchor)
                    })
                    .collect();
                json!({
                    "net":proposal.net, "net_name":resolve::net_name(project, proposal.net),
                    "blockers":proposal.blockers, "retained_anchors":anchors,
                    "branch":proposal.traces.union(&proposal.vias).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        Ok(
            json!({"status":"review", "expected_revision":revision, "proposals":proposals,
            "unresolved_blockers":unresolved}),
        )
    }

    pub(super) fn trial_evidence(
        &self,
        before: &Project,
        after: &Project,
        id: BoardId,
        trial: &PlacementRepairTrial,
    ) -> ToolResult<Value> {
        let prior = super::board(before, id)?;
        let selected: BTreeSet<_> = self.plan.branches.iter().flatten().copied().collect();
        let original: BTreeSet<Uuid> = prior
            .net_segments()
            .values()
            .flat_map(|segment| {
                segment
                    .traces()
                    .keys()
                    .chain(segment.vias().keys())
                    .copied()
            })
            .collect();
        let board = super::board(after, id)?;
        let mut added = BTreeMap::new();
        for segment in board.net_segments().values() {
            for trace in segment
                .traces()
                .values()
                .filter(|trace| !original.contains(&trace.uuid()))
            {
                let anchor = |anchor| {
                    let position =
                        board.anchor_position(segment, anchor, after.library(), after.circuit());
                    layout::trace_anchor(after, board, position, anchor)
                };
                added.insert(trace.uuid(), json!({
                    "kind":"trace", "uuid":trace.uuid(), "net":segment.net(), "layer":trace.layer(),
                    "width_mm":trace.width().to_mm(), "from":anchor(trace.p1()), "to":anchor(trace.p2()),
                }));
            }
            for via in segment
                .vias()
                .values()
                .filter(|via| !original.contains(&via.uuid()))
            {
                added.insert(via.uuid(), json!({"kind":"via","uuid":via.uuid(),"net":segment.net(),
                    "position":crate::views::point(via.position()), "drill_mm":via.drill_diameter().map(|d| d.to_mm()),
                    "size_mm":via.size().map(|d| d.to_mm()),
                    "start_layer":via.start_layer(), "end_layer":via.end_layer()}));
            }
        }
        Ok(json!({"status":"trial","removed":selected,
            "added":added.into_values().collect::<Vec<_>>(),"routing":trial.routing}))
    }
}
