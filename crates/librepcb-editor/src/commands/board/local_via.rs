//! Connect two existing anchors using a geometrically checked local via.

use super::*;
use crate::commands::autoroute::{Autoroute, extract_problem};
use librepcb_autoroute::{LocalViaSite, find_local_via};

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConnectLocalVia {
    pub board: BoardId,
    pub from: TraceEndpoint,
    pub from_layer: Layer,
    pub to: TraceEndpoint,
    pub to_layer: Layer,
    pub position: Option<Point>,
    pub search_radius: PositiveLength,
    pub trace_width: Option<PositiveLength>,
    pub via_drill: Option<PositiveLength>,
    pub via_size: Option<PositiveLength>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LocalViaConnection {
    pub net: NetSignalId,
    pub net_name: String,
    pub via: Uuid,
    pub position: Point,
    pub elbows: [Option<Point>; 2],
    pub trace_width_nm: i64,
    pub via_drill_nm: i64,
    pub via_size_nm: i64,
}

impl Command for ConnectLocalVia {
    type Output = LocalViaConnection;

    fn text(&self) -> String {
        librepcb_i18n::tr!("Autorouter", "Connect anchors through a local via")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Self::Output> {
        let endpoints = [&self.from, &self.to];
        for endpoint in endpoints {
            if !matches!(
                endpoint,
                TraceEndpoint::Pad(_)
                    | TraceEndpoint::FootprintPad { .. }
                    | TraceEndpoint::Via(_)
                    | TraceEndpoint::Junction(_)
            ) {
                return Err(Error::InvalidArgument(
                    "Local via endpoints must be existing footprint pads, vias or junctions."
                        .into(),
                ));
            }
        }
        let from = TraceEnd::resolve(tx, self.board, &self.from)?;
        let to = TraceEnd::resolve(tx, self.board, &self.to)?;
        let net = from
            .net
            .flatten()
            .filter(|net| Some(*net) == to.net.flatten())
            .ok_or_else(|| {
                Error::InvalidArgument("Local via endpoints must share a net.".into())
            })?;
        let p = tx.project();
        let board = resolve::board(p, Some(self.board))?;
        let enabled_layers = board.copper_layers();
        if self.from_layer == self.to_layer {
            return Err(Error::InvalidArgument(
                "Local via endpoints need distinct copper layers.".into(),
            ));
        }
        for (end, layer) in [(from, self.from_layer), (to, self.to_layer)] {
            let segment = end.segment.and_then(|id| board.net_segment(id));
            let on_layer = match end.anchor {
                Some(TraceAnchor::FootprintPad { .. }) => end.smt_layer.is_none_or(|l| l == layer),
                Some(TraceAnchor::Via(id)) => segment
                    .and_then(|s| s.vias().get(&id))
                    .is_some_and(|via| via.is_on_layer(layer)),
                Some(TraceAnchor::Junction(id)) => {
                    segment.and_then(|s| s.junction_layer(&id)) == Some(layer)
                }
                _ => false,
            };
            if !enabled_layers.contains(&layer) || !on_layer {
                return Err(Error::InvalidArgument(format!(
                    "The endpoint is not on {}.",
                    layer.id()
                )));
            }
        }
        let options = Autoroute {
            board: Some(self.board),
            trace_width: self.trace_width,
            via_drill: self.via_drill,
            via_size: self.via_size,
            layers: Some(vec![self.from_layer, self.to_layer]),
            ..Default::default()
        };
        let extracted =
            extract_problem(p, self.board, &options, Some(&[net].into_iter().collect()))?;
        let net_name = resolve::net_name(p, Some(net));
        let rules = &extracted.problem.rules;
        if rules.no_vias.contains(&net.0)
            || rules.net_layers.get(&net.0).is_some_and(|layers| {
                !layers.contains(&self.from_layer) || !layers.contains(&self.to_layer)
            })
        {
            return Err(Error::BoardRequirementsBlocked(format!(
                "saved layer or via restriction for {net_name}"
            )));
        }
        let site = self.position.map_or(
            LocalViaSite::Search(self.search_radius),
            LocalViaSite::Exact,
        );
        let bridge = find_local_via(
            &extracted.problem,
            net.0,
            [
                (from.position, self.from_layer),
                (to.position, self.to_layer),
            ],
            site,
        )?
        .ok_or_else(|| {
            Error::InvalidArgument(
                "No clear via and short leads for the selected anchors and site.".into(),
            )
        })?;
        let (drill, size) = extracted.via_sizes[&net];
        let via = Via::new(
            Uuid::new_random(),
            Layer::TOP_COPPER,
            Layer::BOT_COPPER,
            bridge.position,
            drill,
            size,
            MaskConfig::Off,
        )
        .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        let added = add_via_connected(tx, self.board, via, Some(net))?;
        for ((start, layer), elbow) in [(self.from, self.from_layer), (self.to, self.to_layer)]
            .into_iter()
            .zip(bridge.elbows)
        {
            tx.run(AddTrace {
                board: Some(self.board),
                start,
                end: TraceEndpoint::Via(added.via),
                points: elbow.into_iter().collect(),
                layer: Some(layer),
                width: Some(bridge.width),
                net: Some(NetRef::Id(net)),
            })?;
        }
        Ok(LocalViaConnection {
            net,
            net_name,
            via: added.via,
            position: bridge.position,
            elbows: bridge.elbows,
            trace_width_nm: bridge.width.to_nm(),
            via_drill_nm: extracted.problem.rules.via_drill.to_nm(),
            via_size_nm: extracted.problem.rules.via_diameter.to_nm(),
        })
    }
}
