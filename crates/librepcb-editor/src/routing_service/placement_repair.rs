//! Route explicitly authorized branches after a detached placement edit.

use std::collections::BTreeSet;

use librepcb_core::project::{BoardId, Mutation, NetSignalId, Project};
use librepcb_core::types::{Length, Uuid};

use crate::commands::{Autoroute, AutorouteResult, NetRef, RoutingBounds, resolve_board};
use crate::{Error, ProjectEditor};

use super::region::{self, RegionalRoutingRequest};
use super::{RoutingError, freeze_mutations};

pub struct PlacementRepair {
    pub region: RoutingBounds,
    pub edit_region: RoutingBounds,
    pub editable_nets: BTreeSet<NetSignalId>,
    pub branches: Vec<BTreeSet<Uuid>>,
    pub route: Autoroute,
}

pub struct PlacementRepairTrial {
    pub project: Project,
    pub mutations: Vec<Mutation>,
    pub routing: AutorouteResult,
}

impl PlacementRepair {
    /// Check authorizations against source copper and reconnect the same
    /// anchors on the staged board, using the moved pads' final positions.
    pub fn run(
        &self,
        source: &Project,
        placed: &Project,
        board: BoardId,
        cancelled: impl Fn() -> bool,
    ) -> Result<PlacementRepairTrial, RoutingError> {
        let original = resolve_board(source, Some(board))?;
        let staged = resolve_board(placed, Some(board))?;
        let selected: BTreeSet<_> = self.branches.iter().flatten().copied().collect();
        if selected.len() != self.branches.iter().map(BTreeSet::len).sum::<usize>() {
            return Err(
                Error::InvalidArgument("Authorized branches must be disjoint.".into()).into(),
            );
        }
        let before = region::copper_matching(source, original, |id| selected.contains(&id))?;
        let after = region::copper_matching(placed, staged, |id| selected.contains(&id))?;
        let edit_area = self.edit_region.region()?;
        let route_area = self.region.region()?;
        if ![self.edit_region.min, self.edit_region.max]
            .into_iter()
            .all(|point| route_area.contains(point, Length::ZERO))
        {
            return Err(Error::InvalidArgument(
                "The edit region must lie inside the routing region.".into(),
            )
            .into());
        }
        region::check_rip_up_permissions(source, board, &selected)?;
        let mut branches = Vec::with_capacity(self.branches.len());
        let mut nets = BTreeSet::new();
        for group in &self.branches {
            let invalid_branch = || {
                Error::InvalidArgument(format!(
                    "Authorized copper {group:?} must form one branch between two retained anchors."
                ))
            };
            for id in group {
                let original = before
                    .get(id)
                    .ok_or_else(|| Error::not_found("Trace or via", id))?;
                if !original
                    .net()
                    .is_some_and(|net| self.editable_nets.contains(&net))
                {
                    return Err(Error::InvalidArgument(format!(
                        "Copper {id} is outside editable_nets."
                    ))
                    .into());
                }
                if !original.fits(edit_area) {
                    return Err(Error::InvalidArgument(format!(
                        "Copper {id} extends outside the edit region."
                    ))
                    .into());
                }
                if !after.contains_key(id) {
                    return Err(Error::InvalidArgument(format!(
                        "Copper {id} is missing after placement."
                    ))
                    .into());
                }
            }
            let first = group.first().ok_or_else(invalid_branch)?;
            let old_segment = original
                .net_segment(before[first].segment())
                .ok_or_else(invalid_branch)?;
            let anchors = region::branch_anchors(&before, old_segment, group);
            let [(a, _), (b, _)] = anchors.as_slice() else {
                return Err(invalid_branch().into());
            };
            let ends = [*a, *b];
            let source_branch = region::retained_branch(&before, original, group, ends)
                .ok_or_else(invalid_branch)?;
            let branch = region::retained_branch(&after, staged, group, ends)
                .filter(|wire| wire.net() == source_branch.net())
                .ok_or_else(invalid_branch)?;
            if ![branch.p1_position(), branch.p2_position()]
                .into_iter()
                .all(|point| route_area.contains(point, Length::ZERO))
            {
                return Err(Error::InvalidArgument(
                    "Both final branch anchors must be inside the routing region.".into(),
                )
                .into());
            }
            nets.insert(branch.net());
            branches.push(branch);
        }
        let mut sandbox = ProjectEditor::new(placed.snapshot().map_err(Error::from)?);
        let request = RegionalRoutingRequest {
            request: Autoroute {
                region: Some(self.region),
                nets: Some(nets.into_iter().map(NetRef::Id).collect()),
                ..self.route.clone()
            },
            rip_up: selected.into_iter().collect(),
            // Original copper was checked against the tighter edit bounds.
            rip_up_region: None,
            improve_net: None,
        };
        let routing = request.run_with_staging(
            &mut sandbox,
            board,
            None,
            &branches,
            cancelled,
            |_| Ok(()),
        )?;
        let project = sandbox.project().snapshot().map_err(Error::from)?;
        let mutations = freeze_mutations(sandbox)?;
        Ok(PlacementRepairTrial {
            project,
            mutations,
            routing,
        })
    }
}
