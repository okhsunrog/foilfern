//! Runs a regional rip-up and routing trial on a detached editor.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_autoroute::RoutingRegion;
use librepcb_core::geometry::{Trace, TraceAnchor, Via};
use librepcb_core::project::board::{Board, ViaView};
use librepcb_core::project::{BoardId, NetSegmentId, NetSignalId, Project};
use librepcb_core::types::{Length, Point, PositiveLength, Uuid};
use serde::Serialize;

use crate::commands::{
    Autoroute, AutorouteResult, BoardSelection, RoutingBounds, RoutingObject, RoutingObjectKind,
    remove_board_items,
};
use crate::editor::{Command, ProjectEditor, Transaction};
use crate::{Error, Result};

use super::RoutingError;

/// A builtin request with a region, explicit target nets and copper to rip up.
#[derive(Debug, Clone, Serialize)]
pub struct RegionalRoutingRequest {
    pub request: Autoroute,
    pub rip_up: Vec<Uuid>,
    /// Optional tighter bounds for copper removal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rip_up_region: Option<RoutingBounds>,
    /// Require progress on this net for the candidate to be applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub improve_net: Option<NetSignalId>,
}

/// One possible repair, anchored in the source copper. It must still be routed and checked.
#[derive(Debug, Serialize)]
pub struct RegionalRepairProposal {
    pub net: NetSignalId,
    pub blockers: Vec<Uuid>,
    pub traces: BTreeSet<Uuid>,
    pub vias: BTreeSet<Uuid>,
    #[serde(skip)]
    pub anchors: Vec<(TraceAnchor, Point)>,
}

impl RegionalRepairProposal {
    /// Expand a sampled blocker along two-trace junctions inside the edit
    /// region. Pads, branching junctions and unselected vias remain anchors.
    pub fn suggest(
        project: &Project,
        board: &Board,
        target: NetSignalId,
        bounds: RoutingBounds,
        blockers: &[RoutingObject],
        editable: &BTreeSet<NetSignalId>,
        max_objects: usize,
    ) -> Result<Vec<Self>> {
        let region = bounds.region()?;
        let items = copper(project, board)?;
        let eligible = |id: &Uuid| {
            items.get(id).filter(|item| {
                item.net.is_some_and(|net| editable.contains(&net)) && item.geometry.fits(region)
            })
        };
        let mut proposals: Vec<Self> = Vec::new();
        for object in blockers {
            let Some(item) = eligible(&object.uuid) else {
                continue;
            };
            let Some(net) = item.net.filter(|net| *net != target) else {
                continue;
            };
            let Some(segment) = board.net_segment(item.segment) else {
                continue;
            };
            let mut vias = BTreeSet::new();
            let mut pending = match object.kind {
                RoutingObjectKind::Trace if !item.is_via() => vec![object.uuid],
                RoutingObjectKind::Via if item.is_via() => {
                    let traces: Vec<_> = segment
                        .traces_at(TraceAnchor::Via(object.uuid))
                        .map(Trace::uuid)
                        .collect();
                    if traces.is_empty() || !traces.iter().all(|id| eligible(id).is_some()) {
                        continue;
                    }
                    vias.insert(object.uuid);
                    traces
                }
                _ => continue,
            };
            let mut traces = BTreeSet::new();
            while let Some(id) = pending.pop() {
                if !traces.insert(id) {
                    continue;
                }
                if traces.len() + vias.len() > max_objects {
                    break;
                }
                let trace = &segment.traces()[&id];
                for anchor in [trace.p1(), trace.p2()] {
                    if !matches!(anchor, TraceAnchor::Junction(_)) {
                        continue;
                    }
                    let mut incident = segment.traces_at(anchor).filter(|t| t.uuid() != id);
                    let neighbor = incident.next();
                    if incident.next().is_none()
                        && let Some(neighbor) = neighbor
                        && !traces.contains(&neighbor.uuid())
                        && eligible(&neighbor.uuid()).is_some()
                    {
                        pending.push(neighbor.uuid());
                    }
                }
            }
            if traces.len() + vias.len() > max_objects {
                continue;
            }
            if let Some(existing) = proposals
                .iter_mut()
                .find(|proposal| proposal.traces == traces && proposal.vias == vias)
            {
                if !existing.blockers.contains(&object.uuid) {
                    existing.blockers.push(object.uuid);
                }
            } else {
                let mut ends = BTreeMap::new();
                for id in &traces {
                    let item = &items[id];
                    let Geometry::Trace { trace, endpoints } = &item.geometry else {
                        continue;
                    };
                    for (anchor, position) in [trace.p1(), trace.p2()].into_iter().zip(endpoints) {
                        let (count, _) = ends.entry(anchor).or_insert((0, *position));
                        *count += 1;
                    }
                }
                let anchors = ends
                    .into_iter()
                    .filter_map(|(anchor, (count, position))| match anchor {
                        TraceAnchor::Via(via) if vias.contains(&via) => None,
                        TraceAnchor::Junction(_) if count == segment.traces_at(anchor).count() => {
                            None
                        }
                        _ => Some((anchor, position)),
                    })
                    .collect();
                proposals.push(Self {
                    net,
                    blockers: vec![object.uuid],
                    traces,
                    vias,
                    anchors,
                });
            }
        }
        proposals.sort_by_key(|proposal| proposal.traces.len() + proposal.vias.len());
        Ok(proposals)
    }
}

#[derive(PartialEq, Eq)]
enum Geometry {
    Trace { trace: Trace, endpoints: [Point; 2] },
    Via { via: Via, diameter: PositiveLength },
}

impl Geometry {
    fn fits(&self, area: RoutingRegion) -> bool {
        let fits = |point, diameter: PositiveLength| {
            let width = diameter.to_nm();
            area.contains(point, Length::new(width / 2 + width % 2))
        };
        match self {
            Self::Trace { trace, endpoints } => endpoints.iter().all(|&p| fits(p, trace.width())),
            Self::Via { via, diameter } => fits(via.position(), *diameter),
        }
    }
}

pub(super) struct Copper {
    segment: NetSegmentId,
    net: Option<NetSignalId>,
    geometry: Geometry,
}

impl Copper {
    pub(super) fn select_via(
        &self,
        uuid: Uuid,
        board: &Board,
        selected_traces: &BTreeSet<Uuid>,
        selection: &mut BoardSelection,
    ) -> Result<()> {
        let segment = board
            .net_segment(self.segment)
            .ok_or_else(|| Error::not_found("Net segment", self.segment))?;
        if segment
            .traces_at(TraceAnchor::Via(uuid))
            .any(|trace| !selected_traces.contains(&trace.uuid()))
        {
            return Err(Error::InvalidArgument(format!(
                "Via {uuid} has an attached trace outside the rip-up list."
            )));
        }
        selection.vias.push((self.segment, uuid));
        Ok(())
    }

    pub(super) fn select_trace(&self, uuid: Uuid, selection: &mut BoardSelection) {
        selection.traces.push((self.segment, uuid));
    }

    pub(super) fn is_via(&self) -> bool {
        matches!(self.geometry, Geometry::Via { .. })
    }
}

pub(super) fn copper(project: &Project, board: &Board) -> Result<BTreeMap<Uuid, Copper>> {
    let mut items = BTreeMap::new();
    for (id, segment) in board.net_segments() {
        let mut record = |uuid: Uuid, geometry| {
            if items
                .insert(
                    uuid,
                    Copper {
                        segment: *id,
                        net: segment.net(),
                        geometry,
                    },
                )
                .is_some()
            {
                return Err(Error::Ambiguous {
                    kind: "Copper object",
                    id: uuid.to_string(),
                });
            }
            Ok(())
        };
        for trace in segment.traces().values() {
            let position = |anchor| {
                board
                    .anchor_position(segment, anchor, project.library(), project.circuit())
                    .ok_or_else(|| Error::not_found("Trace anchor", format!("{anchor:?}")))
            };
            record(
                trace.uuid(),
                Geometry::Trace {
                    trace: trace.clone(),
                    endpoints: [position(trace.p1())?, position(trace.p2())?],
                },
            )?;
        }
        for via in segment.vias().values() {
            let view = ViaView::new(via, board, segment.net(), project.circuit())?;
            record(
                via.uuid(),
                Geometry::Via {
                    via: via.clone(),
                    diameter: view.size(),
                },
            )?;
        }
    }
    Ok(items)
}

pub(super) fn ensure_preserved_copper(
    before: &BTreeMap<Uuid, Copper>,
    after: &BTreeMap<Uuid, Copper>,
    selected: &BTreeSet<Uuid>,
) -> std::result::Result<(), RoutingError> {
    for (uuid, item) in before {
        if !selected.contains(uuid)
            && !after
                .get(uuid)
                .is_some_and(|next| next.net == item.net && next.geometry == item.geometry)
        {
            return Err(RoutingError::Refused(format!(
                "Routing would change protected copper {uuid}."
            )));
        }
    }
    Ok(())
}

pub(super) struct RipUp {
    pub(super) board: BoardId,
    pub(super) selection: BoardSelection,
}

impl Command for RipUp {
    type Output = ();

    fn text(&self) -> String {
        librepcb_i18n::tr!("Autorouter", "Autoroute Board")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        remove_board_items(tx, self.board, &self.selection, false)?;
        Ok(())
    }
}

impl RegionalRoutingRequest {
    pub(super) fn run(
        &self,
        editor: &mut ProjectEditor,
        board_id: BoardId,
    ) -> std::result::Result<AutorouteResult, RoutingError> {
        let bounds = self
            .request
            .region
            .ok_or_else(|| Error::InvalidArgument("Regional routing requires a region.".into()))?;
        let area = bounds.region()?;
        let rip_up_bounds = self.rip_up_region.unwrap_or(bounds);
        if !area.contains(rip_up_bounds.min, Length::ZERO)
            || !area.contains(rip_up_bounds.max, Length::ZERO)
        {
            return Err(Error::InvalidArgument(
                "The rip-up region must lie inside the routing region.".into(),
            )
            .into());
        }
        let rip_up_area = rip_up_bounds.region()?;
        let project = editor.project();
        let board = crate::commands::resolve_board(project, Some(board_id))?;
        let targets: BTreeSet<_> = self
            .request
            .nets
            .iter()
            .flatten()
            .map(|net| crate::commands::resolve_net(project, net))
            .collect::<Result<_>>()?;
        if self.improve_net.is_some_and(|net| !targets.contains(&net)) {
            return Err(Error::InvalidArgument("improve_net must be a target net.".into()).into());
        }
        let selected: BTreeSet<_> = self.rip_up.iter().copied().collect();
        let before = copper(project, board)?;
        let mut selection = BoardSelection::default();
        for uuid in &selected {
            let item = before
                .get(uuid)
                .ok_or_else(|| Error::not_found("Trace or via", uuid))?;
            if !item.net.is_some_and(|net| targets.contains(&net)) {
                return Err(Error::InvalidArgument(format!(
                    "Copper {uuid} must belong to a target net."
                ))
                .into());
            }
            if !item.geometry.fits(rip_up_area) {
                let region = if self.rip_up_region.is_some() {
                    "rip-up region"
                } else {
                    "routing region"
                };
                return Err(Error::InvalidArgument(format!(
                    "Copper {uuid} extends outside the {region}."
                ))
                .into());
            }
            if item.is_via() {
                item.select_via(*uuid, board, &selected, &mut selection)?;
            } else {
                item.select_trace(*uuid, &mut selection);
            }
        }
        if !selected.is_empty() {
            editor.execute(RipUp {
                board: board_id,
                selection,
            })?;
            editor
                .update_derived_data(|p| p.rebuild_planes(board_id, None))?
                .map_err(Error::from_export)?;
        }
        let mut request = self.request.clone();
        request.board = Some(board_id);
        let result = editor.execute(request)?;
        let project = editor.project();
        let board = crate::commands::resolve_board(project, Some(board_id))?;
        let after = copper(project, board)?;
        ensure_preserved_copper(&before, &after, &selected)?;
        for (uuid, item) in after {
            if (!before.contains_key(&uuid) || selected.contains(&uuid))
                && !item.geometry.fits(area)
            {
                return Err(RoutingError::Refused(format!(
                    "Routed copper {uuid} extends outside the region."
                )));
            }
        }
        Ok(result)
    }
}
