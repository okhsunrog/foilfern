//! Explicit copper prepared and validated on a detached routing board.

use std::collections::BTreeSet;

use librepcb_core::project::BoardId;
use librepcb_core::types::{Layer, Point, PositiveLength, Uuid};

use crate::commands::{AddTrace, AddVia, BoardSelection, NetRef, TraceEndpoint, resolve_board};
use crate::{Error, ProjectEditor, Result};

use super::{RoutingError, region};

#[derive(Debug)]
pub enum CopperEndpoint {
    Existing(TraceEndpoint),
    /// Index in the plan's via list, resolved after placement.
    NewVia(usize),
}

impl CopperEndpoint {
    fn resolve(self, vias: &[Uuid]) -> Result<TraceEndpoint> {
        match self {
            Self::Existing(endpoint) => Ok(endpoint),
            Self::NewVia(index) => vias
                .get(index)
                .copied()
                .map(TraceEndpoint::Via)
                .ok_or_else(|| Error::InvalidArgument(format!("Unknown planned via {index}"))),
        }
    }
}

#[derive(Debug)]
pub struct PlannedTrace {
    pub start: CopperEndpoint,
    pub end: CopperEndpoint,
    pub points: Vec<Point>,
    pub layer: Option<Layer>,
    pub width: Option<PositiveLength>,
    pub net: Option<NetRef>,
}

#[derive(Debug)]
pub struct CopperPlan {
    pub board: BoardId,
    pub remove_traces: BTreeSet<Uuid>,
    pub remove_vias: BTreeSet<Uuid>,
    pub vias: Vec<AddVia>,
    pub traces: Vec<PlannedTrace>,
}

impl CopperPlan {
    pub(super) fn run(
        self,
        sandbox: &mut ProjectEditor,
    ) -> std::result::Result<(usize, usize), RoutingError> {
        let removed = self
            .remove_traces
            .union(&self.remove_vias)
            .copied()
            .collect();
        let removal_baseline = if self.remove_traces.is_empty() && self.remove_vias.is_empty() {
            None
        } else {
            let project = sandbox.project();
            let board = resolve_board(project, Some(self.board))?;
            let mut selection = BoardSelection::default();
            let before = region::copper(project, board)?;
            for &uuid in &self.remove_traces {
                let trace = before
                    .get(&uuid)
                    .filter(|item| !item.is_via())
                    .ok_or_else(|| Error::not_found("Trace", uuid))?;
                trace.select_trace(uuid, &mut selection);
            }
            for &uuid in &self.remove_vias {
                let via = before
                    .get(&uuid)
                    .filter(|item| item.is_via())
                    .ok_or_else(|| Error::not_found("Via", uuid))?;
                via.select_via(uuid, board, &self.remove_traces, &mut selection)?;
            }
            sandbox.execute(region::RipUp {
                board: self.board,
                selection,
            })?;
            Some(before)
        };
        let mut vias = Vec::with_capacity(self.vias.len());
        for mut via in self.vias {
            via.board = Some(self.board);
            vias.push(sandbox.execute(via)?.via);
        }
        let mut segments = 0;
        for trace in self.traces {
            let added = sandbox.execute(AddTrace {
                board: Some(self.board),
                start: trace.start.resolve(&vias)?,
                end: trace.end.resolve(&vias)?,
                points: trace.points,
                layer: trace.layer,
                width: trace.width,
                net: trace.net,
            })?;
            segments += added.traces.len();
        }
        if let Some(before) = removal_baseline {
            let project = sandbox.project();
            let after = region::copper(project, resolve_board(project, Some(self.board))?)?;
            region::ensure_preserved_copper(&before, &after, &removed)?;
        }
        Ok((vias.len(), segments))
    }
}
