//! Runs a regional rip-up and routing trial on a detached editor.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_autoroute::RoutingRegion;
use librepcb_core::geometry::{Trace, TraceAnchor, Via};
use librepcb_core::project::board::{Board, ViaView};
use librepcb_core::project::{BoardId, NetSegmentId, NetSignalId, Project};
use librepcb_core::types::{Length, Point, PositiveLength, Uuid};
use serde::Serialize;

use crate::commands::{Autoroute, AutorouteResult, BoardSelection, remove_board_items};
use crate::editor::{Command, ProjectEditor, Transaction};
use crate::{Error, Result};

use super::RoutingError;

/// A builtin request with a region, explicit target nets and copper to rip up.
#[derive(Debug, Clone, Serialize)]
pub struct RegionalRoutingRequest {
    pub request: Autoroute,
    pub rip_up: Vec<Uuid>,
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
        let area = self
            .request
            .region
            .ok_or_else(|| Error::InvalidArgument("Regional routing requires a region.".into()))?
            .region()?;
        let project = editor.project();
        let board = crate::commands::resolve_board(project, Some(board_id))?;
        let targets: BTreeSet<_> = self
            .request
            .nets
            .iter()
            .flatten()
            .map(|net| crate::commands::resolve_net(project, net))
            .collect::<Result<_>>()?;
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
            if !item.geometry.fits(area) {
                return Err(Error::InvalidArgument(format!(
                    "Copper {uuid} extends outside the routing region."
                ))
                .into());
            }
            match &item.geometry {
                Geometry::Trace { .. } => selection.traces.push((item.segment, *uuid)),
                Geometry::Via { .. } => {
                    let segment = board
                        .net_segment(item.segment)
                        .ok_or_else(|| Error::not_found("Net segment", item.segment))?;
                    if segment
                        .traces_at(TraceAnchor::Via(*uuid))
                        .any(|trace| !selected.contains(&trace.uuid()))
                    {
                        return Err(Error::InvalidArgument(format!(
                            "Via {uuid} has an attached trace outside the rip-up list."
                        ))
                        .into());
                    }
                    selection.vias.push((item.segment, *uuid));
                }
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
