//! Restore copper discarded by a plane's island filter using a via and lead.

use super::*;
use foilfern_core::project::board::{PlaneFragmentDiagnostic, PlaneFragmentStatus};

#[derive(Debug, Clone, Serialize)]
pub struct StitchPlaneIsland {
    pub board: BoardId,
    pub plane: PlaneId,
    pub from: TraceEndpoint,
    pub from_layer: Layer,
    pub position: Point,
    pub points: Vec<Point>,
    pub trace_width: Option<PositiveLength>,
    pub via_drill: Option<PositiveLength>,
    pub via_size: Option<PositiveLength>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlaneStitch {
    pub net: NetSignalId,
    pub net_name: String,
    pub plane_layer: Layer,
    pub via: Uuid,
    pub traces: Vec<Uuid>,
    pub island_area_before_mm2: f64,
    pub connected_fragment_area_after_mm2: f64,
}

impl StitchPlaneIsland {
    fn fragment_at(
        &self,
        project: &Project,
        status: PlaneFragmentStatus,
        missing: &str,
    ) -> Result<PlaneFragmentDiagnostic> {
        project
            .plane_job(self.board, None)
            .map_err(Error::from_export)?
            .ok_or_else(|| Error::not_found("Plane", self.plane))?
            .diagnose(self.plane)
            .map_err(Error::from_export)?
            .into_iter()
            .find(|fragment| fragment.status == status && fragment.contains(self.position))
            .ok_or_else(|| Error::InvalidArgument(missing.into()))
    }
}

impl Command for StitchPlaneIsland {
    type Output = PlaneStitch;

    fn text(&self) -> String {
        "Stitch copper plane island".into()
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Self::Output> {
        if matches!(
            self.from,
            TraceEndpoint::Point(_) | TraceEndpoint::Trace { .. }
        ) {
            return Err(Error::InvalidArgument(
                "The stitch must start at an existing pad, via or junction.".into(),
            ));
        }
        let start = TraceEnd::resolve(tx, self.board, &self.from)?;
        let project = tx.project();
        let board = resolve::board(project, Some(self.board))?;
        let plane = board
            .plane(self.plane)
            .ok_or_else(|| Error::not_found("Plane", self.plane))?;
        let plane_layer = plane.layer();
        let net = plane
            .net()
            .filter(|net| Some(*net) == start.net.flatten())
            .ok_or_else(|| {
                Error::InvalidArgument("The source and plane must share a net.".into())
            })?;
        if self.from_layer == plane_layer || !board.copper_layers().contains(&self.from_layer) {
            return Err(Error::InvalidArgument(
                "The lead needs an enabled copper layer different from the plane layer.".into(),
            ));
        }
        let segment = start.segment.and_then(|id| board.net_segment(id));
        let on_layer = match start.anchor {
            Some(TraceAnchor::FootprintPad { .. }) => {
                start.smt_layer.is_none_or(|l| l == self.from_layer)
            }
            Some(TraceAnchor::Pad(id)) => segment
                .and_then(|s| s.pads().get(&id))
                .is_some_and(|pad| pad.pad().is_on_layer(self.from_layer)),
            Some(TraceAnchor::Via(id)) => segment
                .and_then(|s| s.vias().get(&id))
                .is_some_and(|via| via.is_on_layer(self.from_layer)),
            Some(TraceAnchor::Junction(id)) => {
                segment.and_then(|s| s.junction_layer(&id)) == Some(self.from_layer)
            }
            None => false,
        };
        if !on_layer {
            return Err(Error::InvalidArgument(
                "The source anchor is not on the lead layer.".into(),
            ));
        }
        let net_name = resolve::net_name(project, Some(net));
        let island = self.fragment_at(
            project,
            PlaneFragmentStatus::IslandDiscarded,
            "The via center is not inside a discarded fragment of this plane.",
        )?;
        let via = tx.run(AddVia {
            board: Some(self.board),
            position: self.position,
            net: Some(NetRef::Id(net)),
            start_layer: None,
            end_layer: None,
            drill_diameter: self.via_drill,
            size: self.via_size,
            exposure: None,
        })?;
        let lead = tx.run(AddTrace {
            board: Some(self.board),
            start: self.from.clone(),
            end: TraceEndpoint::Via(via.via),
            points: self.points.clone(),
            layer: Some(self.from_layer),
            width: self.trace_width,
            net: Some(NetRef::Id(net)),
        })?;
        if lead.traces.is_empty() {
            return Err(Error::InvalidArgument(
                "The stitch did not create a lead.".into(),
            ));
        }
        let connected = self.fragment_at(
            tx.project(),
            PlaneFragmentStatus::Connected,
            "The rebuilt plane did not connect to the proposed via.",
        )?;
        Ok(PlaneStitch {
            net,
            net_name,
            plane_layer,
            via: via.via,
            traces: lead.traces,
            island_area_before_mm2: island.area_mm2(),
            connected_fragment_area_after_mm2: connected.area_mm2(),
        })
    }
}
