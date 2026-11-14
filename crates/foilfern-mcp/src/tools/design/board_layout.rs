//! Declarative board placement using the ordinary editor commands and routing checks.

use std::collections::{BTreeMap, BTreeSet};

use foilfern_core::geometry::TraceAnchor;
use foilfern_core::project::board::{Board, BoardDevice};
use foilfern_core::project::board_requirements::{PATH, Requirements, VERSION};
use foilfern_core::project::{BoardId, ComponentInstanceId, Project};
use foilfern_core::types::{Angle, Point, Uuid};
use foilfern_editor::board_requirements::RoutingGuard;
use foilfern_editor::commands::{AddDevice, MoveDevice};
use foilfern_editor::design_preview::DesignPreview;
use foilfern_editor::routing_validation::{RoutingValidation, capture_placement_state};
use foilfern_editor::{ProjectEditor, Transaction};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::work::Stage;
use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::tools::board_edit::Side;
use crate::tools::board_requirements::{CheckInput, RuleInput, SeverityInput, requirement_json};
use crate::tools::trace_path::{self, PadAddress, PadIdentity};
use crate::units::{PointMm, angle};
use crate::{resolve, views};

mod legend;
mod repair;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BoardLayout {
    /// Board name, index or UUID. Defaults to the first board.
    board: Option<String>,
    /// Side shown in the candidate PNG (default top). Pad coordinates always
    /// use board X/Y coordinates, including when viewing the bottom side.
    image_side: Option<Side>,
    /// Component keys from the design document, or designators/UUIDs.
    #[serde(default)]
    placements: BTreeMap<String, Placement>,
    /// Bounded silkscreen-text searches; positions use board coordinates.
    #[serde(default)]
    legend_placements: Vec<legend::LegendPlacement>,
    /// Named rectangles for device origins, in board millimeters.
    #[serde(default)]
    regions: BTreeMap<String, Region>,
    /// Add saved board requirements by ID, preserving the existing document.
    #[serde(default)]
    requirements: Vec<BoardRule>,
    /// Suggest local branch repairs, or try only the branches explicitly authorized here.
    repair: Option<repair::RepairInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Placement {
    position: Option<PointMm>,
    relative_to: Option<RelativePosition>,
    /// Counterclockwise rotation in degrees.
    rotation: Option<f64>,
    side: Option<Side>,
    /// Set true to lock a fixed interface after placing it.
    locked: Option<bool>,
    /// Library device UUID. Selects a new device, or asserts the existing identity.
    device: Option<String>,
    /// Footprint UUID. Selects a new footprint, or asserts the existing identity.
    footprint: Option<String>,
    /// Require the final origin to lie in this named region.
    region: Option<String>,
    /// Current values of fields that this document changes on a placed device.
    #[serde(default)]
    expected: ExpectedPose,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExpectedPose {
    position: Option<PointMm>,
    rotation: Option<f64>,
    side: Option<Side>,
    locked: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RelativePosition {
    /// Previously placed component or another placement entry.
    component: String,
    /// Optional package pad name or footprint pad UUID. Omit for the device origin.
    pad: Option<String>,
    /// Offset along the board X/Y axes, in millimeters.
    offset: PointMm,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Region {
    min: PointMm,
    max: PointMm,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BoardRule {
    id: String,
    source: String,
    severity: SeverityInput,
    check: CheckInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pose {
    position: Point,
    rotation: Angle,
    side: Side,
    locked: bool,
}

impl From<&BoardDevice> for Pose {
    fn from(device: &BoardDevice) -> Self {
        Self {
            position: device.position(),
            rotation: device.rotation().mapped_to_0_360deg(),
            side: if device.mirrored() {
                Side::Bottom
            } else {
                Side::Top
            },
            locked: device.locked(),
        }
    }
}

#[derive(Serialize)]
struct PlacedDevice {
    position: PointMm,
    rotation: f64,
    side: Side,
    locked: bool,
    device: Uuid,
    footprint: Uuid,
}

impl From<&BoardDevice> for PlacedDevice {
    fn from(device: &BoardDevice) -> Self {
        let pose = Pose::from(device);
        Self {
            position: pose.position.into(),
            rotation: pose.rotation.to_deg(),
            side: pose.side,
            locked: pose.locked,
            device: device.lib_device(),
            footprint: device.lib_footprint(),
        }
    }
}

pub(super) struct AppliedLayout {
    board: BoardId,
    image_side: Side,
    reviewed_devices: BTreeSet<ComponentInstanceId>,
    new_devices: BTreeSet<ComponentInstanceId>,
    regions: Vec<Value>,
    moved: bool,
    legend: Vec<Value>,
    repair: Option<repair::RepairInput>,
    /// For a combined schematic/board document, compare placement against
    /// the circuit and board after the explicitly requested schematic edits.
    placement_baseline: Option<Project>,
}

pub(super) struct LayoutReview {
    pub(super) evidence: Value,
    pub(super) blocked: bool,
    pub(super) image: Vec<u8>,
}

fn component(
    project: &Project,
    bindings: &BTreeMap<&str, ComponentInstanceId>,
    key: &str,
) -> ToolResult<ComponentInstanceId> {
    match bindings.get(key) {
        Some(&id) => Ok(id),
        None => resolve::component(project, key).map(|(id, _)| id),
    }
}

fn board(project: &Project, id: BoardId) -> ToolResult<&Board> {
    project
        .board(id)
        .ok_or_else(|| ToolError::not_found("Placement board is missing."))
}

fn offset(origin: Point, delta: Point) -> ToolResult<Point> {
    let sum = |a: foilfern_core::types::Length, b: foilfern_core::types::Length| {
        a.to_nm()
            .checked_add(b.to_nm())
            .ok_or_else(|| ToolError::invalid("Relative position is out of range."))
    };
    Ok(Point::from_nm(
        sum(origin.x, delta.x)?,
        sum(origin.y, delta.y)?,
    ))
}

fn expected<T: PartialEq>(
    key: &str,
    field: &str,
    from: T,
    to: T,
    expected: Option<T>,
) -> ToolResult<()> {
    if from != to && expected.as_ref() != Some(&from) {
        return Err(ToolError::new(
            ErrorKind::Conflict,
            format!(
                "{key}: current board {field} differs from the expected value. Read the board before moving it."
            ),
        ));
    }
    Ok(())
}

fn requirement_component(bindings: &BTreeMap<&str, ComponentInstanceId>, key: &mut String) {
    if let Some(id) = bindings.get(key.as_str()) {
        *key = id.to_string();
    }
}

fn requirement_pad(
    bindings: &BTreeMap<&str, ComponentInstanceId>,
    pad: &mut PadAddress,
) -> ToolResult<()> {
    match pad {
        PadAddress::Identity(identity) => requirement_component(bindings, &mut identity.component),
        PadAddress::Label(label) => {
            let (key, pin) = resolve::split_pin_ref(label)?;
            if let Some(id) = bindings.get(key) {
                *pad = PadAddress::Identity(PadIdentity {
                    component: id.to_string(),
                    pad: pin.to_owned(),
                });
            }
        }
    }
    Ok(())
}

impl BoardLayout {
    pub(super) fn apply(
        self,
        tx: &mut Transaction<'_>,
        bindings: &BTreeMap<&str, ComponentInstanceId>,
        combined: bool,
        diff: &mut Vec<Value>,
        mut checkpoint: impl FnMut() -> ToolResult<()>,
    ) -> ToolResult<AppliedLayout> {
        let project = tx.project();
        let (_, id, _) = resolve::board(project, self.board.as_deref())?;
        let placement_baseline = if combined {
            Some(project.snapshot()?)
        } else {
            None
        };
        let regions = self
            .regions
            .into_iter()
            .map(|(name, region)| {
                let min = region.min.to_point()?;
                let max = region.max.to_point()?;
                if name.trim().is_empty() || min.x > max.x || min.y > max.y {
                    return Err(ToolError::invalid(format!(
                        "Invalid placement region {name:?}."
                    )));
                }
                Ok((name, (min, max)))
            })
            .collect::<ToolResult<BTreeMap<_, _>>>()?;

        // Kahn's algorithm. Apply anchors first, including when the document
        // names dependents before their anchors.
        let mut selected = BTreeMap::new();
        for (key, placement) in self.placements {
            let instance = component(project, bindings, &key)?;
            let dependency = placement
                .relative_to
                .as_ref()
                .map(|relative| component(project, bindings, &relative.component))
                .transpose()?;
            if selected
                .insert(instance, (key, placement, dependency))
                .is_some()
            {
                return Err(ToolError::invalid(format!(
                    "Duplicate placement of component {instance}."
                )));
            }
        }
        let reviewed_devices = selected.keys().copied().collect();
        let mut users = BTreeMap::<_, Vec<_>>::new();
        let mut ready = BTreeSet::new();
        for (&instance, (_, _, dependency)) in &selected {
            match dependency.filter(|anchor| selected.contains_key(anchor)) {
                Some(anchor) => users.entry(anchor).or_default().push(instance),
                None => {
                    ready.insert(instance);
                }
            }
        }
        let mut new_devices = BTreeSet::new();
        let mut moved = false;
        let mut region_checks = Vec::new();
        while let Some(instance) = ready.pop_first() {
            checkpoint()?;
            let Some((key, spec, dependency)) = selected.remove(&instance) else {
                continue;
            };
            let project = tx.project();
            let layout = board(project, id)?;
            let position = match (spec.position, spec.relative_to.as_ref().zip(dependency)) {
                (Some(value), None) => Some(value.to_point()?),
                (None, Some((relative, anchor))) => {
                    let device = layout.device(anchor).ok_or_else(|| {
                        ToolError::invalid(format!(
                            "{key}: relative anchor {:?} must be placed on the board.",
                            relative.component
                        ))
                    })?;
                    let origin = if let Some(pad) = &relative.pad {
                        let address = PadAddress::Identity(PadIdentity {
                            component: anchor.to_string(),
                            pad: pad.clone(),
                        });
                        let (_, pad) = trace_path::footprint_pad_id(project, layout, &address)?;
                        device
                            .pad(&pad, project.library(), project.circuit())?
                            .ok_or_else(|| {
                                ToolError::not_found(format!(
                                    "Relative anchor pad {pad} is missing."
                                ))
                            })?
                            .position()
                    } else {
                        device.position()
                    };
                    Some(offset(origin, relative.offset.to_point()?)?)
                }
                (None, None) => None,
                (Some(_), Some(_)) => {
                    return Err(ToolError::invalid(format!(
                        "{key}: specify position or relative_to."
                    )));
                }
            };
            let old = layout.device(instance);
            let before = old.map(PlacedDevice::from);
            let current = old.map(Pose::from);
            let desired = Pose {
                position: position.or(current.map(|p| p.position)).ok_or_else(|| {
                    ToolError::invalid(format!(
                        "{key}: an unplaced device needs a position or relative_to."
                    ))
                })?,
                rotation: spec
                    .rotation
                    .map(angle)
                    .transpose()?
                    .map(Angle::mapped_to_0_360deg)
                    .or(current.map(|p| p.rotation))
                    .unwrap_or(Angle::DEG0),
                side: spec.side.or(current.map(|p| p.side)).unwrap_or(Side::Top),
                locked: spec.locked.or(current.map(|p| p.locked)).unwrap_or(false),
            };
            if let Some(region) = &spec.region {
                let &(min, max) = regions.get(region).ok_or_else(|| {
                    ToolError::invalid(format!("{key}: unknown placement region {region:?}."))
                })?;
                let Point { x, y } = desired.position;
                region_checks.push(json!({"component":instance,"key":key,"region":region,
                    "min":views::point(min),"max":views::point(max),"position":views::point(desired.position),
                    "met":(min.x..=max.x).contains(&x) && (min.y..=max.y).contains(&y)}));
            }
            let device = spec
                .device
                .as_deref()
                .map(|d| resolve::required_uuid(d, "device"))
                .transpose()?;
            let footprint = spec
                .footprint
                .as_deref()
                .map(|f| resolve::required_uuid(f, "footprint"))
                .transpose()?;
            if let Some(old) = old
                && (device.is_some_and(|wanted| wanted != old.lib_device())
                    || footprint.is_some_and(|wanted| wanted != old.lib_footprint()))
            {
                return Err(ToolError::new(
                    ErrorKind::Conflict,
                    format!(
                        "{key}: the placed device or footprint differs from the requested identity."
                    ),
                ));
            }
            if let Some(current) = current {
                expected(
                    &key,
                    "position",
                    current.position,
                    desired.position,
                    spec.expected.position.map(PointMm::to_point).transpose()?,
                )?;
                expected(
                    &key,
                    "rotation",
                    current.rotation,
                    desired.rotation,
                    spec.expected
                        .rotation
                        .map(angle)
                        .transpose()?
                        .map(Angle::mapped_to_0_360deg),
                )?;
                expected(&key, "side", current.side, desired.side, spec.expected.side)?;
                expected(
                    &key,
                    "lock",
                    current.locked,
                    desired.locked,
                    spec.expected.locked,
                )?;
                let changes_pose = current.position != desired.position
                    || current.rotation != desired.rotation
                    || current.side != desired.side;
                if current.locked && changes_pose {
                    return Err(ToolError::refused(format!("{key}: the device is locked.")));
                }
                moved |= changes_pose;
                if current != desired {
                    tx.run(MoveDevice {
                        component: instance.into(),
                        board: Some(id),
                        position: (current.position != desired.position)
                            .then_some(desired.position),
                        rotation: (current.rotation != desired.rotation)
                            .then_some(desired.rotation),
                        mirrored: (current.side != desired.side)
                            .then_some(desired.side == Side::Bottom),
                        locked: (current.locked != desired.locked).then_some(desired.locked),
                    })?;
                }
            } else {
                tx.run(AddDevice {
                    component: instance.into(),
                    board: Some(id),
                    device,
                    footprint,
                    position: desired.position,
                    rotation: desired.rotation,
                    mirrored: desired.side == Side::Bottom,
                })?;
                if desired.locked {
                    tx.run(MoveDevice {
                        component: instance.into(),
                        board: Some(id),
                        position: None,
                        rotation: None,
                        mirrored: None,
                        locked: Some(true),
                    })?;
                }
                new_devices.insert(instance);
            }
            if current != Some(desired) {
                let project = tx.project();
                let device = board(project, id)?.device(instance).ok_or_else(|| {
                    ToolError::not_found(format!("Placed device {instance} is missing."))
                })?;
                diff.push(
                    json!({"field":"board_placement","board":id,"component":instance,
                    "key":key,"designator":resolve::designator(project, instance),
                    "before":before,"after":PlacedDevice::from(device)}),
                );
            }
            ready.extend(users.remove(&instance).into_iter().flatten());
        }
        if !selected.is_empty() {
            let remaining: Vec<_> = selected.values().map(|(key, _, _)| key.as_str()).collect();
            return Err(ToolError::invalid(format!(
                "Cyclic relative placement involving {}.",
                remaining.join(", ")
            )));
        }

        let legend = legend::apply(self.legend_placements, tx, id, diff, &mut checkpoint)?;

        let project = tx.project();
        let saved = Requirements::load(project).map_err(foilfern_editor::Error::from)?;
        let mut rules: BTreeMap<_, _> = saved
            .items()
            .iter()
            .cloned()
            .map(|rule| (rule.id.clone(), rule))
            .collect();
        let mut seen = BTreeSet::new();
        for mut input in self.requirements {
            checkpoint()?;
            if !seen.insert(input.id.clone()) {
                return Err(ToolError::invalid(format!(
                    "Duplicate board requirement {:?}.",
                    input.id
                )));
            }
            match &mut input.check {
                CheckInput::DeviceRegion { component, .. } => {
                    requirement_component(bindings, component)
                }
                CheckInput::PadDistance { from, to, .. }
                | CheckInput::TracePath { from, to, .. } => {
                    requirement_pad(bindings, from)?;
                    requirement_pad(bindings, to)?;
                }
                _ => {}
            }
            let rule = RuleInput {
                id: input.id,
                source: input.source,
                severity: input.severity,
                board: id.to_string(),
                check: input.check,
            }
            .resolve(project)?;
            if let Some(existing) = rules.get(&rule.id) {
                if existing != &rule {
                    return Err(ToolError::new(
                        ErrorKind::Conflict,
                        format!(
                            "Board requirement {:?} already exists with different values.",
                            rule.id
                        ),
                    ));
                }
            } else {
                diff.push(json!({
                    "field":"board_requirement","board":id,"id":rule.id,
                    "after":requirement_json(&rule),
                }));
                rules.insert(rule.id.clone(), rule);
            }
        }
        let rules = Requirements::new(VERSION, rules.into_values().collect())
            .map_err(foilfern_editor::Error::from)?;
        if rules != saved {
            tx.write_file(PATH, rules.asset().map_err(foilfern_editor::Error::from)?)?;
        }
        Ok(AppliedLayout {
            board: id,
            image_side: self.image_side.unwrap_or(Side::Top),
            reviewed_devices,
            new_devices,
            regions: region_checks,
            moved,
            legend,
            repair: self.repair,
            placement_baseline,
        })
    }
}

fn changed_traces(before: &Project, after: &Project, id: BoardId) -> ToolResult<Vec<Value>> {
    let previous = board(before, id)?;
    let next = board(after, id)?;
    let next_traces: BTreeMap<_, _> = next
        .net_segments()
        .values()
        .flat_map(|segment| segment.traces().keys().map(move |uuid| (*uuid, segment)))
        .collect();
    let mut changed = Vec::new();
    for segment in previous.net_segments().values() {
        for trace in segment.traces().values() {
            let Some(next_segment) = next_traces.get(&trace.uuid()) else {
                continue;
            };
            let anchors = [trace.p1(), trace.p2()];
            if !anchors.into_iter().any(|anchor| match anchor {
                TraceAnchor::FootprintPad { device, .. } => {
                    previous.device(device.into()).map(Pose::from)
                        != next.device(device.into()).map(Pose::from)
                }
                _ => false,
            }) {
                continue;
            }
            let positions = |board: &Board, proj: &Project, segment| {
                anchors.map(|anchor| {
                    board
                        .anchor_position(segment, anchor, proj.library(), proj.circuit())
                        .map(views::point)
                })
            };
            let from = positions(previous, before, segment);
            let to = positions(next, after, next_segment);
            if from != to {
                changed.push(
                    json!({"uuid":trace.uuid(),"net":segment.net(),"layer":trace.layer(),
                    "before":from,"after":to}),
                );
            }
        }
    }
    Ok(changed)
}

impl AppliedLayout {
    pub(super) fn review(
        mut self,
        preview: &mut DesignPreview,
        source: &Project,
        proposed: &Project,
        stage: impl Fn(Stage) -> ToolResult<()> + Sync,
    ) -> ToolResult<LayoutReview> {
        let checkpoint = || stage(Stage::Check);
        let before = self.placement_baseline.as_ref().unwrap_or(source);
        let repair = self
            .repair
            .map(|input| input.resolve(before, self.board, preview.revision, self.moved))
            .transpose()?;
        let cancelled = || checkpoint().is_err();
        let trial = repair
            .as_ref()
            .filter(|request| self.moved && request.authorized())
            .map(|request| request.run(before, proposed, self.board, cancelled))
            .transpose();
        checkpoint()?;
        let trial = trial?;
        let proposed = trial.as_ref().map_or(proposed, |trial| &trial.project);
        let traces = changed_traces(before, proposed, self.board)?;
        let mut original = ProjectEditor::new(before.snapshot()?);
        let mut updated = ProjectEditor::new(proposed.snapshot()?);
        // Compare the same rules on both sides, including protections added
        // by this document. A new rule can protect existing geometry.
        let rules = Requirements::load(proposed).map_err(foilfern_editor::Error::from)?;
        if Requirements::load(before).map_err(foilfern_editor::Error::from)? != rules {
            original.execute_atomic("Preview board requirements", |tx| {
                tx.write_file(PATH, rules.asset()?)
            })?;
        }
        let baseline =
            capture_placement_state(&mut original, self.board, &BTreeSet::new(), &cancelled);
        checkpoint()?;
        let placed =
            capture_placement_state(&mut updated, self.board, &self.new_devices, &cancelled);
        checkpoint()?;
        let baseline = baseline?;
        let placed = placed?;
        let opened = placed.checks.unrouted;
        let (mut validation, courtyard) = RoutingValidation::compare_placement(baseline, placed);
        if self.placement_baseline.is_some() {
            // Schematic edits explicitly authorize netlist changes. Saved
            // board rules still govern the whole patch, including copper
            // affected by those edits.
            let guard = RoutingGuard::capture_with(source, self.board, rules)?;
            validation.board_requirements_before = guard.findings_before().to_vec();
            validation.board_requirements_after = guard.findings_after(proposed)?;
        }
        legend::review(proposed, self.board, &mut self.legend, checkpoint)?;
        let mut blockers = Vec::new();
        if self
            .legend
            .iter()
            .any(|report| report["applicable"] == false)
        {
            blockers.push("no verified clear position for a legend text".to_owned());
        }
        if !validation.before.complete || !validation.after.complete {
            blockers.push("board DRC did not complete".to_owned());
        }
        if validation.new_error_count != 0 {
            blockers.push(format!(
                "board DRC introduced {} error(s)",
                validation.new_error_count
            ));
        }
        if courtyard {
            blockers.push("device courtyard conflict".to_owned());
        }
        if !validation.netlist_preserved {
            blockers.push("placement changed the circuit netlist".to_owned());
        }
        if !validation.unrouted_regressions.is_empty() {
            blockers.push(format!(
                "placement disconnected {} existing net(s)",
                validation.unrouted_regressions.len()
            ));
        }
        for finding in validation
            .board_requirements_after
            .iter()
            .filter(|f| f.blocks_routing())
        {
            blockers.push(format!("board requirement {}", finding.id));
        }
        for region in &self.regions {
            if region["met"] == false {
                blockers.push(format!("{} is outside its placement region", region["key"]));
            }
        }
        let repair_evidence = if let Some(request) = repair {
            if let Some(trial) = trial.as_ref() {
                if !trial.routing.failed_requests.is_empty() {
                    blockers
                        .push("local routing could not reconnect every authorized branch".into());
                }
                Some(request.trial_evidence(before, proposed, self.board, trial)?)
            } else if !self.moved {
                Some(json!({"status":"placement_unchanged"}))
            } else {
                Some(request.proposals(
                    before,
                    proposed,
                    self.board,
                    preview.revision,
                    &traces,
                    &validation,
                )?)
            }
        } else {
            None
        };
        let blocked = !blockers.is_empty();
        validation.acceptable = !blocked;
        preview.review_board(self.board, source, proposed, blockers.clone())?;
        let mut evidence = json!({
            "board":self.board, "applicable":!blocked,"blockers":blockers,
            "legend_placements":self.legend,
            "regions":self.regions,"new_devices":self.new_devices,"changed_traces":traces,
            "unrouted_after":opened,"validation":validation,
        });
        if let Some(repair) = repair_evidence {
            evidence["repair"] = repair;
        }
        let project = updated.project();
        let board = board(project, self.board)?;
        let devices = self
            .reviewed_devices
            .iter()
            .map(|id| {
                let device = board
                    .device(*id)
                    .ok_or_else(|| ToolError::not_found("Reviewed device disappeared."))?;
                crate::tools::circuit::device_json(project, board, device, true)
            })
            .collect::<ToolResult<Vec<_>>>()?;
        evidence["devices"] = json!(devices);
        stage(Stage::Render)?;
        let side = if self.image_side == Side::Bottom {
            foilfern_scene::BoardSide::Bottom
        } else {
            foilfern_scene::BoardSide::Top
        };
        let image = foilfern_scene::render_board_png(
            project,
            self.board,
            side,
            &foilfern_scene::RenderOptions::default(),
        )?;
        stage(Stage::Render)?;
        evidence["image"] = json!({"index":0,"side":self.image_side,"width":1600,"height":1200,"mime_type":"image/png","pad_coordinate_system":"board"});
        if let Some(trial) = trial {
            preview.append_board_routing(trial.mutations);
        }
        Ok(LayoutReview {
            evidence,
            blocked,
            image,
        })
    }
}
