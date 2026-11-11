//! Explicit copper prepared and validated on a detached routing board.

use librepcb_core::project::BoardId;
use librepcb_core::types::{Layer, Point, PositiveLength, Uuid};

use crate::commands::{AddTrace, AddVia, NetRef, TraceEndpoint};
use crate::{Error, ProjectEditor, Result};

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
    pub vias: Vec<AddVia>,
    pub traces: Vec<PlannedTrace>,
}

impl CopperPlan {
    pub(super) fn run(self, sandbox: &mut ProjectEditor) -> Result<(usize, usize)> {
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
        Ok((vias.len(), segments))
    }
}
