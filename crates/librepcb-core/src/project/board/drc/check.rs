//! Port of libs/librepcb/core/project/board/drc/boarddesignrulecheck.{h,cpp}.
//!
//! [`run_drc()`] runs all checks on a [`BoardDrcData`] snapshot, in
//! parallel like upstream (stage 1: copper areas per layer; stage 2: checks
//! depending on them; independent checks in parallel to both; trivial
//! checks sequentially in the calling thread), using scoped threads instead
//! of `QtConcurrent`. Progress is reported through a callback instead of
//! Qt signals. The messages are collected in the same order as upstream
//! (sequential checks first, then the parallel checks in the order they
//! were started).
//!
//! [`Project::run_drc()`] is the equivalent of upstream `start()` +
//! `waitForFinished()`: it rebuilds the air wires, extracts the data and
//! runs the checks.
//!
//! Differences to upstream:
//!
//! - Checks upstream implements with `QPainterPath` predicates (keepout
//!   zones, pad connections) use the ported Qt predicates of
//!   [`PainterPathPx`]. The union of the object areas (`QPainterPath::|=`,
//!   i.e. `QPathClipper`) is not ported: an object intersects a zone if
//!   any of its areas does, and via circles are drawn as arcs instead of
//!   `addEllipse()` (same Bézier curves). See COMPAT.md.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use clipper::JoinType;
use clipper::PolyFillType::{EvenOdd, NonZero};
use librepcb_i18n::{tr, trn};
use rayon::prelude::*;
use rstar::{
    AABB, Envelope, RTree,
    primitives::{GeomWithData, Rectangle},
};

use super::data::{
    BoardDrcData, DrcAirWireAnchor, DrcAirWireObject, DrcDevice, DrcHole, DrcPad, DrcPlane,
    DrcPolygon, DrcSegment, DrcTrace, DrcVia, DrcZone,
};
use super::error::{Error, Result};
use super::geometry_generator::{GeometryGenerator, has_geometry};
use super::messages::{
    ConnectionAnchor, CopperObject, DrcHoleRef, DrcMessage, ZoneObject, pad_transform,
};
use crate::geometry::{NonEmptyPath, Path, Vertex, Via, ZoneLayers, ZoneRules};
use crate::library::org::{AllowedSlots, BoardDesignRuleCheckSettings};
use crate::project::{BoardId, Project};
use crate::types::{Angle, Layer, Length, Point, PositiveLength, UnsignedLength};
use crate::utils::clipper_helpers::{self, ClipperPaths};
use crate::utils::painter_path::PainterPathPx;
use crate::utils::rational_union::{self, Operand, Region};
use crate::utils::transform::Transform;

const CTX: &str = "librepcb::BoardDesignRuleCheck";

/// Progress events of the design rule check (upstream signals
/// `progressPercent()` and `progressStatus()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrcProgress<'a> {
    /// Overall progress in percent (0..=100).
    Percent(u32),
    /// Status message (translated), e.g. "Check copper clearances...".
    Status(&'a str),
}

/// A progress callback; it is called from several threads.
pub type DrcProgressCallback<'a> = &'a (dyn Fn(DrcProgress<'_>) + Sync);

/// The result of a design rule check (upstream
/// `BoardDesignRuleCheck::Result`).
#[derive(Debug, Clone, Default)]
pub struct DrcResult {
    snapshot: Option<super::job::DrcSnapshot>,
    /// All emitted messages (approved or not).
    pub messages: Vec<DrcMessage>,
    /// Errors of failed checks (empty on success).
    pub errors: Vec<String>,
    /// Whether only the quick check was run.
    pub quick: bool,
    /// Duration of the check.
    pub elapsed: Duration,
}

impl DrcResult {
    /// Whether all full-check jobs finished successfully on an identified snapshot.
    pub fn is_complete(&self) -> bool {
        self.snapshot.is_some() && !self.quick && self.errors.is_empty()
    }

    /// Whether the result belongs to this board and the current project revision.
    pub fn is_current(&self, project: &Project, board: BoardId) -> bool {
        self.snapshot.is_some_and(|s| s.matches(project, board))
    }
}

/// Returns the maximum allowed arc tolerance when flattening arcs.
fn max_arc_tolerance() -> PositiveLength {
    PositiveLength::new(Length::new(5000)).expect("constant is positive")
}

fn positive(length: Length) -> Result<PositiveLength> {
    Ok(PositiveLength::new(length)?)
}

fn unsigned(length: Length) -> Result<UnsignedLength> {
    Ok(UnsignedLength::new(length)?)
}

fn to_clipper(paths: &[Path]) -> ClipperPaths {
    clipper_helpers::paths_to_clipper(paths, max_arc_tolerance())
}

fn from_clipper(paths: &[clipper_helpers::ClipperPath]) -> Vec<Path> {
    clipper_helpers::paths_from_clipper(paths)
}

fn closed(path: &Path) -> Path {
    let mut p = path.clone();
    p.close();
    p
}

/// Intersects `subject` with `clip` (even-odd) and returns the flattened
/// result as paths.
fn intersection(subject: &ClipperPaths, clip: &ClipperPaths) -> Result<Vec<Path>> {
    let tree = clipper_helpers::intersect_to_tree(subject, clip, EvenOdd, EvenOdd, true)?;
    Ok(from_clipper(&clipper_helpers::flatten_tree(tree.root())?))
}

/// Convert only established violations. Keep even collapsed display polygons:
/// an integer-grid rendering must never suppress an exact nonempty result.
fn exact_locations(region: &Region) -> Result<Vec<Path>> {
    if region.is_empty() {
        return Ok(Vec::new());
    }
    let rounded = region.rounded_paths()?;
    // This Clipper call only prepares the display polygons (including cut-ins
    // for holes). It cannot change whether a violation is emitted.
    let tree = clipper_helpers::unite_to_tree(&rounded, NonZero)?;
    let display = clipper_helpers::flatten_tree(tree.root())?;
    Ok(from_clipper(if display.is_empty() {
        &rounded
    } else {
        &display
    }))
}

fn exact_intersection(a: &Region, b: &Region) -> Result<Vec<Path>> {
    exact_locations(&a.intersection(b)?)
}

/// Stroke exact open violation intervals without adding a closing chord.
fn exact_line_locations(paths: &[Vec<rational_union::Point>]) -> Result<Vec<Path>> {
    let width = positive(Length::new(100_000))?;
    let mut locations = Vec::new();
    for path in paths {
        let points = path
            .iter()
            .map(|p| p.rounded())
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let vertices = points
            .iter()
            .map(|p| Vertex::at(Point::from_nm(p.x, p.y)))
            .collect();
        // A collapsed nonempty interval still gets a circular display marker.
        locations.extend(Path::new(vertices).to_outline_strokes(width));
    }
    Ok(locations)
}

/// Data calculated by stage 1 jobs (upstream `CalculatedJobData`).
#[derive(Debug, Clone, Default)]
struct CalculatedJobData {
    copper_paths_per_layer: BTreeMap<Layer, Region>,
}

impl CalculatedJobData {
    fn copper(&self, layer: Layer) -> Region {
        self.copper_paths_per_layer
            .get(&layer)
            .cloned()
            .unwrap_or_default()
    }
}

type Messages = Result<Vec<DrcMessage>>;
type IndependentCheck<'a> = fn(&Checker<'a>, &BoardDrcData) -> Messages;
type Stage2Check<'a> = fn(&Checker<'a>, &BoardDrcData, &CalculatedJobData) -> Messages;

/// Runs the checks and reports progress.
struct Checker<'a> {
    progress: DrcProgressCallback<'a>,
    cancelled: &'a (dyn Fn() -> bool + Sync),
    skipped: std::sync::atomic::AtomicBool,
    /// Total weight and weight of the finished jobs.
    counter: Mutex<(u32, u32)>,
}

impl Checker<'_> {
    fn percent(&self, percent: u32) {
        (self.progress)(DrcProgress::Percent(percent));
    }

    fn status(&self, status: &str) {
        (self.progress)(DrcProgress::Status(status));
    }

    /// Upstream `tryRunJob()`: runs a job, converts its error and updates
    /// the progress.
    fn try_run(&self, weight: u32, job: impl FnOnce() -> Messages) -> DrcResult {
        let mut result = DrcResult::default();
        if (self.cancelled)() {
            self.skipped
                .store(true, std::sync::atomic::Ordering::Relaxed);
            return result;
        }
        match job() {
            Ok(messages) => result.messages = messages,
            Err(e) => {
                log::error!("DRC check failed with exception: {e}");
                result.errors.push(e.to_string());
            }
        }
        // A poisoned lock only means another job panicked; the counter is
        // still valid.
        let mut counter = self
            .counter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        counter.1 += weight;
        let percent = (counter.1 * 80)
            .checked_div(counter.0)
            .map_or(100, |p| (20 + p).min(100));
        drop(counter);
        self.percent(percent);
        result
    }
}

fn append(result: &mut DrcResult, other: DrcResult) {
    result.messages.extend(other.messages);
    result.errors.extend(other.errors);
}

/// Runs the design rule check on `data` (upstream `BoardDesignRuleCheck::run()`).
pub fn run_drc(data: &BoardDrcData, progress: DrcProgressCallback<'_>) -> DrcResult {
    run_drc_with_cancel(data, progress, &|| false)
}

/// Stop starting checks after cancellation, joining checks already in flight.
fn run_drc_with_cancel(
    data: &BoardDrcData,
    progress: DrcProgressCallback<'_>,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> DrcResult {
    let timer = Instant::now();
    let checker = Checker {
        progress,
        cancelled,
        skipped: std::sync::atomic::AtomicBool::new(false),
        counter: Mutex::new((0, 0)),
    };
    checker.percent(15);

    // Determine jobs to execute, in the order how they should be started.
    let mut stage2: Vec<(Stage2Check<'_>, u32)> = vec![(Checker::check_copper_hole_clearances, 3)];
    if !data.quick {
        stage2.push((Checker::check_minimum_pth_annular_ring, 2));
        stage2.push((Checker::check_board_cutouts, 3));
    }
    let mut independent: Vec<(IndependentCheck<'_>, u32)> = vec![
        (Checker::check_copper_copper_clearances, 5),
        (Checker::check_copper_board_clearances, 3),
    ];
    if !data.quick {
        independent.extend::<[(IndependentCheck<'_>, u32); 8]>([
            (Checker::check_drill_drill_clearances, 2),
            (Checker::check_drill_board_clearances, 2),
            (Checker::check_silkscreen_stopmask_clearances, 2),
            (Checker::check_zones, 2),
            (Checker::check_invalid_pad_connections, 2),
            (Checker::check_device_clearances, 2),
            (Checker::check_board_outline, 1),
            (Checker::check_vias, 1),
        ]);
    }
    let mut sequential: Vec<IndependentCheck<'_>> = vec![Checker::check_minimum_copper_width];
    if !data.quick {
        sequential.extend::<[IndependentCheck<'_>; 14]>([
            Checker::check_planes,
            Checker::check_allowed_npth_slots,
            Checker::check_allowed_pth_slots,
            Checker::check_used_layers,
            Checker::check_for_unplaced_components,
            Checker::check_for_missing_connections,
            Checker::check_for_impossible_connections,
            Checker::check_for_stale_objects,
            Checker::check_minimum_silkscreen_width,
            Checker::check_minimum_silkscreen_text_height,
            Checker::check_minimum_npth_drill_diameter,
            Checker::check_minimum_npth_slot_width,
            Checker::check_minimum_pth_drill_diameter,
            Checker::check_minimum_pth_slot_width,
        ]);
    }

    // Calculate total jobs weight. After this, progress is determined by
    // the number of executed jobs.
    let stage1_weight = 3 * data.copper_layers.len() as u32;
    let total = stage1_weight
        + stage2.iter().map(|(_, w)| w).sum::<u32>()
        + independent.iter().map(|(_, w)| w).sum::<u32>()
        + sequential.len() as u32;
    *checker
        .counter
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = (total, 0);
    checker.percent(20);

    let mut result = DrcResult {
        snapshot: Some(data.snapshot),
        quick: data.quick,
        ..DrcResult::default()
    };
    let checker = &checker;
    let calc_cell = std::sync::OnceLock::new();
    let calc_cell = &calc_cell;
    std::thread::scope(|scope| {
        // Start all stage 1 & independent jobs.
        let stage1_handles: Vec<_> = data
            .copper_layers
            .iter()
            .map(|&layer| {
                scope.spawn(move || {
                    let mut paths = None;
                    let r = checker.try_run(3, || {
                        paths = Some(checker.prepare_copper_paths(data, layer)?);
                        Ok(Vec::new())
                    });
                    (layer, paths, r)
                })
            })
            .collect();
        let independent_handles: Vec<_> = independent
            .iter()
            .map(|&(check, weight)| {
                scope.spawn(move || checker.try_run(weight, || check(checker, data)))
            })
            .collect();

        // Collect results of stage 1 jobs.
        let mut calc = CalculatedJobData::default();
        for handle in stage1_handles {
            match handle.join() {
                Ok((layer, paths, r)) => {
                    if let Some(paths) = paths {
                        calc.copper_paths_per_layer.insert(layer, paths);
                    }
                    append(&mut result, r);
                }
                Err(_) => result.errors.push("DRC job panicked.".to_owned()),
            }
        }

        // Start all stage 2 jobs.
        let calc: &CalculatedJobData = calc_cell.get_or_init(|| calc);
        let stage2_handles: Vec<_> = stage2
            .iter()
            .map(|&(check, weight)| {
                scope.spawn(move || checker.try_run(weight, || check(checker, data, calc)))
            })
            .collect();

        // Run all trivial jobs synchronously.
        for check in &sequential {
            let r = checker.try_run(1, || check(checker, data));
            append(&mut result, r);
        }

        // Collect results of independent & stage 2 jobs (in the order they
        // were added upstream: stage 2 jobs first).
        for handle in stage2_handles.into_iter().chain(independent_handles) {
            match handle.join() {
                Ok(r) => append(&mut result, r),
                Err(_) => result.errors.push("DRC job panicked.".to_owned()),
            }
        }
    });

    // Finished!
    result.elapsed = timer.elapsed();
    if checker.skipped.load(std::sync::atomic::Ordering::Relaxed) {
        result.errors.push("DRC cancelled.".to_owned());
    }
    log::debug!(
        "{} {} after {} ms.",
        if data.quick { "Quick check" } else { "DRC" },
        if result.errors.is_empty() {
            "succeeded"
        } else {
            "failed"
        },
        result.elapsed.as_millis()
    );
    let count = result.messages.len();
    checker.status(&trn!(
        CTX,
        "Finished with {0} message(s)!",
        "Finished with {0} message(s)!",
        count,
        count
    ));
    checker.percent(100);
    result
}

impl Project {
    /// Runs the design rule check of a board (upstream
    /// `BoardDesignRuleCheck::start()` + `waitForFinished()`), with the
    /// board's DRC settings unless `settings` is given.
    ///
    /// For a full check (not `quick`), all planes and the air wires are
    /// rebuilt first.
    /// Errors of single checks are reported in [`DrcResult::errors`];
    /// an error is returned if preparation fails, including a failed plane rebuild.
    pub fn run_drc(
        &mut self,
        board: BoardId,
        settings: Option<&BoardDesignRuleCheckSettings>,
        quick: bool,
        progress: DrcProgressCallback<'_>,
    ) -> Result<DrcResult> {
        self.run_drc_with_cancel(board, settings, quick, progress, &|| false)
    }

    /// Run DRC with a cooperative stop check at each check boundary.
    pub fn run_drc_with_cancel(
        &mut self,
        board: BoardId,
        settings: Option<&BoardDesignRuleCheckSettings>,
        quick: bool,
        progress: DrcProgressCallback<'_>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<DrcResult> {
        progress(DrcProgress::Percent(1));

        let job = self.drc_job(board, settings, quick)?;
        if !quick {
            progress(DrcProgress::Status(&tr!(CTX, "Rebuild planes...")));
        }
        let prepared = job.run();
        progress(DrcProgress::Percent(7));
        let data = prepared.apply(self)?;
        progress(DrcProgress::Percent(12));
        Ok(run_drc_with_cancel(&data, progress, cancelled))
    }

    /// Detects DRC message approvals which became obsolete (port of
    /// `Board::updateDrcMessageApprovals()`) and returns the mutation
    /// removing them, or `None` if nothing changes. The caller applies it
    /// (upstream modifies the board without undo command and marks the
    /// project as modified).
    ///
    /// Failed, unidentified or outdated results never change approvals.
    /// Records the approvals of `result` as occurring in this session
    /// (derived data of the board). After a quick check nothing is removed,
    /// since messages of the skipped checks would lose their approvals.
    /// On the first full check after a file format upgrade (approvals
    /// version older than the current file format), all approvals not
    /// occurring anymore are removed and the version is updated; otherwise
    /// only approvals of messages which disappeared during this session
    /// are removed, to keep approvals added by newer minor versions.
    pub fn drc_approvals_update(
        &mut self,
        board: BoardId,
        result: &DrcResult,
    ) -> crate::project::Result<Option<crate::project::Mutation>> {
        use crate::project::{BoardMutation, Mutation};
        let index = self
            .board_index(board)
            .ok_or(crate::project::Error::NotFound {
                kind: crate::project::EntityKind::Board,
                uuid: board.0,
            })?;
        if !result.is_current(self, board) || !result.errors.is_empty() {
            return Ok(None);
        }
        let b = &mut self.boards[index];
        let approvals: BTreeSet<_> = result
            .messages
            .iter()
            .map(|m| m.approval().clone())
            .collect();
        b.derived
            .supported_drc_approvals
            .extend(approvals.iter().cloned());
        if !result.is_complete() {
            return Ok(None);
        }
        let current = crate::application::file_format_version();
        let (version, new) = if b.drc_approvals_version < current {
            let new = b.drc_approvals.intersection(&approvals).cloned().collect();
            (current, new)
        } else {
            let supported = &b.derived.supported_drc_approvals;
            let new: BTreeSet<_> = b
                .drc_approvals
                .iter()
                .filter(|a| !supported.contains(*a) || approvals.contains(*a))
                .cloned()
                .collect();
            if new == b.drc_approvals {
                return Ok(None);
            }
            (b.drc_approvals_version.clone(), new)
        };
        Ok(Some(Mutation::Board(BoardMutation::SetDrcApprovals {
            board,
            version,
            approvals: new,
        })))
    }
}

/// An item of the copper clearance check.
struct CopperItem<'a> {
    object: CopperObject<'a>,
    serialized: crate::serialization::List,
    start_layer: Layer,
    end_layer: Layer,
    net: Option<crate::types::Uuid>,
    clearance: Length,
    copper_area: Region,
    clearance_area: Region,
}

/// Broad-phase bounds covering both directions of a clearance check.
fn copper_envelope(copper: &Region, clearance: &Region) -> Result<Option<AABB<[i128; 2]>>> {
    let a = copper.envelope()?;
    let b = clearance.envelope()?;
    Ok(match (a, b) {
        (Some(a), Some(b)) => Some(a.merged(&b)),
        (a, b) => a.or(b),
    })
}

fn copper_layer_mask(start: Layer, end: Layer, layers: &BTreeSet<Layer>) -> u64 {
    layers
        .iter()
        .filter(|layer| {
            (start.copper_number()..=end.copper_number()).contains(&layer.copper_number())
        })
        .fold(0, |mask, layer| mask | (1u64 << layer.copper_number()))
}

/// A copper clearance violation (merged per object pair).
#[derive(Debug, PartialEq, Eq)]
struct CopperViolation {
    obj1: usize,
    obj2: usize,
    layers: u64,
    clearance: Length,
    locations: Vec<Path>,
}

/// Evaluate independent groups of copper pairs in parallel, then merge in the
/// original (i, k) order so messages, locations and approvals remain stable.
fn check_copper_pairs(
    items: &[CopperItem<'_>],
    layers: &BTreeSet<Layer>,
) -> Result<Vec<CopperViolation>> {
    // Index both areas: custom clearances require checking either direction.
    // i128 accommodates areas and squared distances over the full input range.
    let envelopes = items
        .iter()
        .map(|item| copper_envelope(&item.copper_area, &item.clearance_area))
        .collect::<Result<Vec<_>>>()?;
    let index = RTree::bulk_load(
        envelopes
            .iter()
            .enumerate()
            .filter_map(|(i, bounds)| {
                bounds.map(|bounds| GeomWithData::new(Rectangle::from_aabb(bounds), i))
            })
            .collect(),
    );
    let layer_masks: Vec<u64> = items
        .iter()
        .map(|item| copper_layer_mask(item.start_layer, item.end_layer, layers))
        .collect();

    // Now check for intersections.
    // Indexed parallel collection retains the original i order, including errors.
    // Each worker keeps only violations, not a board-wide quadratic pair list.
    let results: Vec<Result<Vec<CopperViolation>>> = (0..items.len())
        .into_par_iter()
        .map(|i| {
            let mut violations = Vec::new();
            let Some(envelope) = &envelopes[i] else {
                return Ok(violations);
            };
            let mut candidates: Vec<_> = index
                .locate_in_envelope_intersecting(*envelope)
                .map(|entry| entry.data)
                .filter(|&k| k > i)
                .collect();
            // Tree traversal order must not affect messages or approval handling.
            candidates.sort_unstable();
            for k in candidates {
                let (it1, it2) = (&items[i], &items[k]);
                if !(((it1.clearance > Length::ZERO) || (it2.clearance > Length::ZERO))
                    && ((it1.net != it2.net) || it1.net.is_none() || it2.net.is_none()))
                {
                    continue;
                }
                let overlapping = layer_masks[i] & layer_masks[k];
                if overlapping == 0 {
                    continue;
                }
                let mut locations = exact_intersection(&it1.copper_area, &it2.clearance_area)?;
                // Perform the check the other way around only if:
                //  - Either the two items have individual clearances
                //  - Or there are any intersections -> show both violations
                if (it1.clearance != it2.clearance) || !locations.is_empty() {
                    locations.extend(exact_intersection(&it2.copper_area, &it1.clearance_area)?);
                }
                if locations.is_empty() {
                    continue;
                }
                let clearance = it1.clearance.max(it2.clearance);
                violations.push(CopperViolation {
                    obj1: i,
                    obj2: k,
                    layers: overlapping,
                    clearance,
                    locations,
                });
            }
            Ok(violations)
        })
        .collect();
    merge_copper_violations(items, results)
}

/// Merge in input order after every worker has finished. In particular, do not
/// collect the parallel iterator into Result: its chosen error is unspecified.
fn merge_copper_violations(
    items: &[CopperItem<'_>],
    results: impl IntoIterator<Item = Result<Vec<CopperViolation>>>,
) -> Result<Vec<CopperViolation>> {
    let mut violations: Vec<CopperViolation> = Vec::new();
    for result in results {
        for next in result? {
            let (it1, it2) = (&items[next.obj1], &items[next.obj2]);
            let existing = violations.iter_mut().find(|v| {
                let (s1, s2) = (&items[v.obj1].serialized, &items[v.obj2].serialized);
                ((&it1.serialized == s1) && (&it2.serialized == s2))
                    || ((&it1.serialized == s2) && (&it2.serialized == s1))
            });
            match existing {
                Some(v) => {
                    v.layers |= next.layers;
                    v.clearance = v.clearance.max(next.clearance);
                    v.locations.extend(next.locations);
                }
                None => violations.push(next),
            }
        }
    }
    Ok(violations)
}

impl<'a> CopperItem<'a> {
    fn new(
        object: CopperObject<'a>,
        layers: (Layer, Layer),
        net: Option<crate::types::Uuid>,
        clearance: Length,
        copper_area: Region,
        clearance_area: Region,
    ) -> Self {
        Self {
            serialized: object.serialize(),
            object,
            start_layer: layers.0,
            end_layer: layers.1,
            net,
            clearance,
            copper_area,
            clearance_area,
        }
    }
}

/// Object of the keepout zone check with its placement.
struct ZoneItem<'a> {
    zone: &'a DrcZone,
    device: Option<&'a DrcDevice>,
    outline: Path,
    layers: BTreeSet<Layer>,
    rules: ZoneRules,
}

impl Checker<'_> {
    fn prepare_copper_paths(&self, data: &BoardDrcData, layer: Layer) -> Result<Region> {
        self.status(&tr!(CTX, "Prepare '{0}'...", layer.name_tr()));
        let mut generator = GeometryGenerator::new(max_arc_tolerance());
        generator.add_copper(data, layer, &[], data.quick)?;
        generator.take_region()
    }

    fn check_copper_copper_clearances(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();

        // Skip this check if no minimum copper clearances are configured.
        let any_clearance_set = (*data.settings.min_copper_copper_clearance() > Length::ZERO)
            || data
                .net_classes
                .values()
                .any(|nc| *nc.min_copper_copper_clearance > Length::ZERO);
        if !any_clearance_set {
            return Ok(messages);
        }

        self.status(&tr!(CTX, "Check copper clearances..."));

        // Subtract a tolerance to avoid false-positives due to inaccuracies.
        let tolerance = *max_arc_tolerance() + Length::new(1);
        let reduced = |clearance: UnsignedLength| (*clearance - tolerance).max(Length::ZERO);

        // Determine the area of each copper object.
        let mut items: Vec<CopperItem<'_>> = Vec::new();
        let mut generator = GeometryGenerator::new(max_arc_tolerance());
        // Helper for pads: the copper and clearance areas per layer.
        let pad_areas = |pad: &DrcPad,
                         generator: &mut GeometryGenerator|
         -> Result<Vec<(Layer, Length, Region, Region)>> {
            let pad_clearance = pad
                .copper_clearance
                .max(data.min_copper_copper_clearance(pad.net_class));
            let mut areas = Vec::new();
            for &layer in &data.copper_layers {
                if has_geometry(pad, layer) {
                    generator.add_pad(pad, layer, Length::ZERO)?;
                    let copper_area = generator.take_region()?;
                    generator.add_pad(pad, layer, reduced(pad_clearance))?;
                    let clearance_area = generator.take_region()?;
                    areas.push((layer, *pad_clearance, copper_area, clearance_area));
                }
            }
            Ok(areas)
        };

        // Net segments.
        for ns in data.segments.values() {
            let clearance = data.min_copper_copper_clearance(ns.net_class);

            // Pads.
            for pad in ns.pads.values() {
                for (layer, clearance, copper_area, clearance_area) in
                    pad_areas(pad, &mut generator)?
                {
                    items.push(CopperItem::new(
                        CopperObject::Pad(pad, ns),
                        (layer, layer),
                        pad.net,
                        clearance,
                        copper_area,
                        clearance_area,
                    ));
                }
            }

            // Vias.
            for via in ns.vias.values() {
                generator.add_via(via, Length::ZERO)?;
                let copper = generator.take_region()?;
                generator.add_via(via, reduced(clearance))?;
                let clearance_area = generator.take_region()?;
                items.push(CopperItem {
                    object: CopperObject::Via(via, ns),
                    serialized: CopperObject::Via(via, ns).serialize(),
                    start_layer: via.start_layer,
                    end_layer: via.end_layer,
                    net: ns.net,
                    clearance: *clearance,
                    copper_area: copper,
                    clearance_area,
                });
            }

            // Net lines.
            for trace in &ns.traces {
                if data.copper_layers.contains(&trace.layer) {
                    generator.add_trace(trace, Length::ZERO)?;
                    let copper = generator.take_region()?;
                    generator.add_trace(trace, reduced(clearance))?;
                    let clearance_area = generator.take_region()?;
                    items.push(CopperItem {
                        object: CopperObject::Trace(trace, ns),
                        serialized: CopperObject::Trace(trace, ns).serialize(),
                        start_layer: trace.layer,
                        end_layer: trace.layer,
                        net: ns.net,
                        clearance: *clearance,
                        copper_area: copper,
                        clearance_area,
                    });
                }
            }
        }

        // Planes.
        if !data.quick {
            for plane in &data.planes {
                if data.copper_layers.contains(&plane.layer) {
                    let clearance = data.min_copper_copper_clearance(plane.net_class);
                    generator.add_plane(&plane.fragments)?;
                    let copper = generator.take_region()?;
                    let mut expanded = to_clipper(&plane.fragments);
                    clipper_helpers::offset(
                        &mut expanded,
                        reduced(clearance),
                        max_arc_tolerance(),
                        JoinType::Round,
                    )?;
                    let clearance_area = Region::from_integer(&expanded, NonZero)?;
                    items.push(CopperItem {
                        object: CopperObject::Plane(plane),
                        serialized: CopperObject::Plane(plane).serialize(),
                        start_layer: plane.layer,
                        end_layer: plane.layer,
                        net: plane.net,
                        clearance: *clearance,
                        copper_area: copper,
                        clearance_area,
                    });
                }
            }
        }

        let board_clearance = data.settings.min_copper_copper_clearance();

        // Board polygons.
        for polygon in &data.polygons {
            if data.copper_layers.contains(&polygon.layer) {
                generator.add_polygon(
                    &polygon.path,
                    polygon.line_width,
                    polygon.filled,
                    Length::ZERO,
                )?;
                let copper = generator.take_region()?;
                generator.add_polygon(
                    &polygon.path,
                    polygon.line_width,
                    polygon.filled,
                    reduced(board_clearance),
                )?;
                let clearance_area = generator.take_region()?;
                items.push(CopperItem {
                    object: CopperObject::Polygon(polygon, None),
                    serialized: CopperObject::Polygon(polygon, None).serialize(),
                    start_layer: polygon.layer,
                    end_layer: polygon.layer,
                    net: None,
                    clearance: *board_clearance,
                    copper_area: copper,
                    clearance_area,
                });
            }
        }

        // Board stroke texts.
        for st in &data.stroke_texts {
            if data.copper_layers.contains(&st.layer) {
                generator.add_stroke_text(st, Length::ZERO)?;
                let copper = generator.take_region()?;
                generator.add_stroke_text(st, reduced(board_clearance))?;
                let clearance_area = generator.take_region()?;
                items.push(CopperItem {
                    object: CopperObject::StrokeText(st, None),
                    serialized: CopperObject::StrokeText(st, None).serialize(),
                    start_layer: st.layer,
                    end_layer: st.layer,
                    net: None,
                    clearance: *board_clearance,
                    copper_area: copper,
                    clearance_area,
                });
            }
        }

        // Devices.
        for dev in data.devices.values() {
            let transform = dev.transform();

            // Pads.
            for pad in dev.pads.values() {
                for (layer, clearance, copper_area, clearance_area) in
                    pad_areas(pad, &mut generator)?
                {
                    items.push(CopperItem::new(
                        CopperObject::FootprintPad(pad, dev),
                        (layer, layer),
                        pad.net,
                        clearance,
                        copper_area,
                        clearance_area,
                    ));
                }
            }

            // Polygons.
            for polygon in &dev.polygons {
                let layer = transform.map(&polygon.layer);
                if data.copper_layers.contains(&layer) {
                    generator.add_polygon(
                        &transform.map(&polygon.path),
                        polygon.line_width,
                        polygon.filled,
                        Length::ZERO,
                    )?;
                    let copper = generator.take_region()?;
                    generator.add_polygon(
                        &transform.map(&polygon.path),
                        polygon.line_width,
                        polygon.filled,
                        reduced(board_clearance),
                    )?;
                    let clearance_area = generator.take_region()?;
                    items.push(CopperItem {
                        object: CopperObject::Polygon(polygon, Some(dev)),
                        serialized: CopperObject::Polygon(polygon, Some(dev)).serialize(),
                        start_layer: layer,
                        end_layer: layer,
                        net: None,
                        clearance: *board_clearance,
                        copper_area: copper,
                        clearance_area,
                    });
                }
            }

            // Circles.
            for circle in &dev.circles {
                let layer = transform.map(&circle.layer);
                if data.copper_layers.contains(&layer) {
                    generator.add_circle(circle, &transform, Length::ZERO)?;
                    let copper = generator.take_region()?;
                    generator.add_circle(circle, &transform, reduced(board_clearance))?;
                    let clearance_area = generator.take_region()?;
                    items.push(CopperItem {
                        object: CopperObject::Circle(circle, Some(dev)),
                        serialized: CopperObject::Circle(circle, Some(dev)).serialize(),
                        start_layer: layer,
                        end_layer: layer,
                        net: None,
                        clearance: *board_clearance,
                        copper_area: copper,
                        clearance_area,
                    });
                }
            }

            // Stroke texts.
            for st in &dev.stroke_texts {
                // Layer does not need to be transformed!
                if data.copper_layers.contains(&st.layer) {
                    generator.add_stroke_text(st, Length::ZERO)?;
                    let copper = generator.take_region()?;
                    generator.add_stroke_text(st, reduced(board_clearance))?;
                    let clearance_area = generator.take_region()?;
                    items.push(CopperItem {
                        object: CopperObject::StrokeText(st, Some(dev)),
                        serialized: CopperObject::StrokeText(st, Some(dev)).serialize(),
                        start_layer: st.layer,
                        end_layer: st.layer,
                        net: None,
                        clearance: *board_clearance,
                        copper_area: copper,
                        clearance_area,
                    });
                }
            }
        }

        let violations = check_copper_pairs(&items, &data.copper_layers)?;

        // Emit messages.
        for v in violations {
            messages.push(DrcMessage::copper_copper_clearance_violation(
                &items[v.obj1].object,
                &items[v.obj2].object,
                v.layers,
                v.clearance,
                v.locations,
            ));
        }
        Ok(messages)
    }

    fn check_copper_board_clearances(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        let clearance = data.settings.min_copper_board_clearance();
        if *clearance == Length::ZERO {
            return Ok(messages);
        }

        self.status(&tr!(CTX, "Check board clearances..."));

        // Determine restricted area around board outline.
        let restricted_area =
            Region::from_integer(&board_clearance_area(data, clearance)?, EvenOdd)?;

        // Helper for the actual check.
        let intersects = |region: &Region| -> Result<Option<Vec<Path>>> {
            let locations = exact_intersection(&restricted_area, region)?;
            Ok((!locations.is_empty()).then_some(locations))
        };
        let single =
            |add: &dyn Fn(&mut GeometryGenerator) -> Result<()>| -> Result<Option<Vec<Path>>> {
                let mut generator = GeometryGenerator::new(max_arc_tolerance());
                add(&mut generator)?;
                intersects(&generator.take_region()?)
            };

        // Helper for pads: mention every pad only once.
        let check_pad = |pad: &DrcPad| -> Result<Option<Vec<Path>>> {
            for &layer in &data.copper_layers {
                if has_geometry(pad, layer)
                    && let Some(locations) = single(&|g| g.add_pad(pad, layer, Length::ZERO))?
                {
                    return Ok(Some(locations));
                }
            }
            Ok(None)
        };

        // Check net segments.
        for ns in data.segments.values() {
            for pad in ns.pads.values() {
                if let Some(locations) = check_pad(pad)? {
                    messages.push(DrcMessage::copper_board_clearance_pad(
                        ("netsegment", &ns.uuid),
                        pad,
                        clearance,
                        locations,
                    ));
                }
            }
            for via in ns.vias.values() {
                if let Some(locations) = single(&|g| g.add_via(via, Length::ZERO))? {
                    messages.push(DrcMessage::copper_board_clearance_via(
                        ns, via, clearance, locations,
                    ));
                }
            }
            for trace in &ns.traces {
                if let Some(locations) = single(&|g| g.add_trace(trace, Length::ZERO))? {
                    messages.push(DrcMessage::copper_board_clearance_trace(
                        ns, trace, clearance, locations,
                    ));
                }
            }
        }

        // Check planes.
        if !data.quick {
            for plane in &data.planes {
                if let Some(locations) = single(&|g| g.add_plane(&plane.fragments))? {
                    messages.push(DrcMessage::copper_board_clearance_plane(
                        plane, clearance, locations,
                    ));
                }
            }
        }

        // Check board polygons.
        for polygon in &data.polygons {
            if data.copper_layers.contains(&polygon.layer)
                && let Some(locations) = single(&|g| {
                    g.add_polygon(
                        &polygon.path,
                        polygon.line_width,
                        polygon.filled,
                        Length::ZERO,
                    )
                })?
            {
                messages.push(DrcMessage::copper_board_clearance_polygon(
                    polygon, None, clearance, locations,
                ));
            }
        }

        // Check board stroke texts.
        for st in &data.stroke_texts {
            if data.copper_layers.contains(&st.layer)
                && let Some(locations) = single(&|g| g.add_stroke_text(st, Length::ZERO))?
            {
                messages.push(DrcMessage::copper_board_clearance_stroke_text(
                    st, None, clearance, locations,
                ));
            }
        }

        // Check devices.
        for dev in data.devices.values() {
            let transform = dev.transform();
            for pad in dev.pads.values() {
                if let Some(locations) = check_pad(pad)? {
                    messages.push(DrcMessage::copper_board_clearance_pad(
                        ("device", &dev.uuid),
                        pad,
                        clearance,
                        locations,
                    ));
                }
            }
            for polygon in &dev.polygons {
                if data.copper_layers.contains(&transform.map(&polygon.layer))
                    && let Some(locations) = single(&|g| {
                        g.add_polygon(
                            &transform.map(&polygon.path),
                            polygon.line_width,
                            polygon.filled,
                            Length::ZERO,
                        )
                    })?
                {
                    messages.push(DrcMessage::copper_board_clearance_polygon(
                        polygon,
                        Some(dev),
                        clearance,
                        locations,
                    ));
                }
            }
            for circle in &dev.circles {
                if data.copper_layers.contains(&transform.map(&circle.layer))
                    && let Some(locations) =
                        single(&|g| g.add_circle(circle, &transform, Length::ZERO))?
                {
                    messages.push(DrcMessage::copper_board_clearance_circle(
                        dev, circle, clearance, locations,
                    ));
                }
            }
            for st in &dev.stroke_texts {
                // Layer does not need to be transformed!
                if data.copper_layers.contains(&st.layer)
                    && let Some(locations) = single(&|g| g.add_stroke_text(st, Length::ZERO))?
                {
                    messages.push(DrcMessage::copper_board_clearance_stroke_text(
                        st,
                        Some(dev),
                        clearance,
                        locations,
                    ));
                }
            }
        }
        Ok(messages)
    }

    fn check_copper_hole_clearances(
        &self,
        data: &BoardDrcData,
        calc: &CalculatedJobData,
    ) -> Messages {
        let mut messages = Vec::new();
        let clearance = data.settings.min_copper_npth_clearance();
        if *clearance == Length::ZERO
            || (data.holes.is_empty() && data.devices.values().all(|dev| dev.holes.is_empty()))
        {
            return Ok(messages);
        }

        self.status(&tr!(CTX, "Check hole clearances..."));

        // Helper for the actual check.
        let intersects = |hole: &DrcHole, transform: &Transform| -> Result<Vec<Path>> {
            let mut generator = GeometryGenerator::new(max_arc_tolerance());
            generator.add_hole(
                hole.diameter,
                &hole.path,
                transform,
                *clearance - *max_arc_tolerance() - Length::new(1),
            )?;
            exact_locations(
                &generator
                    .take_region()?
                    .intersection_with_union(calc.copper_paths_per_layer.values())?,
            )
        };

        // Check board holes.
        for hole in &data.holes {
            let locations = intersects(hole, &Transform::default())?;
            if !locations.is_empty() {
                messages.push(DrcMessage::copper_hole_clearance_violation(
                    hole, None, clearance, locations,
                ));
            }
        }

        // Check footprint holes.
        for dev in data.devices.values() {
            let transform = dev.transform();
            for hole in &dev.holes {
                let locations = intersects(hole, &transform)?;
                if !locations.is_empty() {
                    messages.push(DrcMessage::copper_hole_clearance_violation(
                        hole,
                        Some(dev),
                        clearance,
                        locations,
                    ));
                }
            }
        }
        Ok(messages)
    }

    fn check_drill_drill_clearances(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        let clearance = data.settings.min_drill_drill_clearance();
        if *clearance == Length::ZERO {
            return Ok(messages);
        }

        self.status(&tr!(CTX, "Check drill clearances..."));

        // Determine diameter expansion.
        let expansion =
            unsigned((*clearance - *max_arc_tolerance() - Length::new(1)).max(Length::ZERO))?;

        // Determine the area of each drill.
        let mut items: Vec<(DrcHoleRef<'_>, ClipperPaths)> = Vec::new();
        let area = |path: &Path, diameter: PositiveLength| {
            to_clipper(&path.to_outline_strokes(diameter + expansion))
        };

        // Pads & Vias.
        for ns in data.segments.values() {
            for pad in ns.pads.values() {
                let t = pad_transform(pad);
                for hole in &pad.holes {
                    items.push((
                        DrcHoleRef::PadHole(ns, pad, hole),
                        area(&t.map(hole.path.get()), hole.diameter),
                    ));
                }
            }
            for via in ns.vias.values() {
                items.push((
                    DrcHoleRef::Via(ns, via),
                    area(
                        NonEmptyPath::from_point(via.position).get(),
                        via.drill_diameter,
                    ),
                ));
            }
        }

        // Board holes.
        for hole in &data.holes {
            items.push((
                DrcHoleRef::BoardHole(hole),
                area(hole.path.get(), hole.diameter),
            ));
        }

        // Devices.
        for dev in data.devices.values() {
            let transform = dev.transform();
            for pad in dev.pads.values() {
                let t = pad_transform(pad);
                for hole in &pad.holes {
                    items.push((
                        DrcHoleRef::FootprintPadHole(dev, pad, hole),
                        area(&t.map(hole.path.get()), hole.diameter),
                    ));
                }
            }
            for hole in &dev.holes {
                items.push((
                    DrcHoleRef::DeviceHole(dev, hole),
                    area(&transform.map(hole.path.get()), hole.diameter),
                ));
            }
        }

        // Now check for intersections.
        for i in 0..items.len() {
            for k in (i + 1)..items.len() {
                let locations = intersection(&items[i].1, &items[k].1)?;
                if !locations.is_empty() {
                    messages.push(DrcMessage::drill_drill_clearance_violation(
                        &items[i].0,
                        &items[k].0,
                        clearance,
                        locations,
                    ));
                }
            }
        }
        Ok(messages)
    }

    fn check_drill_board_clearances(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        let clearance = data.settings.min_drill_board_clearance();
        if *clearance == Length::ZERO {
            return Ok(messages);
        }

        self.status(&tr!(CTX, "Check drill to board edge clearances..."));

        // Determine restricted area around board outline.
        let restricted_area =
            Region::from_integer(&board_clearance_area(data, clearance)?, EvenOdd)?;

        // Helper for the actual check.
        let intersects = |path: &Path, diameter: PositiveLength| -> Result<Vec<Path>> {
            let area = to_clipper(&path.to_outline_strokes(diameter));
            exact_intersection(&restricted_area, &Region::from_integer(&area, EvenOdd)?)
        };
        let mut check =
            |hole: DrcHoleRef<'_>, path: &Path, diameter: PositiveLength| -> Result<()> {
                let locations = intersects(path, diameter)?;
                if !locations.is_empty() {
                    messages.push(DrcMessage::drill_board_clearance_violation(
                        &hole, clearance, locations,
                    ));
                }
                Ok(())
            };

        // Check pads & vias.
        for ns in data.segments.values() {
            for pad in ns.pads.values() {
                let t = pad_transform(pad);
                for hole in &pad.holes {
                    check(
                        DrcHoleRef::PadHole(ns, pad, hole),
                        &t.map(hole.path.get()),
                        hole.diameter,
                    )?;
                }
            }
            for via in ns.vias.values() {
                check(
                    DrcHoleRef::Via(ns, via),
                    NonEmptyPath::from_point(via.position).get(),
                    via.drill_diameter,
                )?;
            }
        }

        // Check board holes.
        for hole in &data.holes {
            check(DrcHoleRef::BoardHole(hole), hole.path.get(), hole.diameter)?;
        }

        // Check devices.
        for dev in data.devices.values() {
            let transform = dev.transform();
            for pad in dev.pads.values() {
                let t = pad_transform(pad);
                for hole in &pad.holes {
                    check(
                        DrcHoleRef::FootprintPadHole(dev, pad, hole),
                        &t.map(hole.path.get()),
                        hole.diameter,
                    )?;
                }
            }
            for hole in &dev.holes {
                check(
                    DrcHoleRef::DeviceHole(dev, hole),
                    &transform.map(hole.path.get()),
                    hole.diameter,
                )?;
            }
        }
        Ok(messages)
    }

    fn check_silkscreen_stopmask_clearances(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        let clearance = data.settings.min_silkscreen_stopmask_clearance();
        let layers_top = &data.silkscreen_layers_top;
        let layers_bot = &data.silkscreen_layers_bot;
        if (*clearance == Length::ZERO) || (layers_top.is_empty() && layers_bot.is_empty()) {
            return Ok(messages);
        }

        self.status(&tr!(CTX, "Check silkscreen to stopmask clearances..."));

        // Determine areas of stop mask openings.
        let mut board_area = to_clipper(&board_outlines(data, &[Layer::BOARD_OUTLINES]));
        clipper_helpers::subtract(
            &mut board_area,
            &to_clipper(&board_outlines(data, &[Layer::BOARD_CUTOUTS])),
            NonZero,
            NonZero,
        )?;
        let board_clearance = board_clearance_area(data, clearance)?;

        // Run the checks on each board side.
        for (layers, stop_mask) in [
            (layers_top, Layer::TOP_STOP_MASK),
            (layers_bot, Layer::BOT_STOP_MASK),
        ] {
            if layers.is_empty() {
                continue;
            }

            // Build stopmask openings area. Only take the board area into
            // account since warnings outside the board area are not really
            // helpful.
            let mut generator = GeometryGenerator::new(max_arc_tolerance());
            generator.add_stop_mask_openings(data, stop_mask, *clearance)?;
            let clearance_area = rational_union::union(&[
                generator.take_region()?.operand(),
                Operand::from_integer(&board_clearance, NonZero),
            ])?
            .0
            .intersection(&Region::from_integer(&board_area, EvenOdd)?)?;

            // Note: We check only stroke texts. For other objects like
            // polygons, usually there are dozens of clearance violations but
            // most of the time they are not relevant and cannot be avoided.
            // So let's omit these annoying warnings.
            let intersects = |st: &super::data::DrcStrokeText| -> Result<Vec<Path>> {
                let mut generator = GeometryGenerator::new(max_arc_tolerance());
                generator.add_stroke_text(st, Length::ZERO)?;
                exact_intersection(&clearance_area, &generator.take_region()?)
            };

            // Check board stroke texts.
            for st in &data.stroke_texts {
                if layers.contains(&st.layer) {
                    let locations = intersects(st)?;
                    if !locations.is_empty() {
                        messages.push(DrcMessage::silkscreen_clearance_violation(
                            st, None, clearance, locations,
                        ));
                    }
                }
            }

            // Check device stroke texts.
            for dev in data.devices.values() {
                for st in &dev.stroke_texts {
                    // Layer does not need to be transformed!
                    if layers.contains(&st.layer) {
                        let locations = intersects(st)?;
                        if !locations.is_empty() {
                            messages.push(DrcMessage::silkscreen_clearance_violation(
                                st,
                                Some(dev),
                                clearance,
                                locations,
                            ));
                        }
                    }
                }
            }
        }
        Ok(messages)
    }

    fn check_minimum_copper_width(&self, data: &BoardDrcData) -> Messages {
        self.status(&tr!(CTX, "Check copper widths..."));
        check_minimum_width(data, data.settings.min_copper_width(), |layer| {
            data.copper_layers.contains(&layer)
        })
    }

    fn check_minimum_pth_annular_ring(
        &self,
        data: &BoardDrcData,
        calc: &CalculatedJobData,
    ) -> Messages {
        let mut messages = Vec::new();
        let annular_width = data.settings.min_pth_annular_ring();
        if *annular_width == Length::ZERO {
            return Ok(messages);
        }

        self.status(&tr!(CTX, "Check PTH annular rings..."));

        let empty = Region::default();

        // Helper.
        let check_pad =
            |pad: &DrcPad| -> Result<Vec<Path>> {
                if pad.holes.is_empty() {
                    return Ok(Vec::new());
                }
                // Determine hole areas including minimum annular ring.
                let transform = pad_transform(pad);
                let mut areas = Vec::new();
                for hole in &pad.holes {
                    let diameter = hole.diameter + (*annular_width * 2) - Length::new(1);
                    if diameter <= Length::ZERO {
                        continue;
                    }
                    let strokes = transform.map(&hole.path.to_outline_strokes(positive(diameter)?));
                    areas.push(Operand::from_integer(&to_clipper(&strokes), NonZero));
                }

                // Decide on the exact difference, before integer display conversion.
                exact_locations(
                    &rational_union::union(&areas)?
                        .0
                        .difference_from_intersection(data.copper_layers.iter().map(|layer| {
                            calc.copper_paths_per_layer.get(layer).unwrap_or(&empty)
                        }))?,
                )
            };

        // Check pad & via annular rings.
        for ns in data.segments.values() {
            for pad in ns.pads.values() {
                let locations = check_pad(pad)?;
                if !locations.is_empty() {
                    messages.push(DrcMessage::minimum_annular_ring_pad(
                        ("netsegment", &ns.uuid, ""),
                        pad,
                        annular_width,
                        locations,
                    ));
                }
            }
            for via in ns.vias.values() {
                let annular = (*via.size - *via.drill_diameter) / 2;
                if annular < *annular_width {
                    messages.push(DrcMessage::minimum_annular_ring_via(
                        ns,
                        via,
                        annular_width,
                        via_location(via),
                    ));
                }
            }
        }

        // Check footprint pad annular rings.
        for dev in data.devices.values() {
            for pad in dev.pads.values() {
                let locations = check_pad(pad)?;
                if !locations.is_empty() {
                    messages.push(DrcMessage::minimum_annular_ring_pad(
                        ("device", &dev.uuid, &dev.cmp_instance_name),
                        pad,
                        annular_width,
                        locations,
                    ));
                }
            }
        }
        Ok(messages)
    }

    fn check_minimum_npth_drill_diameter(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        let min_diameter = data.settings.min_npth_drill_diameter();
        if *min_diameter == Length::ZERO {
            return Ok(messages);
        }

        self.status(&tr!(CTX, "Check NPTH drill diameters..."));

        // Board holes.
        for hole in &data.holes {
            if (hole.path.vertices().len() < 2) && (hole.diameter < min_diameter) {
                messages.push(DrcMessage::minimum_drill_diameter_violation(
                    &DrcHoleRef::BoardHole(hole),
                    min_diameter,
                    hole_location(hole, &Transform::default()),
                ));
            }
        }

        // Package holes.
        for dev in data.devices.values() {
            let transform = dev.transform();
            for hole in &dev.holes {
                if (hole.path.vertices().len() < 2) && (hole.diameter < min_diameter) {
                    messages.push(DrcMessage::minimum_drill_diameter_violation(
                        &DrcHoleRef::DeviceHole(dev, hole),
                        min_diameter,
                        hole_location(hole, &transform),
                    ));
                }
            }
        }
        Ok(messages)
    }

    fn check_minimum_npth_slot_width(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        let min_width = data.settings.min_npth_slot_width();
        if *min_width == Length::ZERO {
            return Ok(messages);
        }

        self.status(&tr!(CTX, "Check NPTH slot widths..."));

        // Board holes.
        for hole in &data.holes {
            if (hole.path.vertices().len() > 1) && (hole.diameter < min_width) {
                messages.push(DrcMessage::minimum_slot_width_violation(
                    &DrcHoleRef::BoardHole(hole),
                    min_width,
                    hole_location(hole, &Transform::default()),
                ));
            }
        }

        // Package holes.
        for dev in data.devices.values() {
            let transform = dev.transform();
            for hole in &dev.holes {
                if (hole.path.vertices().len() > 1) && (hole.diameter < min_width) {
                    messages.push(DrcMessage::minimum_slot_width_violation(
                        &DrcHoleRef::DeviceHole(dev, hole),
                        min_width,
                        hole_location(hole, &transform),
                    ));
                }
            }
        }
        Ok(messages)
    }

    fn check_minimum_pth_drill_diameter(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        self.status(&tr!(CTX, "Check PTH drill diameters..."));
        let min_diameter = data.settings.min_pth_drill_diameter();

        let pad_location = |pad: &DrcPad, hole: &DrcHole| -> Result<Vec<Path>> {
            let diameter = positive((*hole.diameter).max(Length::new(50000)))?;
            Ok(vec![Path::circle(diameter).translated(pad.position)])
        };

        // Pads & vias.
        for ns in data.segments.values() {
            for pad in ns.pads.values() {
                for hole in &pad.holes {
                    if (hole.path.vertices().len() < 2) && (hole.diameter < min_diameter) {
                        messages.push(DrcMessage::minimum_drill_diameter_violation(
                            &DrcHoleRef::PadHole(ns, pad, hole),
                            min_diameter,
                            pad_location(pad, hole)?,
                        ));
                    }
                }
            }
            let min_via_drill = data.min_via_drill_diameter(ns.net_class);
            for via in ns.vias.values() {
                if via.drill_diameter < min_via_drill {
                    let locations = vec![Path::circle(via.drill_diameter).translated(via.position)];
                    messages.push(DrcMessage::minimum_drill_diameter_violation(
                        &DrcHoleRef::Via(ns, via),
                        min_via_drill,
                        locations,
                    ));
                }
            }
        }

        // Footprint pads.
        for dev in data.devices.values() {
            for pad in dev.pads.values() {
                for hole in &pad.holes {
                    if (hole.path.vertices().len() < 2) && (hole.diameter < min_diameter) {
                        messages.push(DrcMessage::minimum_drill_diameter_violation(
                            &DrcHoleRef::FootprintPadHole(dev, pad, hole),
                            min_diameter,
                            pad_location(pad, hole)?,
                        ));
                    }
                }
            }
        }
        Ok(messages)
    }

    fn check_minimum_pth_slot_width(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        let min_width = data.settings.min_pth_slot_width();
        if *min_width == Length::ZERO {
            return Ok(messages);
        }

        self.status(&tr!(CTX, "Check PTH slot widths..."));

        // Pads.
        for ns in data.segments.values() {
            for pad in ns.pads.values() {
                let transform = pad_transform(pad);
                for hole in &pad.holes {
                    if (hole.path.vertices().len() > 1) && (hole.diameter < min_width) {
                        messages.push(DrcMessage::minimum_slot_width_violation(
                            &DrcHoleRef::PadHole(ns, pad, hole),
                            min_width,
                            hole_location(hole, &transform),
                        ));
                    }
                }
            }
        }

        // Footprint pads.
        for dev in data.devices.values() {
            for pad in dev.pads.values() {
                let transform = pad_transform(pad);
                for hole in &pad.holes {
                    if (hole.path.vertices().len() > 1) && (hole.diameter < min_width) {
                        messages.push(DrcMessage::minimum_slot_width_violation(
                            &DrcHoleRef::FootprintPadHole(dev, pad, hole),
                            min_width,
                            hole_location(hole, &transform),
                        ));
                    }
                }
            }
        }
        Ok(messages)
    }

    fn check_minimum_silkscreen_width(&self, data: &BoardDrcData) -> Messages {
        let min_width = data.settings.min_silkscreen_width();
        let layers: Vec<Layer> = data
            .silkscreen_layers_top
            .iter()
            .chain(&data.silkscreen_layers_bot)
            .copied()
            .collect();
        if (*min_width == Length::ZERO) || layers.is_empty() {
            return Ok(Vec::new());
        }

        self.status(&tr!(CTX, "Check silkscreen widths..."));
        check_minimum_width(data, min_width, |layer| layers.contains(&layer))
    }

    fn check_minimum_silkscreen_text_height(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        let min_height = data.settings.min_silkscreen_text_height();
        let layers: Vec<Layer> = data
            .silkscreen_layers_top
            .iter()
            .chain(&data.silkscreen_layers_bot)
            .copied()
            .collect();
        if (*min_height == Length::ZERO) || layers.is_empty() {
            return Ok(messages);
        }

        self.status(&tr!(CTX, "Check silkscreen text heights..."));
        for st in &data.stroke_texts {
            if !layers.contains(&st.layer) {
                continue;
            }
            if st.height < min_height {
                messages.push(DrcMessage::minimum_text_height_violation(
                    st,
                    None,
                    min_height,
                    stroke_text_locations(st)?,
                ));
            }
        }
        Ok(messages)
    }

    fn check_zones(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        self.status(&tr!(CTX, "Check keepout zones..."));

        // Collect all zones.
        let mut zones: Vec<ZoneItem<'_>> = Vec::new();
        for zone in &data.zones {
            // Check validity.
            if zone.board_layers.is_disjoint(&data.copper_layers) || zone.rules.is_empty() {
                messages.push(DrcMessage::useless_zone(zone, vec![closed(&zone.outline)]));
            }

            // Add to collection.
            zones.push(ZoneItem {
                zone,
                device: None,
                outline: zone.outline.clone(),
                layers: zone.board_layers.clone(),
                rules: zone.rules,
            });
        }
        for dev in data.devices.values() {
            let transform = dev.transform();
            for zone in &dev.zones {
                let mut layers = BTreeSet::new();
                if zone.footprint_layers.contains(ZoneLayers::TOP) {
                    layers.insert(transform.map(&Layer::TOP_COPPER));
                }
                if zone.footprint_layers.contains(ZoneLayers::INNER) {
                    layers.extend(data.copper_layers.iter().filter(|l| l.is_inner()));
                }
                if zone.footprint_layers.contains(ZoneLayers::BOTTOM) {
                    layers.insert(transform.map(&Layer::BOT_COPPER));
                }
                zones.push(ZoneItem {
                    zone,
                    device: Some(dev),
                    outline: transform.map(&zone.outline),
                    layers,
                    rules: zone.rules,
                });
            }
        }

        // Check for violations.
        for zone in &zones {
            self.check_zone(data, zone, &mut messages)?;
        }
        Ok(messages)
    }

    fn check_zone(
        &self,
        data: &BoardDrcData,
        zone: &ZoneItem<'_>,
        messages: &mut Vec<DrcMessage>,
    ) -> Result<()> {
        // Determine some zone data.
        let zone_area = PainterPathPx::from_paths([&zone.outline]);
        let mut no_copper_layers = BTreeSet::new();
        if zone.rules.contains(ZoneRules::NO_COPPER) {
            no_copper_layers = zone.layers.clone();
        }
        let mut no_stop_mask_layers = BTreeSet::new();
        if zone.rules.contains(ZoneRules::NO_EXPOSURE) {
            if zone.layers.contains(&Layer::TOP_COPPER) {
                no_stop_mask_layers.insert(Layer::TOP_STOP_MASK);
            }
            if zone.layers.contains(&Layer::BOT_COPPER) {
                no_stop_mask_layers.insert(Layer::BOT_STOP_MASK);
            }
        }
        let mut no_device_layers = BTreeSet::new();
        if zone.rules.contains(ZoneRules::NO_DEVICES) {
            // Note: Also adding documentation layers since many packages
            // probably don't have an explicit package outline.
            if zone.layers.contains(&Layer::TOP_COPPER) {
                no_device_layers.insert(Layer::TOP_PACKAGE_OUTLINES);
                no_device_layers.insert(Layer::TOP_DOCUMENTATION);
            }
            if zone.layers.contains(&Layer::BOT_COPPER) {
                no_device_layers.insert(Layer::BOT_PACKAGE_OUTLINES);
                no_device_layers.insert(Layer::BOT_DOCUMENTATION);
            }
        }

        // Helper functions.
        let intersects_area = |locations: &[Path]| {
            locations
                .iter()
                .any(|p| zone_area.intersects(&PainterPathPx::from_paths([p])))
        };
        let intersects_pad =
            |pad: &DrcPad, layers: &BTreeSet<Layer>| -> Result<Option<Vec<Path>>> {
                let transform = pad_transform(pad);
                let mut outlines: Vec<Path> = Vec::new();
                for layer in layers {
                    for geometry in pad.geometries.get(layer).into_iter().flatten() {
                        for outline in transform.map(&geometry.to_outlines()?) {
                            if !outlines.contains(&outline) {
                                outlines.push(outline);
                            }
                        }
                    }
                }
                Ok((!outlines.is_empty() && intersects_area(&outlines)).then_some(outlines))
            };
        let intersects_polygon =
            |path: &Path, line_width: UnsignedLength, fill: bool| -> Result<Option<Vec<Path>>> {
                let mut locations = Vec::new();
                if *line_width > Length::ZERO {
                    locations.extend(path.to_outline_strokes(positive(*line_width)?));
                }
                if fill && path.is_closed() {
                    locations.push(path.clone());
                }
                Ok((!locations.is_empty() && intersects_area(&locations)).then_some(locations))
            };
        let circle_area = |position: Point, diameter: PositiveLength| {
            PainterPathPx::from_paths([&Path::circle(diameter).translated(position)])
        };
        let z = zone.zone;
        let zd = zone.device;

        // Check devices.
        for dev in data.devices.values() {
            // Skip violations within a single device since this is actually
            // a (minor) library issue and cannot be fixed in the board. It's
            // even handy to use this behavior to simplify zone outlines in
            // footprints.
            if zd.is_some_and(|d| d.uuid == dev.uuid) {
                continue;
            }

            // Check pads.
            for pad in dev.pads.values() {
                let object = ZoneObject::FootprintPad(dev, pad);
                if let Some(locations) = intersects_pad(pad, &no_copper_layers)? {
                    messages.push(DrcMessage::copper_in_keepout_zone(
                        z, zd, &object, locations,
                    ));
                }
                if let Some(locations) = intersects_pad(pad, &no_stop_mask_layers)? {
                    messages.push(DrcMessage::exposure_in_keepout_zone(
                        z, zd, &object, locations,
                    ));
                }
            }

            // Check polygons.
            let transform = dev.transform();
            let mut device_in_keepout_zone = false;
            for polygon in &dev.polygons {
                let check = || {
                    let is_area = polygon.layer.polygons_represent_areas();
                    let path = if is_area {
                        closed(&polygon.path)
                    } else {
                        polygon.path.clone()
                    };
                    intersects_polygon(
                        &transform.map(&path),
                        polygon.line_width,
                        polygon.filled || is_area,
                    )
                };
                let layer = transform.map(&polygon.layer);
                let object = ZoneObject::DevicePolygon(dev, polygon);
                if no_copper_layers.contains(&layer)
                    && let Some(locations) = check()?
                {
                    messages.push(DrcMessage::copper_in_keepout_zone(
                        z, zd, &object, locations,
                    ));
                    continue;
                }
                if no_stop_mask_layers.contains(&layer)
                    && let Some(locations) = check()?
                {
                    messages.push(DrcMessage::exposure_in_keepout_zone(
                        z, zd, &object, locations,
                    ));
                    continue;
                }
                if no_device_layers.contains(&layer) && check()?.is_some() {
                    device_in_keepout_zone = true;
                }
            }

            // Check circles.
            for circle in &dev.circles {
                let check = || {
                    intersects_polygon(
                        &transform.map(&Path::circle(circle.diameter).translated(circle.center)),
                        circle.line_width,
                        circle.filled || circle.layer.polygons_represent_areas(),
                    )
                };
                let layer = transform.map(&circle.layer);
                let object = ZoneObject::DeviceCircle(dev, circle);
                if no_copper_layers.contains(&layer)
                    && let Some(locations) = check()?
                {
                    messages.push(DrcMessage::copper_in_keepout_zone(
                        z, zd, &object, locations,
                    ));
                    continue;
                }
                if no_stop_mask_layers.contains(&layer)
                    && let Some(locations) = check()?
                {
                    messages.push(DrcMessage::exposure_in_keepout_zone(
                        z, zd, &object, locations,
                    ));
                    continue;
                }
                if no_device_layers.contains(&layer) && check()?.is_some() {
                    device_in_keepout_zone = true;
                }
            }

            if device_in_keepout_zone {
                messages.push(DrcMessage::device_in_keepout_zone(
                    z,
                    zd,
                    dev,
                    device_location(dev),
                ));
            }
        }

        // Check net segments.
        for ns in data.segments.values() {
            // Check pads.
            for pad in ns.pads.values() {
                let object = ZoneObject::Pad(ns, pad);
                if let Some(locations) = intersects_pad(pad, &no_copper_layers)? {
                    messages.push(DrcMessage::copper_in_keepout_zone(
                        z, zd, &object, locations,
                    ));
                }
                if let Some(locations) = intersects_pad(pad, &no_stop_mask_layers)? {
                    messages.push(DrcMessage::exposure_in_keepout_zone(
                        z, zd, &object, locations,
                    ));
                }
            }

            // Check vias.
            for via in ns.vias.values() {
                let object = ZoneObject::Via(ns, via);
                if Via::is_on_any_layer_between(
                    no_copper_layers.iter().copied(),
                    via.start_layer,
                    via.end_layer,
                ) && zone_area.intersects(&circle_area(via.position, via.size))
                {
                    messages.push(DrcMessage::copper_in_keepout_zone(
                        z,
                        zd,
                        &object,
                        via_location(via),
                    ));
                }
                for (layer, diameter) in [
                    (Layer::TOP_STOP_MASK, via.stop_mask_diameter_top),
                    (Layer::BOT_STOP_MASK, via.stop_mask_diameter_bot),
                ] {
                    if let (true, Some(diameter)) = (no_stop_mask_layers.contains(&layer), diameter)
                        && zone_area.intersects(&circle_area(via.position, diameter))
                    {
                        messages.push(DrcMessage::exposure_in_keepout_zone(
                            z,
                            zd,
                            &object,
                            via_location(via),
                        ));
                        break;
                    }
                }
            }

            // Check traces.
            for trace in &ns.traces {
                if no_copper_layers.contains(&trace.layer) {
                    let area = PainterPathPx::from_paths([&trace_area(trace)]);
                    if zone_area.intersects(&area) {
                        messages.push(DrcMessage::copper_in_keepout_zone(
                            z,
                            zd,
                            &ZoneObject::Trace(ns, trace),
                            trace_location(trace),
                        ));
                    }
                }
            }
        }

        // Check polygons.
        for polygon in &data.polygons {
            let check = || intersects_polygon(&polygon.path, polygon.line_width, polygon.filled);
            let object = ZoneObject::Polygon(polygon);
            if no_copper_layers.contains(&polygon.layer)
                && let Some(locations) = check()?
            {
                messages.push(DrcMessage::copper_in_keepout_zone(
                    z, zd, &object, locations,
                ));
                continue;
            }
            if no_stop_mask_layers.contains(&polygon.layer)
                && let Some(locations) = check()?
            {
                messages.push(DrcMessage::exposure_in_keepout_zone(
                    z, zd, &object, locations,
                ));
            }
        }
        Ok(())
    }

    fn check_vias(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        self.status(&tr!(CTX, "Check for useless or disallowed vias..."));

        for ns in data.segments.values() {
            for via in ns.vias.values() {
                if (via.is_blind && !data.settings.blind_vias_allowed())
                    || (via.is_buried && !data.settings.buried_vias_allowed())
                {
                    messages.push(DrcMessage::forbidden_via(ns, via, via_location(via)));
                }

                // If the via has no drill layer span, emit an InvalidVia
                // warning.
                if via.drill_layer_span.is_none() {
                    messages.push(DrcMessage::invalid_via(ns, via, via_location(via)));
                    continue;
                }

                // If the total number of layers connected to the via is less
                // than two, add a 'useless via' warning. Connections can be
                // made by traces, planes, or pads.
                if is_via_useless(data, ns, via)? {
                    messages.push(DrcMessage::useless_via(ns, via, via_location(via)));
                }
            }
        }
        Ok(messages)
    }

    fn check_planes(&self, data: &BoardDrcData) -> Messages {
        self.status(&tr!(CTX, "Check for invalid planes..."));
        data.planes
            .iter()
            .filter(|plane| plane.min_width > plane.thermal_spoke_width)
            .map(|plane| {
                Ok(DrcMessage::plane_thermal_spoke_width_ignored(
                    plane,
                    plane_location(plane)?,
                ))
            })
            .collect()
    }

    fn check_allowed_npth_slots(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        self.status(&tr!(CTX, "Check for disallowed NPTH slots..."));

        let allowed = data.settings.allowed_npth_slots();
        if allowed == AllowedSlots::Any {
            return Ok(messages);
        }

        // Board holes.
        for hole in &data.holes {
            if requires_hole_slot_warning(hole, allowed) {
                messages.push(DrcMessage::forbidden_slot(
                    hole,
                    None,
                    None,
                    hole_location(hole, &Transform::default()),
                ));
            }
        }

        // Package holes.
        for dev in data.devices.values() {
            let transform = dev.transform();
            for hole in &dev.holes {
                if requires_hole_slot_warning(hole, allowed) {
                    messages.push(DrcMessage::forbidden_slot(
                        hole,
                        Some(("device", &dev.uuid)),
                        None,
                        hole_location(hole, &transform),
                    ));
                }
            }
        }
        Ok(messages)
    }

    fn check_allowed_pth_slots(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        self.status(&tr!(CTX, "Check for disallowed PTH slots..."));

        let allowed = data.settings.allowed_pth_slots();
        if allowed == AllowedSlots::Any {
            return Ok(messages);
        }

        // Pads.
        for ns in data.segments.values() {
            for pad in ns.pads.values() {
                let transform = pad_transform(pad);
                for hole in &pad.holes {
                    if requires_hole_slot_warning(hole, allowed) {
                        messages.push(DrcMessage::forbidden_slot(
                            hole,
                            Some(("netsegment", &ns.uuid)),
                            Some(pad),
                            hole_location(hole, &transform),
                        ));
                    }
                }
            }
        }

        // Footprint pads.
        for dev in data.devices.values() {
            for pad in dev.pads.values() {
                let transform = pad_transform(pad);
                for hole in &pad.holes {
                    if requires_hole_slot_warning(hole, allowed) {
                        messages.push(DrcMessage::forbidden_slot(
                            hole,
                            Some(("device", &dev.uuid)),
                            Some(pad),
                            hole_location(hole, &transform),
                        ));
                    }
                }
            }
        }
        Ok(messages)
    }

    fn check_invalid_pad_connections(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        self.status(&tr!(CTX, "Check pad connections..."));

        let is_invalid = |pad: &DrcPad, layer: &Layer| {
            let valid = pad.geometries.get(layer).into_iter().flatten().any(|g| {
                // Upstream ignores errors (empty painter path).
                let outlines = g.to_outlines().unwrap_or_default();
                PainterPathPx::from_paths(&outlines).contains_point((0.0, 0.0))
            });
            !valid
        };
        let location =
            |pad: &DrcPad| vec![Path::circle(positive_const(500_000)).translated(pad.position)];

        // Pads.
        for ns in data.segments.values() {
            for pad in ns.pads.values() {
                for layer in &pad.layers_with_traces {
                    if is_invalid(pad, layer) {
                        messages.push(DrcMessage::invalid_pad_connection(
                            ("netsegment", &ns.uuid, ""),
                            pad,
                            *layer,
                            location(pad),
                        ));
                    }
                }
            }
        }

        // Footprint pads.
        for dev in data.devices.values() {
            for pad in dev.pads.values() {
                for layer in &pad.layers_with_traces {
                    if is_invalid(pad, layer) {
                        messages.push(DrcMessage::invalid_pad_connection(
                            ("device", &dev.uuid, &dev.cmp_instance_name),
                            pad,
                            *layer,
                            location(pad),
                        ));
                    }
                }
            }
        }
        Ok(messages)
    }

    fn check_device_clearances(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        self.status(&tr!(CTX, "Check device clearances..."));

        for (outlines_layer, courtyard_layer) in [
            (Layer::TOP_PACKAGE_OUTLINES, Layer::TOP_COURTYARD),
            (Layer::BOT_PACKAGE_OUTLINES, Layer::BOT_COURTYARD),
        ] {
            // Determine device outlines and courtyards.
            let devices: Vec<(&DrcDevice, ClipperPaths, ClipperPaths)> = data
                .devices
                .values()
                .map(|dev| {
                    Ok((
                        dev,
                        device_outline_paths(dev, outlines_layer)?,
                        device_outline_paths(dev, courtyard_layer)?,
                    ))
                })
                .collect::<Result<_>>()?;

            // Helper function.
            let overlap = |area1: &ClipperPaths, area2: &ClipperPaths| -> Result<Vec<Path>> {
                if area1.is_empty() || area2.is_empty() {
                    return Ok(Vec::new());
                }
                intersection(area1, area2)
            };

            // Check for overlaps.
            for (i, (dev1, outlines1, courtyard1)) in devices.iter().enumerate() {
                for (dev2, outlines2, courtyard2) in &devices[(i + 1)..] {
                    let locations = overlap(outlines1, outlines2)?;
                    if !locations.is_empty() {
                        messages.push(DrcMessage::overlapping_devices(dev1, dev2, locations));
                        continue;
                    }
                    let mut locations = overlap(outlines1, courtyard2)?;
                    if locations.is_empty() {
                        locations = overlap(outlines2, courtyard1)?;
                    }
                    if !locations.is_empty() {
                        messages.push(DrcMessage::device_in_courtyard(dev1, dev2, locations));
                    }
                }
            }
        }
        Ok(messages)
    }

    fn check_board_outline(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        self.status(&tr!(CTX, "Check board outline..."));

        // Collect all valid outlines and report all open polygons.
        let all_outline_layers = [
            Layer::BOARD_OUTLINES,
            Layer::BOARD_CUTOUTS,
            Layer::BOARD_PLATED_CUTOUTS,
        ];
        let mut objects: Vec<(&DrcPolygon, Option<&DrcDevice>, Path)> = Vec::new();
        let open_location = |path: &Path, line_width: UnsignedLength| -> Result<Vec<Path>> {
            Ok(path.to_outline_strokes(positive((*line_width).max(Length::new(100_000)))?))
        };
        for polygon in &data.polygons {
            if all_outline_layers.contains(&polygon.layer) {
                if polygon.path.is_closed() {
                    objects.push((polygon, None, polygon.path.clone()));
                } else {
                    messages.push(DrcMessage::open_board_outline_polygon(
                        &polygon.uuid,
                        None,
                        open_location(&polygon.path, polygon.line_width)?,
                    ));
                }
            }
        }
        for dev in data.devices.values() {
            let transform = dev.transform();
            for polygon in &dev.polygons {
                let path = transform.map(&polygon.path);
                if all_outline_layers.contains(&polygon.layer) {
                    if polygon.path.is_closed() {
                        objects.push((polygon, Some(dev), path));
                    } else {
                        messages.push(DrcMessage::open_board_outline_polygon(
                            &polygon.uuid,
                            Some(&dev.uuid),
                            open_location(&path, polygon.line_width)?,
                        ));
                    }
                }
            }
        }

        // Check if there's exactly one board outline.
        let outlines = board_outlines(data, &[Layer::BOARD_OUTLINES]);
        if outlines.is_empty() {
            messages.push(DrcMessage::missing_board_outline());
        } else if outlines.len() > 1 {
            messages.push(DrcMessage::multiple_board_outlines(outlines.clone()));
        }

        // Check for intersections between several board outlines.
        for (i, (polygon1, device1, path1)) in objects.iter().enumerate() {
            for (polygon2, device2, path2) in &objects[(i + 1)..] {
                if (polygon1.layer == Layer::BOARD_OUTLINES)
                    && (polygon2.layer == Layer::BOARD_OUTLINES)
                {
                    let mut intersections = to_clipper(std::slice::from_ref(path1));
                    clipper_helpers::intersect(
                        &mut intersections,
                        &to_clipper(std::slice::from_ref(path2)),
                        EvenOdd,
                        EvenOdd,
                    )?;
                    if !intersections.is_empty() {
                        messages.push(DrcMessage::intersecting_board_outlines(
                            polygon1,
                            *device1,
                            polygon2,
                            *device2,
                            from_clipper(&intersections),
                        ));
                    }
                }
            }
        }

        // Check if there are any cutouts located outside the board outline.
        // Intended especially to catch cutouts at the board edge which were
        // made with separate polygons instead of drawing a single board
        // outline polygon.
        let mut outlines_area = to_clipper(&outlines);
        for (polygon, device, path) in &objects {
            if polygon.layer != Layer::BOARD_OUTLINES {
                let mut subtraction = to_clipper(std::slice::from_ref(path));
                clipper_helpers::subtract(&mut subtraction, &outlines_area, NonZero, NonZero)?;
                if !subtraction.is_empty() {
                    messages.push(DrcMessage::cutout_outside_board_area(
                        &polygon.uuid,
                        device.map(|d| &d.uuid),
                        from_clipper(&subtraction),
                    ));
                }
            }
        }

        // Check if the board outline can be manufactured with the smallest
        // tool.
        let min_edge_radius = unsigned(*data.settings.min_outline_tool_diameter() / 2)?;
        if *min_edge_radius > Length::ZERO {
            let offset1 = (*min_edge_radius - Length::new(10000)).max(Length::ZERO);
            let offset2 = -*min_edge_radius;
            let cutouts =
                board_outlines(data, &[Layer::BOARD_CUTOUTS, Layer::BOARD_PLATED_CUTOUTS]);
            clipper_helpers::subtract(&mut outlines_area, &to_clipper(&cutouts), NonZero, NonZero)?;
            let mut non_manufacturable = outlines_area.clone();
            let tolerance = max_arc_tolerance();
            clipper_helpers::offset(&mut non_manufacturable, offset1, tolerance, JoinType::Round)?;
            clipper_helpers::offset(&mut non_manufacturable, offset2, tolerance, JoinType::Round)?;
            let difference = clipper_helpers::subtract_to_tree(
                &non_manufacturable,
                &outlines_area,
                EvenOdd,
                EvenOdd,
                true,
            )?;
            let non_manufacturable = clipper_helpers::flatten_tree(difference.root())?;
            if !non_manufacturable.is_empty() {
                messages.push(DrcMessage::minimum_board_outline_inner_radius_violation(
                    min_edge_radius,
                    from_clipper(&non_manufacturable),
                ));
            }
        }
        Ok(messages)
    }

    fn check_board_cutouts(&self, data: &BoardDrcData, calc: &CalculatedJobData) -> Messages {
        let mut messages = Vec::new();
        self.status(&tr!(CTX, "Check board cutouts..."));

        let is_cutout =
            |layer| [Layer::BOARD_CUTOUTS, Layer::BOARD_PLATED_CUTOUTS].contains(&layer);
        if !data.polygons.iter().any(|p| is_cutout(p.layer))
            && !data.devices.values().any(|dev| {
                dev.polygons.iter().any(|p| is_cutout(p.layer))
                    || dev.circles.iter().any(|c| is_cutout(c.layer))
            })
        {
            return Ok(messages);
        }

        // Helper to stroke a path.
        let get_locations = |path: &Path, line_width: UnsignedLength| -> Result<Vec<Path>> {
            Ok(path.to_outline_strokes(positive((*line_width).max(Length::new(100_000)))?))
        };

        let top = calc.copper(Layer::TOP_COPPER);
        let bottom = calc.copper(Layer::BOT_COPPER);
        let united_copper = rational_union::union(&[top.operand(), bottom.operand()])?.0;
        let intersected_copper = top.intersection(&bottom)?;

        let clipped_locations =
            |region: &Region, outline: &Path, inside: bool| -> Result<Vec<Path>> {
                let inputs =
                    Operand::from_integer(&to_clipper(std::slice::from_ref(outline)), EvenOdd);
                let mut locations = Vec::new();
                for path in &inputs.paths {
                    locations.extend(exact_line_locations(&region.clip_polyline(path, inside)?)?);
                }
                Ok(locations)
            };
        let non_plated_but_intersected_by_copper = |layer: Layer, outline: &Path| {
            if (layer != Layer::BOARD_CUTOUTS) || !outline.is_closed() {
                return Ok(Vec::new());
            }
            clipped_locations(&united_copper, outline, true)
        };
        let plated_but_not_surrounded_by_copper = |layer: Layer, outline: &Path| {
            if (layer != Layer::BOARD_PLATED_CUTOUTS) || !outline.is_closed() {
                return Ok(Vec::new());
            }
            clipped_locations(&intersected_copper, outline, false)
        };

        // Check all plated and non-plated cutouts.
        let mut plated_cutouts_locations = Vec::new();
        let mut check = |object: (&str, &crate::types::Uuid),
                         device: Option<&DrcDevice>,
                         layer: Layer,
                         outline: &Path,
                         line_width: UnsignedLength,
                         messages: &mut Vec<DrcMessage>|
         -> Result<()> {
            let locations = non_plated_but_intersected_by_copper(layer, outline)?;
            if !locations.is_empty() {
                messages.push(DrcMessage::non_plated_cutout_with_copper(
                    object, device, locations,
                ));
            } else {
                let locations = plated_but_not_surrounded_by_copper(layer, outline)?;
                if !locations.is_empty() {
                    messages.push(DrcMessage::plated_cutout_without_copper(
                        object, device, locations,
                    ));
                }
            }
            if layer == Layer::BOARD_PLATED_CUTOUTS {
                plated_cutouts_locations.extend(get_locations(outline, line_width)?);
            }
            Ok(())
        };
        for polygon in &data.polygons {
            check(
                ("polygon", &polygon.uuid),
                None,
                polygon.layer,
                &polygon.path,
                polygon.line_width,
                &mut messages,
            )?;
        }
        for dev in data.devices.values() {
            let transform = dev.transform();
            for polygon in &dev.polygons {
                check(
                    ("polygon", &polygon.uuid),
                    Some(dev),
                    polygon.layer,
                    &transform.map(&polygon.path),
                    polygon.line_width,
                    &mut messages,
                )?;
            }
            for circle in &dev.circles {
                // Note: upstream places the circle at the device position
                // (not at its transformed center).
                let outline = Path::circle(circle.diameter).translated(dev.position);
                check(
                    ("circle", &circle.uuid),
                    Some(dev),
                    circle.layer,
                    &outline,
                    circle.line_width,
                    &mut messages,
                )?;
            }
        }

        // Warn if there are any plated cutouts.
        if !plated_cutouts_locations.is_empty() {
            messages.push(DrcMessage::plated_cutouts(plated_cutouts_locations));
        }
        Ok(messages)
    }

    fn check_used_layers(&self, data: &BoardDrcData) -> Messages {
        self.status(&tr!(CTX, "Check used layers..."));

        // Determine all used copper layers.
        let mut used_layers = BTreeSet::new();
        used_layers.insert(Layer::TOP_COPPER); // Can't be disabled -> no warning.
        used_layers.insert(Layer::BOT_COPPER); // Can't be disabled -> no warning.
        for dev in data.devices.values() {
            let transform = dev.transform();
            for polygon in &dev.polygons {
                if polygon.layer.is_copper() {
                    used_layers.insert(transform.map(&polygon.layer));
                }
            }
            for circle in &dev.circles {
                if circle.layer.is_copper() {
                    used_layers.insert(transform.map(&circle.layer));
                }
            }
        }
        for ns in data.segments.values() {
            used_layers.extend(ns.traces.iter().map(|t| t.layer));
        }
        used_layers.extend(data.planes.iter().map(|p| p.layer));
        used_layers.extend(
            data.polygons
                .iter()
                .filter(|p| p.layer.is_copper())
                .map(|p| p.layer),
        );
        used_layers.extend(
            data.stroke_texts
                .iter()
                .filter(|t| t.layer.is_copper())
                .map(|t| t.layer),
        );

        // Sort layers by copper number.
        let sorted = |layers: Vec<Layer>| {
            let mut layers = layers;
            layers.sort_by_key(|l| l.copper_number());
            layers
        };

        // Warn about disabled layers.
        let mut messages: Vec<DrcMessage> = sorted(
            used_layers
                .difference(&data.copper_layers)
                .copied()
                .collect(),
        )
        .into_iter()
        .map(DrcMessage::disabled_layer)
        .collect();

        // Warn about unused layers.
        messages.extend(
            sorted(
                data.copper_layers
                    .difference(&used_layers)
                    .copied()
                    .collect(),
            )
            .into_iter()
            .map(DrcMessage::unused_layer),
        );
        Ok(messages)
    }

    fn check_for_unplaced_components(&self, data: &BoardDrcData) -> Messages {
        // The actual check is already done while extracting the data, so we
        // only need to create the messages.
        self.status(&tr!(CTX, "Check for unplaced components..."));
        Ok(data
            .unplaced_components
            .iter()
            .map(|(uuid, name)| DrcMessage::missing_device(uuid, name))
            .collect())
    }

    fn check_for_missing_connections(&self, data: &BoardDrcData) -> Messages {
        self.status(&tr!(CTX, "Check for missing connections..."));

        let convert_anchor = |anchor: &DrcAirWireAnchor| -> Result<ConnectionAnchor<'_>> {
            let invalid = || Error::InvalidAirWireAnchor;
            let segment = |uuid| data.segments.get(uuid).ok_or_else(invalid);
            match anchor.object.as_ref().ok_or_else(invalid)? {
                DrcAirWireObject::FootprintPad { device, pad } => {
                    let dev = data.devices.get(device).ok_or_else(invalid)?;
                    let pad = dev.pads.get(pad).ok_or_else(invalid)?;
                    Ok(ConnectionAnchor::FootprintPad(dev, pad))
                }
                DrcAirWireObject::Pad { segment: s, pad } => {
                    let ns = segment(s)?;
                    ns.pads.get(pad).ok_or_else(invalid)?;
                    Ok(ConnectionAnchor::Pad(ns))
                }
                DrcAirWireObject::Junction {
                    segment: s,
                    junction,
                } => {
                    let ns = segment(s)?;
                    ns.junctions.get(junction).ok_or_else(invalid)?;
                    Ok(ConnectionAnchor::Junction(ns))
                }
                DrcAirWireObject::Via { segment: s, via } => {
                    let ns = segment(s)?;
                    ns.vias.get(via).ok_or_else(invalid)?;
                    Ok(ConnectionAnchor::Via(ns))
                }
            }
        };

        // No check based on copper paths implemented yet -> return existing
        // air wires instead (they have been rebuilt when starting the DRC).
        data.air_wires
            .iter()
            .map(|aw| {
                let locations = vec![Path::obround_line(
                    aw.p1.position,
                    aw.p2.position,
                    positive_const(50_000),
                )];
                Ok(DrcMessage::missing_connection(
                    &convert_anchor(&aw.p1)?,
                    &convert_anchor(&aw.p2)?,
                    &aw.net_name,
                    locations,
                ))
            })
            .collect()
    }

    fn check_for_impossible_connections(&self, data: &BoardDrcData) -> Messages {
        self.status(&tr!(CTX, "Check for impossible connections..."));

        // See https://github.com/LibrePCB/LibrePCB/issues/1510.
        let mut messages = Vec::new();
        for dev in data.devices.values() {
            for con in &dev.impossible_signal_connections {
                messages.push(DrcMessage::impossible_connection(
                    dev,
                    &con.signal_uuid,
                    &con.signal_name,
                    &con.net_name,
                    device_location(dev),
                ));
            }
        }
        Ok(messages)
    }

    fn check_for_stale_objects(&self, data: &BoardDrcData) -> Messages {
        let mut messages = Vec::new();
        self.status(&tr!(CTX, "Check for stale objects..."));
        for ns in data.segments.values() {
            // Warn about empty net segments.
            if ns.junctions.is_empty()
                && ns.traces.is_empty()
                && ns.vias.is_empty()
                && ns.pads.is_empty()
            {
                messages.push(DrcMessage::empty_net_segment(ns));
            }

            // Warn about junctions without any traces.
            for junction in ns.junctions.values() {
                if junction.traces == 0 {
                    let locations =
                        vec![Path::circle(positive_const(300_000)).translated(junction.position)];
                    messages.push(DrcMessage::unconnected_junction(junction, ns, locations));
                }
            }
        }
        Ok(messages)
    }
}

fn positive_const(nm: i64) -> PositiveLength {
    PositiveLength::new(Length::new(nm)).expect("constant is positive")
}

/// Returns the (transformed) stroke paths of a text, stroked with at least
/// 50µm.
fn stroke_text_locations(st: &super::data::DrcStrokeText) -> Result<Vec<Path>> {
    let transform = Transform::new(st.position, st.rotation, st.mirror);
    let width = positive((*st.stroke_width).max(Length::new(50000)))?;
    Ok(transform
        .map(&st.paths)
        .iter()
        .flat_map(|p| p.to_outline_strokes(width))
        .collect())
}

/// Upstream `checkMinimumWidth()`.
fn check_minimum_width(
    data: &BoardDrcData,
    min_width: UnsignedLength,
    layer_filter: impl Fn(Layer) -> bool,
) -> Messages {
    let mut messages = Vec::new();
    let stroke = |path: &Path, width: UnsignedLength| -> Result<Vec<Path>> {
        Ok(path.to_outline_strokes(positive((*width).max(Length::new(50000)))?))
    };

    // Stroke texts.
    for st in &data.stroke_texts {
        if !layer_filter(st.layer) {
            continue;
        }
        if st.stroke_width < min_width {
            messages.push(DrcMessage::minimum_width_stroke_text(
                st,
                None,
                min_width,
                stroke_text_locations(st)?,
            ));
        }
    }

    // Polygons.
    for polygon in &data.polygons {
        // Filled polygons with line width 0 have no strokes in Gerber files.
        if polygon.filled && polygon.path.is_closed() && (*polygon.line_width == Length::ZERO) {
            continue;
        }
        if !layer_filter(polygon.layer) {
            continue;
        }
        if polygon.line_width < min_width {
            messages.push(DrcMessage::minimum_width_polygon(
                polygon,
                min_width,
                stroke(&polygon.path, polygon.line_width)?,
            ));
        }
    }

    // Planes.
    for plane in &data.planes {
        if !layer_filter(plane.layer) {
            continue;
        }
        let min_net_width = min_width.max(data.min_copper_width(plane.net_class));
        if plane.min_width < min_net_width {
            messages.push(DrcMessage::minimum_width_plane(
                plane,
                min_net_width,
                plane_location(plane)?,
            ));
        }
    }

    // Devices.
    for dev in data.devices.values() {
        let transform = dev.transform();
        for st in &dev.stroke_texts {
            // Do *not* mirror layer since it is independent of the device!
            if !layer_filter(st.layer) {
                continue;
            }
            if st.stroke_width < min_width {
                messages.push(DrcMessage::minimum_width_stroke_text(
                    st,
                    Some(dev),
                    min_width,
                    stroke_text_locations(st)?,
                ));
            }
        }
        for polygon in &dev.polygons {
            // Filled polygons with line width 0 have no strokes in Gerber
            // files.
            if polygon.filled && polygon.path.is_closed() && (*polygon.line_width == Length::ZERO) {
                continue;
            }
            if !layer_filter(transform.map(&polygon.layer)) {
                continue;
            }
            if polygon.line_width < min_width {
                messages.push(DrcMessage::minimum_width_device_polygon(
                    dev,
                    polygon,
                    min_width,
                    stroke(&transform.map(&polygon.path), polygon.line_width)?,
                ));
            }
        }
        for circle in &dev.circles {
            if !layer_filter(transform.map(&circle.layer)) {
                continue;
            }
            // Filled circles are a single (zero-length) stroke in Gerber
            // files.
            let outer_diameter = circle.diameter + circle.line_width;
            let relevant_width = if circle.filled {
                UnsignedLength::from(outer_diameter)
            } else {
                circle.line_width
            };
            if relevant_width < min_width {
                let locations =
                    vec![transform.map(&Path::circle(outer_diameter).translated(circle.center))];
                messages.push(DrcMessage::minimum_width_device_circle(
                    dev, circle, min_width, locations,
                ));
            }
        }
    }

    // Net lines.
    for ns in data.segments.values() {
        for trace in &ns.traces {
            if !layer_filter(trace.layer) {
                continue;
            }
            let min_net_width = min_width.max(data.min_copper_width(ns.net_class));
            if trace.width < min_net_width {
                messages.push(DrcMessage::minimum_width_trace(
                    ns,
                    trace,
                    min_net_width,
                    trace_location(trace),
                ));
            }
        }
    }
    Ok(messages)
}

/// Upstream `requiresHoleSlotWarning()`.
fn requires_hole_slot_warning(hole: &DrcHole, allowed: AllowedSlots) -> bool {
    let count = hole.path.vertices().len();
    (hole.path.is_curved() && (allowed < AllowedSlots::Any))
        || ((count > 2) && (allowed < AllowedSlots::MultiSegmentStraight))
        || ((count > 1) && (allowed < AllowedSlots::SingleSegmentStraight))
}

/// Upstream `getBoardClearanceArea()`.
fn board_clearance_area(data: &BoardDrcData, clearance: UnsignedLength) -> Result<ClipperPaths> {
    let outlines = board_outlines(data, &[Layer::BOARD_OUTLINES, Layer::BOARD_CUTOUTS]);
    // Larger tolerance is required to avoid false-positives, see
    // https://github.com/LibrePCB/LibrePCB/issues/1434.
    let width =
        positive((*clearance + *clearance - (*max_arc_tolerance() * 2)).max(Length::new(1)))?;
    let mut result: ClipperPaths = outlines
        .iter()
        .flat_map(|outline| to_clipper(&outline.to_outline_strokes(width)))
        .collect();
    clipper_helpers::unite(&mut result, NonZero)?;
    Ok(result)
}

/// Upstream `getBoardOutlines()`: all closed polygons (and circles of
/// devices) on the given layers.
fn board_outlines(data: &BoardDrcData, layers: &[Layer]) -> Vec<Path> {
    let mut outlines = Vec::new();
    for polygon in &data.polygons {
        if layers.contains(&polygon.layer) && polygon.path.is_closed() {
            outlines.push(polygon.path.clone());
        }
    }
    for dev in data.devices.values() {
        let transform = dev.transform();
        for polygon in &dev.polygons {
            if layers.contains(&polygon.layer) && polygon.path.is_closed() {
                outlines.push(transform.map(&polygon.path));
            }
        }
        for circle in &dev.circles {
            if layers.contains(&circle.layer) {
                outlines
                    .push(transform.map(&Path::circle(circle.diameter).translated(circle.center)));
            }
        }
    }
    outlines
}

/// Upstream `getDeviceOutlinePaths()`.
fn device_outline_paths(device: &DrcDevice, layer: Layer) -> Result<ClipperPaths> {
    let mut paths = ClipperPaths::new();
    let transform = device.transform();
    for polygon in &device.polygons {
        if transform.map(&polygon.layer) != layer {
            continue;
        }
        let path = transform.map(&polygon.path);
        clipper_helpers::unite_with(
            &mut paths,
            &[clipper_helpers::path_to_clipper(&path, max_arc_tolerance())],
            NonZero,
            NonZero,
        )?;
    }
    for circle in &device.circles {
        if transform.map(&circle.layer) != layer {
            continue;
        }
        let absolute_pos = transform.map(&circle.center);
        let path = Path::circle(circle.diameter).translated(absolute_pos);
        clipper_helpers::unite_with(
            &mut paths,
            &[clipper_helpers::path_to_clipper(&path, max_arc_tolerance())],
            NonZero,
            NonZero,
        )?;
    }
    Ok(paths)
}

/// Upstream `getDeviceLocation()`.
fn device_location(device: &DrcDevice) -> Vec<Path> {
    let mut locations = Vec::new();
    let transform = device.transform();

    // Helper function to add drawings on a particular layer.
    let add_drawing = |layer: Layer, locations: &mut Vec<Path>| {
        let mut add_path = |path: Path, line_width: UnsignedLength, fill: bool| {
            let path = transform.map(&path);
            if let Ok(width) = PositiveLength::new(*line_width) {
                locations.extend(path.to_outline_strokes(width));
            }
            if path.is_closed() && fill {
                locations.push(path);
            }
        };
        for polygon in &device.polygons {
            if polygon.layer == layer {
                add_path(polygon.path.clone(), polygon.line_width, polygon.filled);
            }
        }
        for circle in &device.circles {
            if circle.layer == layer {
                add_path(
                    Path::circle(circle.diameter).translated(circle.center),
                    circle.line_width,
                    circle.filled,
                );
            }
        }
    };

    // Add drawings on documentation layer.
    add_drawing(Layer::TOP_DOCUMENTATION, &mut locations);
    add_drawing(Layer::BOT_DOCUMENTATION, &mut locations);

    // If there's no documentation, add drawings on placement layer.
    if locations.is_empty() {
        add_drawing(Layer::TOP_LEGEND, &mut locations);
        add_drawing(Layer::BOT_LEGEND, &mut locations);
    }

    // Add origin cross.
    let origin_line = Path::new(vec![
        Vertex::at(Point::from_nm(-500_000, 0)),
        Vertex::at(Point::from_nm(500_000, 0)),
    ]);
    let stroke_width = positive_const(50_000);
    locations.extend(
        origin_line
            .translated(device.position)
            .to_outline_strokes(stroke_width),
    );
    locations.extend(
        origin_line
            .rotated(Angle::DEG90, Point::ORIGIN)
            .translated(device.position)
            .to_outline_strokes(stroke_width),
    );
    locations
}

/// Upstream `getViaLocation()`.
fn via_location(via: &DrcVia) -> Vec<Path> {
    vec![Path::circle(via.size).translated(via.position)]
}

/// Upstream `isViaUseless()`.
fn is_via_useless(data: &BoardDrcData, ns: &DrcSegment, via: &DrcVia) -> Result<bool> {
    // The layers of the traces directly connected to the via have already
    // been added, so if there are two or more connected layers, the via is
    // not useless and we can skip all the other checks.
    if via.connected_layers.len() >= 2 {
        return Ok(false);
    }

    let mut connected_layers = via.connected_layers.clone();
    let via_position = clipper_helpers::point_to_clipper(via.position);
    let start = via.start_layer.copper_number();
    let end = via.end_layer.copper_number();

    // A via is considered to be connected to a plane if it contains the
    // layer the plane is on, both of them are part of a net, they are in
    // the same net, and the via is physically within the plane.
    for plane in &data.planes {
        let copper_number = plane.layer.copper_number();
        if (start > copper_number)
            || (end < copper_number)
            || plane.net.is_none()
            || ns.net.is_none()
            || (plane.net != ns.net)
            || connected_layers.contains(&plane.layer)
        {
            continue;
        }
        let plane_path = clipper_helpers::path_to_clipper(&plane.outline, max_arc_tolerance());
        if clipper::point_in_polygon(via_position, &plane_path) != 0 {
            connected_layers.insert(plane.layer);
            if connected_layers.len() >= 2 {
                return Ok(false);
            }
        }
    }

    // For each pad geometry that shares a layer with the current via, check
    // if the center of the via is in the outline of the pad.
    for device in data.devices.values() {
        for pad in device.pads.values() {
            let transform = pad_transform(pad);
            for (layer, geometries) in &pad.geometries {
                let copper_number = layer.copper_number();
                if !layer.is_copper()
                    || (start > copper_number)
                    || (end < copper_number)
                    || connected_layers.contains(layer)
                {
                    continue;
                }
                for geometry in geometries {
                    for outline in geometry.to_outlines()? {
                        let pad_path = clipper_helpers::path_to_clipper(
                            &transform.map(&outline),
                            max_arc_tolerance(),
                        );
                        if clipper::point_in_polygon(via_position, &pad_path) != 0 {
                            connected_layers.insert(*layer);
                            if connected_layers.len() >= 2 {
                                return Ok(false);
                            }
                        }
                    }
                }
            }
        }
    }

    // If we get here, connected_layers must have a size of zero or one, so
    // the via is probably useless.
    Ok(true)
}

/// Returns the area of a trace.
fn trace_area(trace: &DrcTrace) -> Path {
    Path::obround_line(trace.p1, trace.p2, trace.width)
}

/// Upstream `getTraceLocation()`.
fn trace_location(trace: &DrcTrace) -> Vec<Path> {
    vec![trace_area(trace)]
}

/// Upstream `getPlaneLocation()`.
fn plane_location(plane: &DrcPlane) -> Result<Vec<Path>> {
    Ok(closed(&plane.outline).to_outline_strokes(positive(Length::new(200_000))?))
}

/// Upstream `getHoleLocation()`.
fn hole_location(hole: &DrcHole, transform: &Transform) -> Vec<Path> {
    transform
        .map(hole.path.get())
        .to_outline_strokes(hole.diameter)
}

#[cfg(test)]
mod exact_geometry_tests {
    use super::*;
    use crate::utils::rational_union::Point as ExactPoint;
    use dashu_ratio::RBig;

    fn rectangle(x: i64, y: i64, w: i64, h: i64) -> Region {
        rational_union::union(&[Operand {
            paths: vec![
                [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
                    .into_iter()
                    .map(|(x, y)| ExactPoint::from_nm(x, y))
                    .collect(),
            ],
            fill: NonZero,
        }])
        .unwrap()
        .0
    }

    #[test]
    fn parallel_pairs_preserve_order_locations_and_multilayer_merging() {
        let polygons: Vec<_> = (0..3)
            .map(|_| DrcPolygon {
                uuid: crate::types::Uuid::new_random(),
                layer: Layer::TOP_COPPER,
                line_width: unsigned(Length::ZERO).unwrap(),
                filled: true,
                path: Path::default(),
            })
            .collect();
        // The same three objects on two layers must yield three merged messages,
        // in the original pair order, retaining locations from both layers.
        let items: Vec<_> = (0..6)
            .map(|i| {
                let layer = if i < 3 {
                    Layer::TOP_COPPER
                } else {
                    Layer::BOT_COPPER
                };
                CopperItem::new(
                    CopperObject::Polygon(&polygons[i % 3], None),
                    (layer, layer),
                    None,
                    Length::new(10 + i as i64),
                    rectangle(i as i64, 0, 20, 20),
                    rectangle(-10, -10, 50, 50),
                )
            })
            .collect();
        let layers = [Layer::TOP_COPPER, Layer::BOT_COPPER].into();
        let mut expected = None;
        for threads in [1, 2, 4] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            for _ in 0..3 {
                let actual = pool
                    .install(|| check_copper_pairs(&items, &layers))
                    .unwrap();
                assert_eq!(
                    actual.iter().map(|v| (v.obj1, v.obj2)).collect::<Vec<_>>(),
                    [(0, 1), (0, 2), (1, 2)]
                );
                for (v, width) in actual.iter().zip([14, 15, 15]) {
                    assert_eq!(v.layers, 1 | (1u64 << 63));
                    assert_eq!(v.clearance, Length::new(width));
                    assert_eq!(v.locations.len(), 4);
                    let mut locations = Vec::new();
                    for offset in [0, 3] {
                        locations.extend(
                            exact_intersection(
                                &items[v.obj1 + offset].copper_area,
                                &items[v.obj2 + offset].clearance_area,
                            )
                            .unwrap(),
                        );
                        locations.extend(
                            exact_intersection(
                                &items[v.obj2 + offset].copper_area,
                                &items[v.obj1 + offset].clearance_area,
                            )
                            .unwrap(),
                        );
                    }
                    assert_eq!(v.locations, locations);
                }
                if let Some(expected) = &expected {
                    assert_eq!(&actual, expected);
                } else {
                    expected = Some(actual);
                }
            }
        }
    }

    #[test]
    fn parallel_pair_errors_are_reported_in_input_order() {
        // Completion order cannot choose the error or turn a failed check into
        // a partial success (which could otherwise clear stored approvals).
        let results = vec![
            Ok(Vec::new()),
            Err(Error::IncompletePlaneRebuild),
            Err(Error::StaleSnapshot),
        ];
        assert!(matches!(
            merge_copper_violations(&[], results),
            Err(Error::IncompletePlaneRebuild)
        ));
        let results = vec![
            Err(Error::StaleSnapshot),
            Err(Error::IncompletePlaneRebuild),
        ];
        assert!(matches!(
            merge_copper_violations(&[], results),
            Err(Error::StaleSnapshot)
        ));
    }

    #[test]
    fn spatial_candidates_retain_asymmetric_clearances_and_full_range() {
        let small = rectangle(0, 0, 2, 2);
        let wide = rectangle(-10, -10, 22, 22);
        let other = rectangle(8, 0, 2, 2);
        assert!(small.intersection(&other).unwrap().is_empty());
        assert!(!wide.intersection(&other).unwrap().is_empty());
        let limit = 0x3fff_ffff_ffff_ffff;
        let full = rectangle(-limit, -limit, 2 * limit, 2 * limit);
        let empty = Region::default();
        let pairs = [
            (&small, &wide),
            (&other, &other),
            (&empty, &empty),
            (&full, &full),
        ];
        let bounds: Vec<_> = pairs
            .iter()
            .map(|(a, b)| copper_envelope(a, b).unwrap())
            .collect();
        // Force internal tree nodes, including envelopes spanning the full range.
        let tree = RTree::bulk_load(
            (0..128)
                .filter_map(|i| {
                    bounds[i % 4].map(|b| GeomWithData::new(Rectangle::from_aabb(b), i))
                })
                .collect(),
        );
        for query in bounds.iter().flatten() {
            let mut actual: Vec<_> = tree
                .locate_in_envelope_intersecting(*query)
                .map(|e| e.data)
                .collect();
            actual.sort_unstable();
            let expected: Vec<_> = (0..128)
                .filter(|&i| bounds[i % 4].is_some_and(|b| b.intersects(query)))
                .collect();
            assert_eq!(actual, expected);
        }
        assert!(bounds[0].unwrap().intersects(&bounds[1].unwrap()));
        assert!(bounds[2].is_none());
    }

    #[test]
    fn layer_masks_preserve_sparse_and_full_stack_spans() {
        let all: BTreeSet<_> = (0..64).map(|n| Layer::copper(n).unwrap()).collect();
        assert_eq!(
            copper_layer_mask(Layer::TOP_COPPER, Layer::BOT_COPPER, &all),
            u64::MAX
        );
        let sparse: BTreeSet<_> = [0, 1, 2, 63].map(|n| Layer::copper(n).unwrap()).into();
        let mask = |a, b| {
            copper_layer_mask(
                Layer::copper(a).unwrap(),
                Layer::copper(b).unwrap(),
                &sparse,
            )
        };
        assert_eq!(mask(1, 62), 0b110);
        assert_eq!(mask(2, 1), 0);
        assert_eq!(mask(0, 1) & mask(2, 63), 0);
        assert_eq!(mask(0, 63) & mask(63, 63), 1u64 << 63);
    }

    #[test]
    fn local_layer_queries_equal_global_booleans_before_rounding() {
        let outer = rectangle(-100, -100, 200, 200);
        let hole = rectangle(-10, -10, 20, 20);
        let donut = outer.difference(&hole).unwrap();
        let fractional = rational_union::union(&[Operand {
            paths: vec![vec![
                ExactPoint::from_nm(-3, -4),
                ExactPoint::from_nm(15, -2),
                ExactPoint::from_nm(1, 20),
            ]],
            fill: NonZero,
        }])
        .unwrap()
        .0
        .intersection(&rectangle(-6, -5, 14, 16))
        .unwrap();
        let layers = [donut, fractional, rectangle(200, 200, 5, 5)];
        let empty = Region::default();
        // Include empty layer sets, explicit missing layers, enclosing holes,
        // remote rings, and fractional boundaries at query-box intersections.
        for stack in [
            vec![],
            vec![&layers[0]],
            vec![&layers[0], &layers[1]],
            vec![&layers[0], &layers[1], &layers[2]],
            vec![&layers[0], &empty],
        ] {
            let operands: Vec<_> = stack.iter().map(|r| r.operand()).collect();
            let any = rational_union::union(&operands).unwrap().0;
            let all = rational_union::intersection(&operands).unwrap();
            for x in -12..12 {
                let query = rectangle(x, x / 2, 4, 6);
                assert_eq!(
                    query
                        .intersection_with_union(stack.iter().copied())
                        .unwrap(),
                    query.intersection(&any).unwrap()
                );
                assert_eq!(
                    query
                        .difference_from_intersection(stack.iter().copied())
                        .unwrap(),
                    query.difference(&all).unwrap()
                );
            }
            assert!(
                empty
                    .intersection_with_union(stack.iter().copied())
                    .unwrap()
                    .is_empty()
            );
            assert!(
                empty
                    .difference_from_intersection(stack.iter().copied())
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn collapsed_display_does_not_hide_a_real_violation() {
        let region = rational_union::union(&[Operand {
            paths: vec![vec![
                ExactPoint::from_nm(0, 0),
                ExactPoint::new(RBig::ONE / RBig::from(3), RBig::ZERO),
                ExactPoint::new(RBig::ONE / RBig::from(3), RBig::from(2)),
                ExactPoint::from_nm(0, 2),
            ]],
            fill: NonZero,
        }])
        .unwrap()
        .0;
        let locations = exact_intersection(&region, &region).unwrap();
        assert!(!locations.is_empty());
        assert!(
            locations
                .iter()
                .flat_map(|p| p.vertices())
                .all(|v| v.pos.x == Length::ZERO)
        );
        assert!(
            exact_intersection(&region, &Region::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cutout_locations_do_not_close_open_intervals() {
        let path = vec![
            ExactPoint::from_nm(0, 0),
            ExactPoint::from_nm(1_000_000, 0),
            ExactPoint::from_nm(1_000_000, 1_000_000),
        ];
        assert_eq!(exact_line_locations(&[path]).unwrap().len(), 2);
        let collapsed = vec![
            ExactPoint::from_nm(0, 0),
            ExactPoint::new(RBig::ONE / RBig::from(3), RBig::ZERO),
        ];
        assert_eq!(exact_line_locations(&[collapsed]).unwrap().len(), 1);
    }
}
