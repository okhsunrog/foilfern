//! Autorouting of a board with the built-in router of
//! [`librepcb_autoroute`] (no upstream counterpart: upstream LibrePCB only
//! exports Specctra DSN for external autorouters).
//!
//! [`Autoroute`] extracts a [`RoutingProblem`] from a board, routes the
//! current air wires and applies the result as ordinary traces and vias
//! through [`AddTrace`] and the via logic of [`AddVia`](super::AddVia), so the result is
//! one undo group and indistinguishable from manually drawn copper.
//!
//! The problem is built from the design rule check input
//! ([`BoardDrcData`]), which already resolves all geometry the DRC checks
//! (pad shapes per layer, actual via sizes, footprint items):
//!
//! - outline: all polygons on board edge layers (board and footprints);
//! - terminals: all footprint pads with a net, all vias with a net and the
//!   junctions air wires end at;
//! - obstacles: pads without net (and the pad shapes on further layers),
//!   traces, vias without net, copper polygons, circles and stroke texts,
//!   and "no copper" keepout zones; holes and via/pad drills as holes;
//! - planes are no obstacles: they are filled around the new copper when
//!   they are rebuilt, and they connect their own net through the air wires;
//! - rules: the board's DRC settings (clearances, minimum widths, drills),
//!   the design rules (default trace width, via drill and annular ring) and
//!   the net classes, plus a small safety margin on all clearances
//!   ([`CLEARANCE_MARGIN`]) which absorbs polygon approximations of arcs.

use std::collections::{BTreeMap, BTreeSet};

pub use librepcb_autoroute::{
    AccessBlocker, Connection as RoutingConnection, Shape as RoutingShape, TerminalAccess,
    TerminalId as RoutingTerminalId, diagnose_access,
};
use librepcb_autoroute::{
    FrontierBlocker, FrontierSample, Hole, LayerScope, NetId, Obstacle, RoutingProblem,
    RoutingRegion, RoutingResult, Shape, Terminal, TerminalId, UnroutedReason,
};
use librepcb_core::geometry::{
    NonEmptyPath, PadGeometry, Path, TraceAnchor, Via, ZoneLayers, ZoneRules,
};
use librepcb_core::project::board::drc::{BoardDrcData, DrcHole, DrcPad, DrcStrokeText};
use librepcb_core::project::board::{AirWire, Board, BoardAirWiresBuilder};
use librepcb_core::project::{BoardId, ComponentInstanceId, NetSignalId, Project};
use librepcb_core::types::{Layer, Length, MaskConfig, Point, PositiveLength, Uuid};
use librepcb_core::utils::transform::Transform;
use librepcb_i18n::tr;

use super::board::{AddTrace, TraceEndpoint, add_via_connected};
use super::{NetRef, resolve};
use crate::editor::{Command, Transaction};
use crate::error::{Error, Result};

mod rules;

/// Safety margin added to all clearances of the routing problem (10 µm).
pub const CLEARANCE_MARGIN: Length = Length::new(10_000);

/// Tolerance for flattening arcs to polygons (5 µm).
const ARC_TOLERANCE: Length = Length::new(5_000);

fn positive(length: Length) -> Option<PositiveLength> {
    PositiveLength::new(length).ok()
}

/// Rectangular area of a regional routing request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RoutingBounds {
    pub min: Point,
    pub max: Point,
}

impl RoutingBounds {
    pub(crate) fn region(self) -> Result<RoutingRegion> {
        RoutingRegion::new(self.min, self.max).ok_or_else(|| {
            Error::InvalidArgument("The routing region must have positive width and height.".into())
        })
    }
}

/// Autoroutes the air wires of a board (see the [module docs](self)).
///
/// Existing copper is kept; only unrouted connections (air wires) are
/// routed. Connections which cannot be routed are reported in the result
/// and stay air wires.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Autoroute {
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// Only route the air wires of these nets (default: all nets).
    #[serde(default)]
    pub nets: Option<Vec<NetRef>>,
    /// Copper layers traces may use (default: all copper layers of the
    /// board). Vias always span all layers.
    #[serde(default)]
    pub layers: Option<Vec<Layer>>,
    /// Trace width of all nets (default: per net class default trace width,
    /// else the design rules' default trace width, at least the DRC minimum
    /// copper width).
    #[serde(default)]
    pub trace_width: Option<PositiveLength>,
    /// Drill diameter of new vias (default: the design rules' default via
    /// drill, at least the DRC minimum PTH drill diameter).
    #[serde(default)]
    pub via_drill: Option<PositiveLength>,
    /// Outer diameter of new vias (default: from the drill and the design
    /// rules' via annular ring, at least the DRC minimum annular ring).
    #[serde(default)]
    pub via_size: Option<PositiveLength>,
    /// Pitch of the routing grid (default: automatic, 0.1 mm for small
    /// boards).
    #[serde(default)]
    pub grid_pitch: Option<PositiveLength>,
    /// Cost of a via as trace length (default: 2 mm).
    #[serde(default)]
    pub via_cost: Option<Length>,
    /// Collect rejected grid steps for failed strict searches.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub search_evidence: bool,
    /// Check additional ways to reach the grid from pads.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pad_escape: bool,
    /// Learn contested locations after successful rip-ups.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub congestion_history: bool,
    /// Initial order within each priority tier; omitted keeps builtin defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<RoutingOrder>,
    /// Nets routed first and protected from rip-up by other nets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub critical_nets: Vec<NetRef>,
    /// Limits new copper to a rectangular region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<RoutingBounds>,
}

/// Explicit builtin trial order, independent of the pad escape setting.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RoutingOrder {
    ShortestFirst,
    LongestFirst,
    ConstrainedFirst,
}

impl From<RoutingOrder> for librepcb_autoroute::RoutingOrder {
    fn from(order: RoutingOrder) -> Self {
        match order {
            RoutingOrder::ShortestFirst => Self::ShortestFirst,
            RoutingOrder::LongestFirst => Self::LongestFirst,
            RoutingOrder::ConstrainedFirst => Self::ConstrainedFirst,
        }
    }
}

impl Autoroute {
    pub(crate) fn critical_net_ids(&self, project: &Project) -> Result<BTreeSet<NetSignalId>> {
        self.critical_nets
            .iter()
            .map(|net| resolve::required_net(project, net))
            .collect()
    }
}

/// An air wire which was not routed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UnroutedAirWire {
    /// The net.
    pub net: NetSignalId,
    /// Name of the net.
    pub net_name: String,
    /// First anchor.
    pub p1: TraceAnchor,
    /// Position of the first anchor.
    pub p1_position: Point,
    /// Second anchor.
    pub p2: TraceAnchor,
    /// Position of the second anchor.
    pub p2_position: Point,
    /// Last strict failure, final verification outcome, or `unsupported_anchor`.
    pub reason: String,
    /// Last failure from an attempt allowing rip-up, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_reason: Option<String>,
    /// Bounded samples from the failed search on the original request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_evidence: Vec<RoutingSearchSample>,
}

impl UnroutedAirWire {
    fn new(p: &Project, wire: &AirWire, reason: &str) -> Self {
        Self {
            net: wire.net(),
            net_name: resolve::net_name(p, Some(wire.net())),
            p1: wire.p1(),
            p1_position: wire.p1_position(),
            p2: wire.p2(),
            p2_position: wire.p2_position(),
            reason: reason.to_owned(),
            soft_reason: None,
            search_evidence: Vec::new(),
        }
    }
}

/// Result of [`Autoroute`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AutorouteResult {
    /// The board.
    pub board: BoardId,
    /// Number of air wires given to the router.
    pub air_wires: usize,
    /// Number of routed connections.
    pub routed: usize,
    /// Number of connections the router could not route.
    pub unrouted: usize,
    /// Number of new traces.
    pub traces: usize,
    /// Number of new vias.
    pub vias: usize,
    /// Grid pitch used by the router (nanometers).
    pub grid_pitch: i64,
    /// Work performed by the builtin search.
    #[serde(default)]
    pub effort: RoutingEffort,
    /// Original air wires left unresolved, with the router's attempt evidence.
    #[serde(default)]
    pub failed_requests: Vec<UnroutedAirWire>,
    /// The air wires of the routed nets which remain after routing
    /// (rebuilt from the new copper).
    pub remaining_air_wires: Vec<UnroutedAirWire>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RoutingEffort {
    pub searches: usize,
    pub rip_ups: usize,
    pub congestion_tiles: usize,
}

impl Command for Autoroute {
    type Output = AutorouteResult;

    fn text(&self) -> String {
        tr!("Autorouter", "Autoroute Board")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<AutorouteResult> {
        let board = resolve::board(tx.project(), self.board)?.id();
        let nets = match &self.nets {
            Some(refs) => Some(
                refs.iter()
                    .map(|r| resolve::required_net(tx.project(), r))
                    .collect::<Result<BTreeSet<_>>>()?,
            ),
            None => None,
        };
        let extracted = extract_problem(tx.project(), board, &self, nets.as_ref())?;
        let result = librepcb_autoroute::route(&extracted.problem)?;
        let (traces, vias) = apply_result(tx, board, &extracted, &result)?;

        let p = tx.project();
        let failed_requests: Vec<_> = result
            .unrouted
            .iter()
            .map(|u| {
                // The extractor adds one original air wire per request.
                let wire = &extracted.request_wires[u.request];
                let mut failure = UnroutedAirWire::new(p, wire, reason_name(u.reason));
                failure.soft_reason = u.soft_reason.map(|reason| reason_name(reason).to_owned());
                failure.search_evidence = u
                    .frontier
                    .iter()
                    .map(|sample| extracted.search_sample(sample))
                    .collect();
                failure
            })
            .chain(
                extracted
                    .unsupported
                    .iter()
                    .map(|wire| UnroutedAirWire::new(p, wire, "unsupported_anchor")),
            )
            .collect();

        // Only copy failure evidence when the rebuilt air wire has the same endpoints.
        let b = resolve::board(p, Some(board))?;
        let builder = BoardAirWiresBuilder::new(b, p.library(), p.circuit());
        let remaining_air_wires = extracted
            .nets
            .iter()
            .flat_map(|net| builder.build_air_wires(*net))
            .map(|wire| {
                let ends = (wire.p1(), wire.p2());
                let failure = failed_requests.iter().find(|f| {
                    f.net == wire.net() && (ends == (f.p1, f.p2) || ends == (f.p2, f.p1))
                });
                let mut remaining = UnroutedAirWire::new(
                    p,
                    &wire,
                    failure.map_or("not_routed", |f| f.reason.as_str()),
                );
                remaining.soft_reason = failure.and_then(|f| f.soft_reason.clone());
                remaining
            })
            .collect();
        Ok(AutorouteResult {
            board,
            air_wires: result.stats.air_wires,
            routed: result.routes.len(),
            unrouted: failed_requests.len(),
            traces,
            vias,
            grid_pitch: result.stats.grid_pitch,
            effort: RoutingEffort {
                searches: result.stats.searches,
                rip_ups: result.stats.rip_ups,
                congestion_tiles: result.stats.congestion_tiles,
            },
            failed_requests,
            remaining_air_wires,
        })
    }
}

fn reason_name(reason: UnroutedReason) -> &'static str {
    match reason {
        UnroutedReason::NoAccess => "no_access",
        UnroutedReason::NoPath => "no_path",
        UnroutedReason::SearchBudget => "search_budget",
        UnroutedReason::GeometryRejected => "geometry_rejected",
        UnroutedReason::RipUpLimit => "rip_up_limit",
        UnroutedReason::ProtectedRoute => "protected_route",
        UnroutedReason::NoRipUpConflict => "no_rip_up_conflict",
        UnroutedReason::AttachmentRemoved => "attachment_removed",
        UnroutedReason::NotRouted => "not_routed",
        UnroutedReason::Cancelled => "cancelled",
        _ => "unknown",
    }
}

/// Type of the board object that contributed routing geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingObjectKind {
    Trace,
    Via,
    Pad,
    Junction,
    Hole,
    Polygon,
    Circle,
    StrokeText,
    Zone,
}

/// Identity of routing geometry in the board or its placed footprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RoutingObject {
    pub kind: RoutingObjectKind,
    pub uuid: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_uuid: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pad_uuid: Option<Uuid>,
}

/// Geometry confirmed by exact checking of a rejected grid step.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RoutingSearchBlocker {
    BoardEdge,
    RegionBoundary,
    Existing { object: RoutingObject },
    Generated { request: usize, net: NetSignalId },
}

/// Search evidence on the same nanometer coordinates as the failed request.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RoutingSearchSample {
    pub layer: Layer,
    pub from: Point,
    pub to: Point,
    /// True for a via transition or its landing clearance.
    pub via: bool,
    /// True when the occupancy was from a route generated in this run.
    pub generated: bool,
    pub blocker: Option<RoutingSearchBlocker>,
}

impl RoutingObject {
    fn new(kind: RoutingObjectKind, uuid: Uuid, device_uuid: Option<Uuid>) -> Self {
        Self {
            kind,
            uuid,
            device_uuid,
            pad_uuid: None,
        }
    }

    /// Identifies the board object addressed by a trace anchor.
    pub fn from_anchor(anchor: TraceAnchor) -> Self {
        use RoutingObjectKind as Kind;
        match anchor {
            TraceAnchor::FootprintPad { device, pad } => Self::new(Kind::Pad, pad, Some(device)),
            TraceAnchor::Pad(uuid) => Self::new(Kind::Pad, uuid, None),
            TraceAnchor::Via(uuid) => Self::new(Kind::Via, uuid, None),
            TraceAnchor::Junction(uuid) => Self::new(Kind::Junction, uuid, None),
        }
    }
}

/// A routing problem with the mapping back to the board.
#[derive(Debug, Clone)]
pub struct ExtractedProblem {
    /// The problem.
    pub problem: RoutingProblem,
    /// The anchor of each terminal (index = [`TerminalId`]).
    pub terminal_anchors: Vec<TraceAnchor>,
    /// Source objects indexed by routing obstacle.
    pub obstacle_objects: Vec<RoutingObject>,
    /// Source objects indexed by routing hole.
    pub hole_objects: Vec<RoutingObject>,
    /// The nets whose air wires are routed.
    pub nets: BTreeSet<NetSignalId>,
    /// Original board air wires, indexed by routing connection request.
    request_wires: Vec<AirWire>,
    /// Air wires which could not be given to the router (anchors which are
    /// not supported, e.g. standalone board pads).
    pub unsupported: Vec<AirWire>,
    /// Drill and size of new vias per net if they differ from the
    /// automatic values of the via (`None`: automatic).
    pub via_sizes: BTreeMap<NetSignalId, (Option<PositiveLength>, Option<PositiveLength>)>,
}

impl ExtractedProblem {
    fn search_sample(&self, sample: &FrontierSample) -> RoutingSearchSample {
        let blocker = sample.blocker.map(|blocker| match blocker {
            FrontierBlocker::RegionBoundary => RoutingSearchBlocker::RegionBoundary,
            FrontierBlocker::Existing(AccessBlocker::BoardEdge) => RoutingSearchBlocker::BoardEdge,
            FrontierBlocker::Existing(blocker) => {
                let object = match blocker {
                    AccessBlocker::Obstacle(i) => self.obstacle_objects[i],
                    AccessBlocker::Hole(i) => self.hole_objects[i],
                    AccessBlocker::Terminal(id) => {
                        RoutingObject::from_anchor(self.terminal_anchors[id.0])
                    }
                    AccessBlocker::BoardEdge => return RoutingSearchBlocker::BoardEdge,
                };
                RoutingSearchBlocker::Existing { object }
            }
            FrontierBlocker::Generated { request, net } => RoutingSearchBlocker::Generated {
                request,
                net: NetSignalId(net),
            },
        });
        RoutingSearchSample {
            layer: sample.layer,
            from: sample.from,
            to: sample.to,
            via: sample.via,
            generated: sample.generated,
            blocker,
        }
    }

    /// Explains the effective builtin rules for one net using the same board
    /// snapshot and options used to extract this problem.
    pub fn rule_report(
        &self,
        p: &Project,
        board: &Board,
        options: &Autoroute,
        net: NetSignalId,
        grid_pitch: Option<Length>,
    ) -> impl serde::Serialize {
        rules::Inputs::new(p, board, options).report(self, net, grid_pitch)
    }
}

/// Converts a path to polygons (arcs flattened, closing vertex removed).
fn polygon_points(path: &Path) -> Vec<Point> {
    let tolerance = positive(ARC_TOLERANCE).expect("constant is positive");
    let mut points: Vec<Point> = path
        .flattened_arcs(tolerance)
        .vertices()
        .iter()
        .map(|v| v.pos)
        .collect();
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    points
}

/// Converts a path to line segments (arcs flattened).
fn path_segments(path: &Path) -> Vec<(Point, Point)> {
    let tolerance = positive(ARC_TOLERANCE).expect("constant is positive");
    let points: Vec<Point> = path
        .flattened_arcs(tolerance)
        .vertices()
        .iter()
        .map(|v| v.pos)
        .collect();
    points.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Returns the shapes of a stroke along `path` (a circle for single points).
fn stroke_shapes(path: &Path, width: PositiveLength) -> Vec<Shape> {
    let segments = path_segments(path);
    if segments.is_empty() {
        return path
            .vertices()
            .first()
            .map(|v| Shape::Circle {
                center: v.pos,
                diameter: width,
            })
            .into_iter()
            .collect();
    }
    segments
        .into_iter()
        .map(|(start, end)| Shape::Segment { start, end, width })
        .collect()
}

/// Returns the shapes of a drill along `path` (placed with `transform`).
fn hole_shapes(path: &NonEmptyPath, diameter: PositiveLength, transform: &Transform) -> Vec<Shape> {
    stroke_shapes(&transform.map(path.get()), diameter)
}

/// Returns the copper shapes of a pad geometry placed with `transform`.
fn pad_geometry_shapes(geometry: &PadGeometry, transform: &Transform) -> Vec<Shape> {
    let mut shapes: Vec<Shape> = geometry
        .to_outlines()
        .unwrap_or_default()
        .iter()
        .map(|outline| Shape::Polygon(polygon_points(&transform.map(outline))))
        .filter(|s| matches!(s, Shape::Polygon(p) if p.len() >= 3))
        .collect();
    // The copper around the holes (like the DRC's copper area).
    for hole in geometry.holes().iter() {
        shapes.extend(stroke_shapes(
            &transform.map(hole.path().get()),
            hole.diameter(),
        ));
    }
    shapes
}

/// Layers of a footprint zone on the board.
fn zone_layers(layers: ZoneLayers, mirrored: bool, copper_layers: &[Layer]) -> BTreeSet<Layer> {
    let (top, bottom) = if mirrored {
        (Layer::BOT_COPPER, Layer::TOP_COPPER)
    } else {
        (Layer::TOP_COPPER, Layer::BOT_COPPER)
    };
    let mut result = BTreeSet::new();
    if layers.contains(ZoneLayers::TOP) {
        result.insert(top);
    }
    if layers.contains(ZoneLayers::BOTTOM) {
        result.insert(bottom);
    }
    if layers.contains(ZoneLayers::INNER) {
        result.extend(copper_layers.iter().filter(|l| l.is_inner()));
    }
    result
}

/// Helper building the problem.
struct Builder<'a> {
    problem: RoutingProblem,
    terminal_anchors: Vec<TraceAnchor>,
    terminals_by_anchor: BTreeMap<TraceAnchor, TerminalId>,
    obstacle_objects: Vec<RoutingObject>,
    hole_objects: Vec<RoutingObject>,
    copper_layers: &'a [Layer],
}

impl Builder<'_> {
    fn layer_scope(&self, layers: impl IntoIterator<Item = Layer>) -> LayerScope {
        let layers: BTreeSet<Layer> = layers
            .into_iter()
            .filter(|l| self.copper_layers.contains(l))
            .collect();
        if layers.len() == self.copper_layers.len() {
            LayerScope::All
        } else {
            LayerScope::Only(layers.into_iter().collect())
        }
    }

    fn clearance(&self, clearance: Length) -> Option<Length> {
        let clearance = clearance + CLEARANCE_MARGIN;
        (clearance > self.problem.rules.clearance).then_some(clearance)
    }

    fn add_obstacle(&mut self, obstacle: Obstacle, object: RoutingObject) {
        self.problem.add_obstacle(obstacle);
        self.obstacle_objects.push(object);
    }

    fn add_hole(&mut self, shapes: Vec<Shape>, plated: bool, object: RoutingObject) {
        for shape in shapes {
            self.problem.add_hole(Hole { shape, plated });
            self.hole_objects.push(object);
        }
    }

    /// Adds a pad: a terminal (first shape) plus same-net obstacles for the
    /// other shapes, or obstacles only if it has no net.
    fn add_pad(&mut self, pad: &DrcPad, anchor: TraceAnchor) {
        let object = RoutingObject::from_anchor(anchor);
        let transform = Transform::new(pad.position, pad.rotation, pad.mirror);
        let clearance = self.clearance(*pad.copper_clearance);
        let net = pad.net;
        let mut shapes_per_layer: Vec<(Layer, Vec<Shape>)> = Vec::new();
        for (layer, geometries) in &pad.geometries {
            if !self.copper_layers.contains(layer) {
                continue;
            }
            let shapes: Vec<Shape> = geometries
                .iter()
                .flat_map(|g| pad_geometry_shapes(g, &transform))
                .collect();
            if !shapes.is_empty() {
                shapes_per_layer.push((*layer, shapes));
            }
        }
        // Plated drills.
        for hole in &pad.holes {
            let shapes = hole_shapes(&hole.path, hole.diameter, &transform);
            self.add_hole(
                shapes,
                true,
                RoutingObject {
                    kind: RoutingObjectKind::Hole,
                    uuid: hole.uuid,
                    device_uuid: object.device_uuid,
                    pad_uuid: Some(object.uuid),
                },
            );
        }
        let layers: Vec<Layer> = shapes_per_layer.iter().map(|(l, _)| *l).collect();
        if let (Some(net), Some((_, first))) = (net, shapes_per_layer.first()) {
            // The terminal: the first shape of the first layer, on all
            // layers of the pad.
            let shape = first[0].clone();
            let id = self.problem.add_terminal(Terminal {
                net,
                position: pad.position,
                shape: shape.clone(),
                layers: self.layer_scope(layers.iter().copied()),
                clearance,
            });
            self.problem.pad_terminals.insert(id);
            self.terminal_anchors.push(anchor);
            self.terminals_by_anchor.insert(anchor, id);
            for (layer, shapes) in &shapes_per_layer {
                for s in shapes {
                    if *s != shape {
                        self.add_obstacle(
                            Obstacle {
                                shape: s.clone(),
                                layers: LayerScope::single(*layer),
                                net: Some(net),
                                clearance,
                            },
                            object,
                        );
                    }
                }
            }
        } else {
            for (layer, shapes) in shapes_per_layer {
                for shape in shapes {
                    self.add_obstacle(
                        Obstacle {
                            shape,
                            layers: LayerScope::single(layer),
                            net,
                            clearance,
                        },
                        object,
                    );
                }
            }
        }
    }

    fn add_board_holes(&mut self, holes: &[DrcHole], transform: &Transform, device: Option<Uuid>) {
        for hole in holes {
            let shapes = hole_shapes(&hole.path, hole.diameter, transform);
            self.add_hole(
                shapes,
                false,
                RoutingObject::new(RoutingObjectKind::Hole, hole.uuid, device),
            );
        }
    }

    fn add_keepout(
        &mut self,
        rules: ZoneRules,
        outline: &Path,
        layers: BTreeSet<Layer>,
        object: RoutingObject,
    ) {
        if !rules.contains(ZoneRules::NO_COPPER) || layers.is_empty() {
            return;
        }
        let points = polygon_points(outline);
        if points.len() >= 3 {
            let layers = self.layer_scope(layers);
            self.add_obstacle(Obstacle::keepout(Shape::Polygon(points), layers), object);
        }
    }

    fn add_copper_polygon(
        &mut self,
        path: &Path,
        line_width: Length,
        filled: bool,
        layer: Layer,
        object: RoutingObject,
    ) {
        if !self.copper_layers.contains(&layer) {
            return;
        }
        if let Some(width) = positive(line_width) {
            for shape in stroke_shapes(path, width) {
                self.add_obstacle(
                    Obstacle::copper(shape, LayerScope::single(layer), None),
                    object,
                );
            }
        }
        if filled && path.is_closed() {
            let points = polygon_points(path);
            if points.len() >= 3 {
                self.add_obstacle(
                    Obstacle::copper(Shape::Polygon(points), LayerScope::single(layer), None),
                    object,
                );
            }
        }
    }

    fn add_copper_text(&mut self, text: &DrcStrokeText, device: Option<Uuid>) {
        if self.copper_layers.contains(&text.layer)
            && let Some(width) = positive(*text.stroke_width)
        {
            let transform = Transform::new(text.position, text.rotation, text.mirror);
            let object = RoutingObject::new(RoutingObjectKind::StrokeText, text.uuid, device);
            for path in &text.paths {
                for shape in stroke_shapes(&transform.map(path), width) {
                    self.add_obstacle(
                        Obstacle::copper(shape, LayerScope::single(text.layer), None),
                        object,
                    );
                }
            }
        }
    }
}

/// Builds the routing problem of a board (see the [module docs](self)).
///
/// `nets` limits the routed air wires to these nets.
pub fn extract_problem(
    p: &Project,
    board: BoardId,
    options: &Autoroute,
    nets: Option<&BTreeSet<NetSignalId>>,
) -> Result<ExtractedProblem> {
    use RoutingObjectKind as Kind;

    let b = resolve::board(p, Some(board))?;
    let settings = b.settings().drc_settings.clone();
    let data = BoardDrcData::new(p, board, &settings, true)
        .map_err(|e| Error::InvalidArgument(e.to_string()))?;

    // Copper layers, top to bottom.
    let mut copper_layers: Vec<Layer> = data.copper_layers.iter().copied().collect();
    copper_layers.sort_by_key(|l| l.copper_number());

    let inputs = rules::Inputs::new(p, b, options);
    let mut routing_rules = inputs.routing_rules();
    routing_rules.critical_nets = options
        .critical_net_ids(p)?
        .into_iter()
        .map(|id| id.0)
        .collect();
    let trace_width = routing_rules.trace_width;
    let region = options.region.map(RoutingBounds::region).transpose()?;

    // Vias: keep both dimensions automatic only when they reproduce the
    // routing geometry. An explicit size requires an explicit drill, even
    // when that drill equals the inherited value (Via's file-format invariant).
    let mut via_sizes = BTreeMap::new();
    for net in p.circuit().net_signals().values() {
        let (auto_drill, auto_size) = inputs.inherited_via(net.net_class());
        let explicit =
            auto_drill != routing_rules.via_drill || auto_size != routing_rules.via_diameter;
        let drill = explicit.then_some(routing_rules.via_drill);
        let size = explicit.then_some(routing_rules.via_diameter);
        via_sizes.insert(NetSignalId(net.uuid()), (drill, size));
    }

    // Outline.
    let mut outline = Vec::new();
    for polygon in &data.polygons {
        if polygon.layer.is_board_edge() {
            outline.push(polygon_points(&polygon.path));
        }
    }
    for dev in data.devices.values() {
        let transform = dev.transform();
        for polygon in &dev.polygons {
            if transform.map(&polygon.layer).is_board_edge() {
                outline.push(polygon_points(&transform.map(&polygon.path)));
            }
        }
    }
    outline.retain(|p| p.len() >= 3);

    let mut builder = Builder {
        problem: RoutingProblem::new(outline, copper_layers.clone(), routing_rules),
        terminal_anchors: Vec::new(),
        terminals_by_anchor: BTreeMap::new(),
        obstacle_objects: Vec::new(),
        hole_objects: Vec::new(),
        copper_layers: &copper_layers,
    };
    builder.problem.region = region;

    // Board items.
    for polygon in &data.polygons {
        builder.add_copper_polygon(
            &polygon.path,
            *polygon.line_width,
            polygon.filled,
            polygon.layer,
            RoutingObject::new(Kind::Polygon, polygon.uuid, None),
        );
    }
    for text in &data.stroke_texts {
        builder.add_copper_text(text, None);
    }
    builder.add_board_holes(&data.holes, &Transform::default(), None);
    for zone in &data.zones {
        builder.add_keepout(
            zone.rules,
            &zone.outline,
            zone.board_layers.clone(),
            RoutingObject::new(Kind::Zone, zone.uuid, None),
        );
    }

    // Devices.
    for (device_uuid, dev) in &data.devices {
        let device = Some(*device_uuid);
        let transform = dev.transform();
        for (pad_uuid, pad) in &dev.pads {
            builder.add_pad(
                pad,
                TraceAnchor::FootprintPad {
                    device: *device_uuid,
                    pad: *pad_uuid,
                },
            );
        }
        for polygon in &dev.polygons {
            builder.add_copper_polygon(
                &transform.map(&polygon.path),
                *polygon.line_width,
                polygon.filled,
                transform.map(&polygon.layer),
                RoutingObject::new(Kind::Polygon, polygon.uuid, device),
            );
        }
        for circle in &dev.circles {
            let layer = transform.map(&circle.layer);
            if !copper_layers.contains(&layer) {
                continue;
            }
            let center = transform.map(&circle.center);
            let outer = circle.diameter + *circle.line_width;
            if let Some(diameter) = positive(outer)
                && (circle.filled || *circle.line_width > Length::ZERO)
            {
                builder.add_obstacle(
                    Obstacle::copper(
                        Shape::Circle { center, diameter },
                        LayerScope::single(layer),
                        None,
                    ),
                    RoutingObject::new(Kind::Circle, circle.uuid, device),
                );
            }
        }
        for text in &dev.stroke_texts {
            builder.add_copper_text(text, device);
        }
        builder.add_board_holes(&dev.holes, &transform, device);
        for zone in &dev.zones {
            let layers = zone_layers(zone.footprint_layers, dev.mirror, &copper_layers);
            builder.add_keepout(
                zone.rules,
                &transform.map(&zone.outline),
                layers,
                RoutingObject::new(Kind::Zone, zone.uuid, device),
            );
        }
    }

    // Net segments.
    let mut junction_terminals: BTreeSet<Uuid> = BTreeSet::new();
    let air_wire_builder = BoardAirWiresBuilder::new(b, p.library(), p.circuit());
    let routed_nets: BTreeSet<NetSignalId> = p
        .circuit()
        .net_signals()
        .keys()
        .copied()
        .filter(|n| nets.is_none_or(|f| f.contains(n)))
        .collect();
    let mut air_wires = Vec::new();
    for net in &routed_nets {
        for wire in air_wire_builder.build_air_wires(*net) {
            if region.is_some_and(|area| {
                !area.contains(wire.p1_position(), Length::ZERO)
                    || !area.contains(wire.p2_position(), Length::ZERO)
            }) {
                continue;
            }
            for anchor in [wire.p1(), wire.p2()] {
                if let TraceAnchor::Junction(j) = anchor {
                    junction_terminals.insert(j);
                }
            }
            air_wires.push(wire);
        }
    }
    for seg in data.segments.values() {
        let net = seg.net;
        for (uuid, pad) in &seg.pads {
            builder.add_pad(pad, TraceAnchor::Pad(*uuid));
        }
        for via in seg.vias.values() {
            let object = RoutingObject::new(Kind::Via, via.uuid, None);
            let layers = builder.layer_scope(
                copper_layers
                    .iter()
                    .copied()
                    .filter(|l| Via::is_on_layer_between(*l, via.start_layer, via.end_layer)),
            );
            let shape = Shape::Circle {
                center: via.position,
                diameter: via.size,
            };
            builder.add_hole(
                vec![Shape::Circle {
                    center: via.position,
                    diameter: via.drill_diameter,
                }],
                true,
                object,
            );
            match net {
                Some(net) => {
                    let anchor = TraceAnchor::Via(via.uuid);
                    let id = builder.problem.add_terminal(Terminal {
                        net,
                        position: via.position,
                        shape,
                        layers,
                        clearance: None,
                    });
                    builder.terminal_anchors.push(anchor);
                    builder.terminals_by_anchor.insert(anchor, id);
                }
                None => builder.add_obstacle(Obstacle::copper(shape, layers, None), object),
            }
        }
        for trace in &seg.traces {
            builder.add_obstacle(
                Obstacle::copper(
                    Shape::Segment {
                        start: trace.p1,
                        end: trace.p2,
                        width: trace.width,
                    },
                    LayerScope::single(trace.layer),
                    net,
                ),
                RoutingObject::new(Kind::Trace, trace.uuid, None),
            );
        }
        if let Some(net) = net {
            for junction in seg.junctions.values() {
                if !junction_terminals.contains(&junction.uuid) {
                    continue;
                }
                let widths_layers: Vec<(PositiveLength, Layer)> = seg
                    .traces
                    .iter()
                    .filter(|t| t.p1 == junction.position || t.p2 == junction.position)
                    .map(|t| (t.width, t.layer))
                    .collect();
                let Some(&(_, layer)) = widths_layers.first() else {
                    continue;
                };
                let diameter = widths_layers
                    .iter()
                    .map(|(w, _)| *w)
                    .max()
                    .unwrap_or(trace_width);
                let anchor = TraceAnchor::Junction(junction.uuid);
                let id = builder.problem.add_terminal(Terminal {
                    net,
                    position: junction.position,
                    shape: Shape::Circle {
                        center: junction.position,
                        diameter,
                    },
                    layers: LayerScope::single(layer),
                    clearance: None,
                });
                builder.terminal_anchors.push(anchor);
                builder.terminals_by_anchor.insert(anchor, id);
            }
        }
    }

    // Connections.
    let mut request_wires = Vec::new();
    let mut unsupported = Vec::new();
    for wire in air_wires {
        match (
            builder.terminals_by_anchor.get(&wire.p1()),
            builder.terminals_by_anchor.get(&wire.p2()),
        ) {
            (Some(a), Some(b)) if a != b => {
                builder.problem.connect(*a, *b);
                request_wires.push(wire);
            }
            (Some(_), Some(_)) => {}
            _ => unsupported.push(wire),
        }
    }
    Ok(ExtractedProblem {
        problem: builder.problem,
        terminal_anchors: builder.terminal_anchors,
        obstacle_objects: builder.obstacle_objects,
        hole_objects: builder.hole_objects,
        nets: routed_nets,
        request_wires,
        unsupported,
        via_sizes,
    })
}

/// Returns the trace endpoint for a route node at `position` on `layer`
/// of `net`, looked up in the current board: a via, a terminal pad or an
/// existing junction there, else a new junction.
fn endpoint_at(
    p: &Project,
    board: BoardId,
    extracted: &ExtractedProblem,
    net: NetId,
    position: Point,
    layer: Layer,
) -> Result<TraceEndpoint> {
    let b = resolve::board(p, Some(board))?;
    let net_id = NetSignalId(net);
    for seg in b.net_segments().values() {
        if seg.net() != Some(net_id) {
            continue;
        }
        if let Some(via) = seg
            .vias()
            .values()
            .find(|v| v.position() == position && v.is_on_layer(layer))
        {
            return Ok(TraceEndpoint::Via(via.uuid()));
        }
    }
    for (terminal, anchor) in extracted
        .problem
        .terminals
        .iter()
        .zip(&extracted.terminal_anchors)
    {
        let on_layer = match &terminal.layers {
            LayerScope::All => true,
            LayerScope::Only(layers) => layers.contains(&layer),
        };
        if terminal.net != net || terminal.position != position || !on_layer {
            continue;
        }
        if let TraceAnchor::FootprintPad { device, pad } = anchor {
            return Ok(TraceEndpoint::FootprintPad {
                component: ComponentInstanceId(*device),
                pad: *pad,
            });
        }
    }
    for seg in b.net_segments().values() {
        if seg.net() != Some(net_id) {
            continue;
        }
        for junction in seg.junctions().values() {
            if junction.position() == position
                && seg
                    .junction_layer(&junction.uuid())
                    .is_none_or(|l| l == layer)
            {
                return Ok(TraceEndpoint::Junction(junction.uuid()));
            }
        }
    }
    Ok(TraceEndpoint::Point(position))
}

/// Applies a routing result as traces and vias. Returns the number of new
/// traces and vias.
pub fn apply_result(
    tx: &mut Transaction<'_>,
    board: BoardId,
    extracted: &ExtractedProblem,
    result: &RoutingResult,
) -> Result<(usize, usize)> {
    let mut traces = 0;
    let mut vias = 0;
    for route in &result.routes {
        let net = NetSignalId(route.net);
        let (drill, size) = extracted.via_sizes.get(&net).copied().unwrap_or_default();
        for via in &route.vias {
            let via = Via::new(
                Uuid::new_random(),
                Layer::TOP_COPPER,
                Layer::BOT_COPPER,
                via.position,
                drill,
                size,
                MaskConfig::Off,
            )
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;
            add_via_connected(tx, board, via, Some(net))?;
            vias += 1;
        }
        for segment in &route.segments {
            if segment.start == segment.end {
                continue;
            }
            let start = endpoint_at(
                tx.project(),
                board,
                extracted,
                route.net,
                segment.start,
                segment.layer,
            )?;
            let end = endpoint_at(
                tx.project(),
                board,
                extracted,
                route.net,
                segment.end,
                segment.layer,
            )?;
            if start == end {
                continue;
            }
            tx.run(AddTrace {
                board: Some(board),
                start,
                end,
                points: Vec::new(),
                layer: Some(segment.layer),
                width: Some(segment.width),
                net: Some(NetRef::Id(net)),
            })?;
            traces += 1;
        }
    }
    Ok((traces, vias))
}
