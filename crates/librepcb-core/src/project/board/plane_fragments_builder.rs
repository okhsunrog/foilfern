//! Port of libs/librepcb/core/project/board/boardplanefragmentsbuilder.{h,cpp}.
//!
//! Differences to upstream:
//! - The job data (upstream `JobData`) is extracted from the project with
//!   [`Project::plane_job()`] into an owned [`PlaneJob`] snapshot, which
//!   can be computed on any thread ([`PlaneJob::run()`], one rayon task per
//!   layer like upstream's `QtConcurrent` threads). The result
//!   ([`PlaneFragments`]) is applied with
//!   [`Project::apply_plane_job_result()`] (not undoable, journaled as
//!   [`BoardChange::PlaneFragments`](super::BoardChange::PlaneFragments)). [`Project::rebuild_planes()`] does
//!   all three steps (upstream `runAndApply()`).
//! - Creating a job does not modify the board: the scheduled layers are
//!   removed from [`BoardDerived::dirty_plane_layers()`](super::BoardDerived::dirty_plane_layers) when the result is
//!   applied, and only if the project was not modified in the meantime
//!   (upstream takes them when the job is created).
//! - Cancellation is an `AtomicBool` passed to
//!   [`PlaneJob::run_with_abort()`] instead of `cancel()`.
//!
//! The geometry is calculated exactly like upstream (same Clipper port, same
//! input order), so the fragments are identical; they end up in the Gerber
//! files.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};

use clipper::{IntPoint, JoinType, PolyFillType};
use rayon::prelude::*;

use super::PlaneConnectStyle;
use super::board_context::BoardContext;
use super::export_error::{BoardExportError, BoardExportResult};
use crate::geometry::{NonEmptyPath, PadGeometry, PadGeometryShape, Path, ZoneLayers, ZoneRules};
use crate::project::Project;
use crate::project::error::{EntityKind, Error};
use crate::project::id::{BoardId, NetSignalId, PlaneId};
use crate::types::{Angle, Layer, Length, Point, PositiveLength, UnsignedLength, Uuid};
use crate::utils::clipper_helpers::{self as ch, ClipperPath, ClipperPaths};
use crate::utils::toolbox;
use crate::utils::transform::Transform;

/// Returns the maximum allowed arc tolerance when flattening arcs. Do not
/// change this if you don't know exactly what you're doing (it affects all
/// planes in all existing boards)!
fn max_arc_tolerance() -> PositiveLength {
    PositiveLength::new(Length::new(5000)).expect("constant is positive")
}

fn positive(length: Length) -> Result<PositiveLength, String> {
    PositiveLength::new(length).map_err(|e| e.to_string())
}

/// Plane data of a [`PlaneJob`] (upstream `PlaneData`).
#[derive(Debug, Clone)]
struct PlaneData {
    uuid: Uuid,
    layer: Layer,
    net: Option<NetSignalId>,
    outline: Path,
    min_width: UnsignedLength,
    min_clearance_to_copper: UnsignedLength,
    min_clearance_to_board: Option<UnsignedLength>,
    min_clearance_to_npth: Option<UnsignedLength>,
    keep_islands: bool,
    priority: i32,
    connect_style: PlaneConnectStyle,
    thermal_gap: PositiveLength,
    thermal_spoke_width: PositiveLength,
}

/// A keepout zone (upstream `KeepoutZoneData`), already preprocessed: the
/// outline is transformed and closed, the layers are board layers.
#[derive(Debug, Clone)]
struct KeepoutZoneData {
    /// Footprint zone flags, resolved in [`PlaneJob::preprocess()`].
    transform: Transform,
    layers: ZoneLayers,
    board_layers: BTreeSet<Layer>,
    outline: Path,
}

/// A polygon, circle, stroke text path or trace (upstream `PolygonData`).
#[derive(Debug, Clone)]
struct PolygonData {
    transform: Transform,
    layer: Layer,
    net: Option<NetSignalId>,
    path: Path,
    width: UnsignedLength,
    filled: bool,
}

/// Upstream `ViaData`.
#[derive(Debug, Clone)]
struct ViaData {
    net: Option<NetSignalId>,
    position: Point,
    diameter: PositiveLength,
    start_layer: Layer,
    end_layer: Layer,
}

/// Upstream `PadData`.
#[derive(Debug, Clone)]
struct PadData {
    transform: Transform,
    net: Option<NetSignalId>,
    clearance: UnsignedLength,
    geometries: BTreeMap<Layer, Vec<PadGeometry>>,
}

/// Upstream `TraceData`.
#[derive(Debug, Clone)]
struct TraceData {
    layer: Layer,
    net: Option<NetSignalId>,
    start: Point,
    end: Point,
    width: PositiveLength,
}

/// A snapshot of everything the plane fragments of a board depend on
/// (upstream `BoardPlaneFragmentsBuilder::JobData`), created by
/// [`Project::plane_job()`].
#[derive(Debug, Clone)]
pub struct PlaneJob {
    board: BoardId,
    revision: u64,
    layers: Vec<Layer>,
    planes: Vec<PlaneData>,
    keepout_zones: Vec<KeepoutZoneData>,
    polygons: Vec<PolygonData>,
    vias: Vec<ViaData>,
    pads: Vec<PadData>,
    holes: Vec<(Transform, PositiveLength, NonEmptyPath)>,
    traces: Vec<TraceData>,
}

static_assertions::assert_impl_all!(PlaneJob: Send, Sync);

/// The result of a [`PlaneJob`] (upstream
/// `BoardPlaneFragmentsBuilder::Result`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneFragments {
    /// The board of the calculated planes.
    pub board: BoardId,
    /// All processed layers.
    pub layers: BTreeSet<Layer>,
    /// The calculated plane fragments.
    pub planes: BTreeMap<PlaneId, Vec<Path>>,
    /// Any occurred errors (empty on success).
    pub errors: Vec<String>,
    /// Whether the run completed or was aborted.
    pub finished: bool,
    /// Project revision the job was created at.
    revision: u64,
}

impl PlaneFragments {
    /// Fails if any error occurred (upstream `throwOnError()`).
    pub fn check(&self) -> BoardExportResult<()> {
        match self.errors.first() {
            Some(first) => Err(BoardExportError::PlaneRebuildFailed {
                count: self.errors.len(),
                first: first.clone(),
            }),
            None => Ok(()),
        }
    }
}

/// Result of one layer (upstream `LayerJobResult`).
#[derive(Debug, Default)]
struct LayerResult {
    planes: BTreeMap<Uuid, Vec<Path>>,
    errors: Vec<String>,
}

impl Project {
    /// Creates a snapshot of the data needed to calculate the plane
    /// fragments of a board (upstream `createJob()`).
    ///
    /// If `layers` is `None`, all planes are rebuilt (more reliable, but
    /// slower). Otherwise only planes which are scheduled for rebuild (see
    /// [`BoardDerived::dirty_plane_layers()`](super::BoardDerived::dirty_plane_layers)), visible and located on the
    /// given layers are rebuilt (quick rebuild).
    ///
    /// Returns `None` if no plane needs a rebuild.
    pub fn plane_job(
        &self,
        board: BoardId,
        layers: Option<&BTreeSet<Layer>>,
    ) -> BoardExportResult<Option<PlaneJob>> {
        let b = self.board(board).ok_or(Error::NotFound {
            kind: EntityKind::Board,
            uuid: board.0,
        })?;
        let layers_with_planes: BTreeSet<Layer> = b
            .planes()
            .values()
            .filter(|p| layers.is_none_or(|f| p.visible() && f.contains(&p.layer())))
            .map(|p| p.layer())
            .collect();
        let mut job_layers: BTreeSet<Layer> = b
            .derived()
            .dirty_plane_layers()
            .intersection(&layers_with_planes)
            .copied()
            .collect();
        if layers.is_none() {
            job_layers.extend(layers_with_planes);
        }
        if job_layers.is_empty() {
            return Ok(None);
        }
        let ctx = BoardContext::new(self, b)?;
        let mut job = PlaneJob {
            board,
            revision: self.revision(),
            layers: job_layers.iter().copied().collect(),
            planes: Vec::new(),
            keepout_zones: Vec::new(),
            polygons: Vec::new(),
            vias: Vec::new(),
            pads: Vec::new(),
            holes: Vec::new(),
            traces: Vec::new(),
        };
        let mut layers = job_layers;
        layers.insert(Layer::BOARD_OUTLINES);
        layers.insert(Layer::BOARD_CUTOUTS);
        for dev in &ctx.devices {
            let transform = dev.transform();
            for pad in ctx.device_pads(dev)? {
                job.pads.push(PadData {
                    transform: pad.transform(),
                    net: pad.net,
                    clearance: pad.properties.copper_clearance(),
                    geometries: pad.geometries,
                });
            }
            for polygon in dev.footprint.polygons() {
                let layer = transform.map(&polygon.layer());
                if layers.contains(&layer) {
                    job.polygons.push(PolygonData {
                        transform,
                        layer,
                        net: None,
                        path: polygon.path().clone(),
                        width: polygon.line_width(),
                        filled: polygon.is_filled(),
                    });
                }
            }
            for circle in dev.footprint.circles() {
                let layer = transform.map(&circle.layer());
                if layers.contains(&layer) {
                    job.polygons.push(PolygonData {
                        transform,
                        layer,
                        net: None,
                        path: Path::circle(circle.diameter()).translated(circle.center()),
                        width: circle.line_width(),
                        filled: circle.is_filled(),
                    });
                }
            }
            for zone in dev.footprint.zones() {
                if zone.rules().contains(ZoneRules::NO_PLANES) {
                    job.keepout_zones.push(KeepoutZoneData {
                        transform,
                        layers: zone.layers(),
                        board_layers: BTreeSet::new(),
                        outline: zone.outline().clone(),
                    });
                }
            }
            for hole in dev.footprint.holes() {
                job.holes
                    .push((transform, hole.diameter(), hole.path().clone()));
            }
            for text in dev.device.stroke_texts().values() {
                if layers.contains(&text.layer()) {
                    for path in ctx.stroke_text_paths(text, Some(dev))? {
                        job.polygons.push(PolygonData {
                            transform: BoardContext::stroke_text_transform(text),
                            layer: text.layer(),
                            net: None,
                            path,
                            width: text.stroke_width(),
                            filled: false,
                        });
                    }
                }
            }
        }
        // Upstream `toOptionalClearance()`: 0 = no clearance (don't clip),
        // 1 = clip without clearance, else the clearance.
        let optional_clearance = |c: UnsignedLength| {
            if *c > Length::new(1) {
                Some(c)
            } else if *c == Length::new(1) {
                Some(UnsignedLength::default())
            } else {
                None
            }
        };
        for plane in b.planes().values() {
            if layers.contains(&plane.layer()) {
                job.planes.push(PlaneData {
                    uuid: plane.uuid(),
                    layer: plane.layer(),
                    net: plane.net(),
                    outline: plane.outline().clone(),
                    min_width: plane.min_width(),
                    min_clearance_to_copper: plane.min_clearance_to_copper(),
                    min_clearance_to_board: optional_clearance(plane.min_clearance_to_board()),
                    min_clearance_to_npth: optional_clearance(plane.min_clearance_to_npth()),
                    keep_islands: plane.keep_islands(),
                    priority: plane.priority(),
                    connect_style: plane.connect_style(),
                    thermal_gap: plane.thermal_gap(),
                    thermal_spoke_width: plane.thermal_spoke_width(),
                });
            }
        }
        for zone in b.zones().values() {
            if zone.rules().contains(ZoneRules::NO_PLANES) {
                job.keepout_zones.push(KeepoutZoneData {
                    transform: Transform::default(),
                    layers: ZoneLayers::empty(),
                    board_layers: zone.layers().clone(),
                    outline: zone.outline().clone(),
                });
            }
        }
        for polygon in b.polygons().values() {
            if layers.contains(&polygon.layer()) {
                job.polygons.push(PolygonData {
                    transform: Transform::default(),
                    layer: polygon.layer(),
                    net: None,
                    path: polygon.path().clone(),
                    width: polygon.line_width(),
                    filled: polygon.is_filled(),
                });
            }
        }
        for text in b.stroke_texts().values() {
            if layers.contains(&text.layer()) {
                for path in ctx.stroke_text_paths(text, None)? {
                    job.polygons.push(PolygonData {
                        transform: BoardContext::stroke_text_transform(text),
                        layer: text.layer(),
                        net: None,
                        path,
                        width: text.stroke_width(),
                        filled: false,
                    });
                }
            }
        }
        for hole in b.holes().values() {
            job.holes
                .push((Transform::default(), hole.diameter(), hole.path().clone()));
        }
        for segment in b.net_segments().values() {
            for pad in ctx.segment_pads(segment) {
                job.pads.push(PadData {
                    transform: pad.transform(),
                    net: segment.net(),
                    clearance: pad.properties.copper_clearance(),
                    geometries: pad.geometries,
                });
            }
            for via in ctx.segment_vias(segment) {
                job.vias.push(ViaData {
                    net: segment.net(),
                    position: via.via.position(),
                    diameter: via.props.size,
                    start_layer: via.via.start_layer(),
                    end_layer: via.via.end_layer(),
                });
            }
            for trace in segment.traces().values() {
                if layers.contains(&trace.layer()) {
                    let (Some(start), Some(end)) = (
                        ctx.anchor_position(segment, trace.p1()),
                        ctx.anchor_position(segment, trace.p2()),
                    ) else {
                        continue; // Invalid anchor, rejected by the mutations.
                    };
                    job.traces.push(TraceData {
                        layer: trace.layer(),
                        net: segment.net(),
                        start,
                        end,
                        width: trace.width(),
                    });
                }
            }
        }
        Ok(Some(job))
    }

    /// Applies the result of a [`PlaneJob`] to its board (upstream
    /// `Result::applyToBoard()`, see
    /// [`apply_plane_fragments()`](Self::apply_plane_fragments)). Returns
    /// whether any plane was modified.
    ///
    /// The processed layers are no longer scheduled for rebuild, unless the
    /// job was aborted or the project was modified since the job was
    /// created.
    pub fn apply_plane_job_result(&mut self, result: PlaneFragments) -> BoardExportResult<bool> {
        let unchanged = self.revision() == result.revision;
        let before = self.revision();
        self.apply_plane_fragments(result.board, result.planes)?;
        if result.finished && unchanged {
            self.take_dirty_plane_layers(result.board, &result.layers)?;
        }
        Ok(self.revision() != before)
    }

    /// Builds and applies the plane fragments of a board (upstream
    /// `BoardPlaneFragmentsBuilder::runAndApply()`, blocking; the layers
    /// are calculated in parallel). See [`plane_job()`](Self::plane_job)
    /// for `layers`. Returns all calculated plane fragments.
    ///
    /// Fails if any plane could not be calculated; nothing is applied then.
    pub fn rebuild_planes(
        &mut self,
        board: BoardId,
        layers: Option<&BTreeSet<Layer>>,
    ) -> BoardExportResult<BTreeMap<PlaneId, Vec<Path>>> {
        let Some(job) = self.plane_job(board, layers)? else {
            return Ok(BTreeMap::new());
        };
        let result = job.run();
        result.check()?;
        let planes = result.planes.clone();
        self.apply_plane_job_result(result)?;
        Ok(planes)
    }
}

impl PlaneJob {
    /// Returns the board.
    pub fn board(&self) -> BoardId {
        self.board
    }

    /// Returns the layers whose planes are calculated.
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// Calculates the plane fragments (blocking, one rayon task per layer).
    pub fn run(self) -> PlaneFragments {
        self.run_with_abort(&AtomicBool::new(false))
    }

    /// Like [`run()`](Self::run), but stops early (with
    /// [`PlaneFragments::finished`] = `false`) once `abort` is set.
    pub fn run_with_abort(mut self, abort: &AtomicBool) -> PlaneFragments {
        log::debug!(
            "Start calculating areas of {} plane(s) on {} layer(s)...",
            self.planes.len(),
            self.layers.len()
        );
        let mut result = PlaneFragments {
            board: self.board,
            layers: self.layers.iter().copied().collect(),
            planes: BTreeMap::new(),
            errors: Vec::new(),
            finished: false,
            revision: self.revision,
        };
        match self.preprocess() {
            Ok(board_area) => {
                let this = &self;
                let results: Vec<LayerResult> = self
                    .layers
                    .par_iter()
                    .map(|layer| this.run_layer(*layer, &board_area, abort))
                    .collect();
                for r in results {
                    result
                        .planes
                        .extend(r.planes.into_iter().map(|(k, v)| (PlaneId(k), v)));
                    result.errors.extend(r.errors);
                }
            }
            Err(e) => {
                log::error!("Failed to calculate plane fragments: {e}");
                result.errors.push(e.to_string());
            }
        }
        result.finished = !abort.load(Ordering::Relaxed);
        result
    }

    /// Preprocesses the data and returns the board area (upstream first
    /// part of `run()`).
    fn preprocess(&mut self) -> Result<ClipperPaths, ch::Error> {
        let tol = max_arc_tolerance();
        for zone in &mut self.keepout_zones {
            if zone.layers.contains(ZoneLayers::TOP) {
                zone.board_layers
                    .insert(zone.transform.map(&Layer::TOP_COPPER));
            }
            if zone.layers.contains(ZoneLayers::INNER) {
                zone.board_layers
                    .extend(self.layers.iter().filter(|l| l.is_inner()).copied());
            }
            if zone.layers.contains(ZoneLayers::BOTTOM) {
                zone.board_layers
                    .insert(zone.transform.map(&Layer::BOT_COPPER));
            }
            let mut closed = zone.outline.clone();
            closed.close();
            zone.outline = zone.transform.map(&closed);
        }
        for polygon in &mut self.polygons {
            polygon.path = polygon.transform.map(&polygon.path);
        }
        for (transform, _, path) in &mut self.holes {
            *path = transform.map(path);
        }
        for trace in std::mem::take(&mut self.traces) {
            self.polygons.push(PolygonData {
                transform: Transform::default(),
                layer: trace.layer,
                net: trace.net,
                path: Path::new(vec![
                    crate::geometry::Vertex::at(trace.start),
                    crate::geometry::Vertex::at(trace.end),
                ]),
                width: trace.width.into(),
                filled: false,
            });
        }

        // Determine board area.
        let mut outlines = Vec::new();
        let mut cutouts = Vec::new();
        for polygon in &self.polygons {
            if (polygon.layer == Layer::BOARD_OUTLINES) && polygon.path.is_closed() {
                outlines.push(polygon.path.clone());
            } else if (polygon.layer == Layer::BOARD_CUTOUTS) && polygon.path.is_closed() {
                cutouts.push(polygon.path.clone());
            }
        }
        let mut board_area = ch::paths_to_clipper(&outlines, tol);
        ch::subtract(
            &mut board_area,
            &ch::paths_to_clipper(&cutouts, tol),
            PolyFillType::NonZero,
            PolyFillType::NonZero,
        )?;

        // Sort planes: First by priority, then by uuid to get a really unique
        // priority order over all existing planes. This way we can ensure
        // that even planes with the same priority will always be filled in
        // the same order. Random order would be dangerous!
        self.planes.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| b.uuid.cmp(&a.uuid))
        });
        Ok(board_area)
    }

    /// Calculates the planes of one layer (upstream `runLayer()`).
    fn run_layer(
        &self,
        layer: Layer,
        board_area: &ClipperPaths,
        abort: &AtomicBool,
    ) -> LayerResult {
        let mut result = LayerResult::default();
        for (index, plane) in self.planes.iter().enumerate() {
            if plane.layer != layer {
                continue;
            }
            if abort.load(Ordering::Relaxed) {
                break;
            }
            match self.build_plane(plane, &self.planes[..index], board_area, &result) {
                Ok(fragments) => {
                    result.planes.insert(plane.uuid, fragments);
                }
                Err(e) => {
                    log::error!("Failed to calculate plane areas, leaving empty: {e}");
                    result.errors.push(e);
                }
            }
        }
        result
    }

    /// Calculates the fragments of one plane.
    fn build_plane(
        &self,
        plane: &PlaneData,
        previous_planes: &[PlaneData],
        board_area: &ClipperPaths,
        layer_result: &LayerResult,
    ) -> Result<Vec<Path>, String> {
        let tol = max_arc_tolerance();
        let err = |e: ch::Error| e.to_string();
        let mut removed_areas = ClipperPaths::new();
        let mut connected_areas = ClipperPaths::new();

        // Start with board outline shrunk by the given clearance and clipped
        // to the plane outline. Except if the board clearance is zero, in
        // this case we don't clip the plane to the board outlines.
        let mut closed_outline = plane.outline.clone();
        closed_outline.close();
        let plane_outline = ch::path_to_clipper(&closed_outline, tol);
        let mut fragments = match plane.min_clearance_to_board {
            Some(clearance) => {
                let mut fragments = board_area.clone();
                if *clearance != Length::ZERO {
                    ch::offset(&mut fragments, -*clearance, tol, JoinType::Round).map_err(err)?;
                }
                ch::intersect(
                    &mut fragments,
                    std::slice::from_ref(&plane_outline),
                    PolyFillType::EvenOdd,
                    PolyFillType::EvenOdd,
                )
                .map_err(err)?;
                fragments
            }
            None => vec![plane_outline.clone()],
        };
        let full_plane_area = fragments.clone();

        // Collect other planes.
        for other in previous_planes {
            if (other.layer == plane.layer) && (other.net != plane.net) {
                let clearance = plane
                    .min_clearance_to_copper
                    .max(other.min_clearance_to_copper);
                let other_fragments = layer_result
                    .planes
                    .get(&other.uuid)
                    .map_or(&[][..], Vec::as_slice);
                let mut paths = ch::paths_to_clipper(other_fragments, tol);
                ch::offset(&mut paths, *clearance, tol, JoinType::Round).map_err(err)?;
                removed_areas.extend(paths);
            }
        }

        // Collect keepout zones.
        for zone in &self.keepout_zones {
            if zone.board_layers.contains(&plane.layer) {
                removed_areas.push(ch::path_to_clipper(&zone.outline, tol));
            }
        }

        // Collect holes, if clearance to holes is enabled.
        if let Some(clearance) = plane.min_clearance_to_npth {
            for (_, diameter, path) in &self.holes {
                let diameter = positive(**diameter + *clearance * 2)?;
                let paths = path.to_outline_strokes(diameter);
                removed_areas.extend(ch::paths_to_clipper(&paths, tol));
            }
        }

        // Collect vias.
        let layer_number = plane.layer.copper_number();
        for via in &self.vias {
            if (via.start_layer.copper_number() > layer_number)
                || (via.end_layer.copper_number() < layer_number)
            {
                continue;
            }
            if plane.net.is_some() && (via.net == plane.net) {
                // Via has same net as plane -> no cut-out.
                // Note: Do not respect the plane connect style for vias, but
                // always connect them with solid style. Since vias are not
                // soldered, heat dissipation is not an issue or often even
                // desired. See discussion
                // https://github.com/LibrePCB/LibrePCB/issues/454#issuecomment-1373402172
                let path = Path::circle(via.diameter).translated(via.position);
                connected_areas.push(ch::path_to_clipper(&path, tol));
            } else {
                // Via has different net than plane -> subtract with clearance.
                let dia = positive(*via.diameter + *plane.min_clearance_to_copper * 2)?;
                let path = Path::circle(dia).translated(via.position);
                removed_areas.push(ch::path_to_clipper(&path, tol));
            }
        }

        // Collect traces & other strokes.
        for polygon in &self.polygons {
            if polygon.layer != plane.layer {
                continue;
            }
            if plane.net.is_some() && (polygon.net == plane.net) {
                // Same net signal -> memorize as connected area.
                if polygon.filled {
                    connected_areas.push(ch::path_to_clipper(&polygon.path, tol));
                }
                if !polygon.filled || (*polygon.width > Length::ZERO) {
                    let width = positive((*polygon.width).max(Length::new(1)))?;
                    let paths = polygon.path.to_outline_strokes(width);
                    connected_areas.extend(ch::paths_to_clipper(&paths, tol));
                }
            } else {
                // Different net signal -> subtract with clearance.
                if polygon.filled {
                    let mut paths = vec![ch::path_to_clipper(&polygon.path, tol)];
                    ch::offset(
                        &mut paths,
                        *plane.min_clearance_to_copper,
                        tol,
                        JoinType::Round,
                    )
                    .map_err(err)?;
                    removed_areas.extend(paths);
                }
                if !polygon.filled || (*polygon.width > Length::ZERO) {
                    let width = positive(
                        (*polygon.width + *plane.min_clearance_to_copper * 2).max(Length::new(1)),
                    )?;
                    let paths = polygon.path.to_outline_strokes(width);
                    removed_areas.extend(ch::paths_to_clipper(&paths, tol));
                }
            }
        }

        // Collect pads.
        let mut thermal_pad_areas = ClipperPaths::new();
        let mut thermal_pad_areas_shrunk = ClipperPaths::new();
        let mut thermal_pad_clearance_areas = ClipperPaths::new();
        let map_outlines = |transform: &Transform, geometry: &PadGeometry| {
            geometry
                .to_outlines()
                .map(|outlines| ch::paths_to_clipper(&transform.map(&outlines), tol))
                .map_err(err)
        };
        for pad in &self.pads {
            let same_net = plane.net.is_some() && (pad.net == plane.net);
            let geometries = pad
                .geometries
                .get(&plane.layer)
                .map_or(&[][..], Vec::as_slice);
            for geometry in geometries {
                if same_net {
                    // Same net signal -> memorize as connected area.
                    connected_areas.extend(map_outlines(&pad.transform, geometry)?);
                }
                if !same_net || (plane.connect_style != PlaneConnectStyle::Solid) {
                    // Determine required clearance. For connection style
                    // 'none' for pads of the same net, use the thermal gap
                    // clearance since usually it is smaller than the planes
                    // clearance, so it leads to a higher plane area.
                    let clearance = if same_net {
                        *plane.thermal_gap
                    } else {
                        *plane.min_clearance_to_copper
                    }
                    .max(*pad.clearance);
                    let mut clipper_paths =
                        map_outlines(&pad.transform, &geometry.with_offset(clearance))?;

                    // For thermal relief connection, subtract the spokes from
                    // the cutout.
                    if same_net
                        && (plane.connect_style == PlaneConnectStyle::ThermalRelief)
                        && ch::any_points_of_paths_inside(&clipper_paths, &plane_outline)
                    {
                        // Note: Make spokes *slightly* thicker to avoid them to
                        // be removed due to numerical inaccuary of minimum width
                        // procedure.
                        // Note 2: Do not accept a spoke width smaller than the
                        // minimum copper width because this will effectively
                        // break all thermal pads, see
                        // https://github.com/LibrePCB/LibrePCB/issues/1682.
                        let spoke_width = positive(
                            (*plane.thermal_spoke_width).max(*plane.min_width) + Length::new(10),
                        )?;
                        let spoke_length = Length::new(100_000_000); // Maximum spoke length.
                        let rotation = pad.transform.rotation;
                        let position = pad.transform.position;
                        for (start, angle) in determine_thermal_spokes(geometry) {
                            let p1 = start.rotated(rotation, Point::ORIGIN) + position;
                            let p2 = (Point::new(spoke_length, Length::ZERO)
                                .rotated(angle, Point::ORIGIN)
                                + start)
                                .rotated(rotation, Point::ORIGIN)
                                + position;
                            let spoke = vec![ch::path_to_clipper(
                                &Path::obround_line(p1, p2, spoke_width),
                                tol,
                            )];
                            ch::subtract(
                                &mut clipper_paths,
                                &spoke,
                                PolyFillType::EvenOdd,
                                PolyFillType::NonZero,
                            )
                            .map_err(err)?;
                        }
                        // Memorize copper area for later removal of unconnected
                        // thermal spokes.
                        let mut tmp = map_outlines(&pad.transform, geometry)?;
                        if tmp.len() > 1 {
                            ch::unite(&mut tmp, PolyFillType::NonZero).map_err(err)?;
                        }
                        thermal_pad_areas.extend(tmp);
                        // Memorize clearance area for later removal of
                        // unconnected thermal spokes.
                        let offset = clearance + *plane.min_width - *tol - Length::new(10);
                        let mut tmp = map_outlines(&pad.transform, &geometry.with_offset(offset))?;
                        if tmp.len() > 1 {
                            ch::unite(&mut tmp, PolyFillType::NonZero).map_err(err)?;
                        }
                        thermal_pad_clearance_areas.extend(tmp);
                        // Memorize slightly shrunk copper area for later
                        // removal of unconnected thermal spokes.
                        let offset = -*tol - Length::new(10);
                        let tmp = map_outlines(&pad.transform, &geometry.with_offset(offset))?;
                        thermal_pad_areas_shrunk.extend(tmp);
                    }
                    removed_areas.extend(clipper_paths);

                    // Also create cut-outs for each hole to ensure correct
                    // clearance even if the pad outline is too small or
                    // invalid.
                    if !same_net {
                        for hole in geometry.holes() {
                            let width = positive(*hole.diameter() + clearance * 2)?;
                            let paths = pad.transform.map(&hole.path().to_outline_strokes(width));
                            removed_areas.extend(ch::paths_to_clipper(&paths, tol));
                        }
                    }
                }
            }
        }

        // Subtract all the collected areas to remove.
        ch::subtract(
            &mut fragments,
            &removed_areas,
            PolyFillType::EvenOdd,
            PolyFillType::NonZero,
        )
        .map_err(err)?;

        // Ensure minimum width. Reduce minWidth by 1nm to ensure plane areas
        // do not disappear between two objects with a distance of *exactly*
        // 2*minClearance+minWidth (e.g. two 0.5mm traces on a 1.0mm grid).
        let min_width_offset = (*plane.min_width / 2) - Length::new(1);
        if min_width_offset > Length::ZERO {
            ch::offset(&mut fragments, -min_width_offset, tol, JoinType::Round).map_err(err)?;
            ch::offset(&mut fragments, min_width_offset, tol, JoinType::Round).map_err(err)?;
        }

        // Split thermal spokes and flatten result for detecting unconnected
        // thermal spokes.
        let tree = ch::subtract_to_tree(
            &fragments,
            &thermal_pad_areas_shrunk,
            PolyFillType::EvenOdd,
            PolyFillType::NonZero,
            true,
        )
        .map_err(err)?;
        fragments = ch::flatten_tree(tree.root()).map_err(err)?;

        // Remove unconnected thermal spokes.
        if thermal_pad_areas.len() != thermal_pad_clearance_areas.len() {
            return Err("Thermal pads inconsistency, please open a bug report.".to_owned());
        }
        let is_unconnected_spoke = |fragment: &ClipperPath| {
            let mut pad_index = None;
            for (i, area) in thermal_pad_areas.iter().enumerate() {
                if ch::any_points_inside(fragment, area) {
                    if pad_index.is_some() {
                        return false;
                    }
                    pad_index = Some(i);
                }
            }
            pad_index
                .is_some_and(|i| ch::all_points_inside(fragment, &thermal_pad_clearance_areas[i]))
        };
        fragments.retain(|f| !is_unconnected_spoke(f));

        // Fill thermal pads.
        ch::intersect(
            &mut thermal_pad_areas,
            &full_plane_area,
            PolyFillType::NonZero,
            PolyFillType::EvenOdd,
        )
        .map_err(err)?;
        let tree = ch::unite_with_to_tree(
            &fragments,
            &thermal_pad_areas,
            PolyFillType::EvenOdd,
            PolyFillType::NonZero,
        )
        .map_err(err)?;
        fragments = ch::flatten_tree(tree.root()).map_err(err)?;

        // If requested, remove unconnected fragments (islands).
        if plane.net.is_some() && !plane.keep_islands {
            let mut kept = ClipperPaths::with_capacity(fragments.len());
            for fragment in fragments {
                let mut intersections = vec![fragment.clone()];
                ch::intersect(
                    &mut intersections,
                    &connected_areas,
                    PolyFillType::NonZero,
                    PolyFillType::NonZero,
                )
                .map_err(err)?;
                if !intersections.is_empty() {
                    kept.push(fragment);
                }
            }
            fragments = kept;
        }

        // Make result canonical for a reproducible output by rotating and
        // sorting the fragments.
        let key = |p: &IntPoint| (p.x, p.y);
        for path in &mut fragments {
            if let Some(min_index) = path
                .iter()
                .enumerate()
                .min_by_key(|(i, p)| (key(p), *i))
                .map(|(i, _)| i)
            {
                path.rotate_left(min_index);
            }
        }
        fragments.retain(|p| !p.is_empty());
        fragments.sort_by_key(|p| key(&p[0]));

        Ok(ch::paths_from_clipper(&fragments))
    }
}

/// Returns the thermal spokes (start point and direction) of a pad geometry
/// (upstream `determineThermalSpokes()`).
fn determine_thermal_spokes(geometry: &PadGeometry) -> Vec<(Point, Angle)> {
    let vertices = geometry.path().vertices();
    // For circular pads, rotate spokes by 45° since this often allows to add
    // more spokes if several pads are placed in a row.
    let is_circular_round = matches!(
        geometry.shape(),
        PadGeometryShape::RoundedRect | PadGeometryShape::RoundedOctagon
    ) && (geometry.width() == geometry.height())
        && (*geometry.corner_radius() >= (geometry.width() / 2));
    let is_circular_stroke =
        (geometry.shape() == PadGeometryShape::Stroke) && (vertices.len() == 1);
    if is_circular_round || is_circular_stroke {
        let center = if is_circular_stroke {
            vertices[0].pos
        } else {
            Point::ORIGIN
        };
        return vec![
            (center, Angle::DEG45),
            (center, Angle::DEG135),
            (center, Angle::DEG225),
            (center, Angle::DEG315),
        ];
    }

    // For any shape other than a complex stroke, add horizontal and vertical
    // spokes.
    let is_centered_shape = geometry.shape() != PadGeometryShape::Stroke;
    let is_obround_stroke = (geometry.shape() == PadGeometryShape::Stroke)
        && (vertices.len() == 2)
        && (vertices[0].angle == Angle::DEG0);
    if is_centered_shape || is_obround_stroke {
        let mut center = Point::ORIGIN;
        let mut angle = Angle::DEG0;
        if is_obround_stroke {
            let p1 = vertices[0].pos;
            let p2 = vertices[vertices.len() - 1].pos;
            center = (p1 + p2) / 2;
            angle = toolbox::angle_between_points(p1, p2);
        }
        return vec![
            (center, angle),
            (center, angle + Angle::DEG90),
            (center, angle + Angle::DEG180),
            (center, angle + Angle::DEG270),
        ];
    }

    // For complex strokes, add two 45° spokes on each end.
    if vertices.len() > 1 {
        let p1 = vertices[0].pos;
        let p2 = vertices[vertices.len() - 1].pos;
        let angle = toolbox::angle_between_points(p1, p2);
        return vec![
            (p1, angle + Angle::DEG135),
            (p1, angle - Angle::DEG135),
            (p2, angle + Angle::DEG45),
            (p2, angle - Angle::DEG45),
        ];
    }

    // For invalid strokes, add no spokes at all.
    Vec::new()
}
