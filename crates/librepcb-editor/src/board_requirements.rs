//! Measure saved board requirements against the actual project geometry.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::project::board::{Board, FootprintPadView, centerline_length_mm};
use librepcb_core::project::board_requirements::{Check, Pad, Requirements, Severity};
use librepcb_core::project::{BoardId, NetSignalId, Project};
use librepcb_core::types::{Layer, Uuid};
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::routing_service::region::{Copper, copper_matching};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Fail,
    Pending,
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingReference {
    Board,
    Device,
    Pad,
    PadNet,
    CopperLayer,
    Net,
    TraceOrVia,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Measurement {
    Missing {
        reference: MissingReference,
    },
    NoAnchoredTracePath,
    Position {
        x_mm: f64,
        y_mm: f64,
    },
    Distance {
        distance_mm: f64,
    },
    TracePath {
        length_mm: f64,
        layer_changes: usize,
        layers: BTreeSet<Layer>,
    },
    NetRouting {
        layers: BTreeSet<Layer>,
        via_count: usize,
    },
    ProtectedCopper {
        object: Uuid,
        changed: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub board: BoardId,
    pub severity: Severity,
    pub source: String,
    /// The normalized target and limit, using the native asset units.
    pub check: Check,
    pub status: Status,
    pub measured: Measurement,
}

impl Finding {
    pub fn blocks_routing(&self) -> bool {
        self.severity == Severity::Hard && matches!(self.status, Status::Fail | Status::Unresolved)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct State {
    pub(crate) findings: Vec<Finding>,
    protected: BTreeMap<Uuid, Copper>,
    protected_rules: BTreeMap<String, Uuid>,
}

pub fn check(project: &Project) -> Result<Vec<Finding>> {
    let requirements = Requirements::load(project)?;
    let boards: BTreeSet<_> = requirements.items().iter().map(|rule| rule.board).collect();
    let mut findings = Vec::new();
    for board in boards {
        findings.extend(State::capture(project, board, &requirements)?.findings);
    }
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(findings)
}

/// Source evidence for direct routing or import inside an atomic editor transaction.
pub struct RoutingGuard {
    board: BoardId,
    requirements: Requirements,
    before: State,
}

impl RoutingGuard {
    pub fn capture(project: &Project, board: BoardId) -> Result<Self> {
        let requirements = Requirements::load(project)?;
        Self::capture_with(project, board, requirements)
    }

    /// Measures the source using the proposed requirements before an edit
    /// that may also change the saved rules.
    pub fn capture_with(
        project: &Project,
        board: BoardId,
        requirements: Requirements,
    ) -> Result<Self> {
        let before = State::capture(project, board, &requirements)?;
        Ok(Self {
            board,
            requirements,
            before,
        })
    }

    pub fn findings_before(&self) -> &[Finding] {
        &self.before.findings
    }

    pub fn findings_after(&self, project: &Project) -> Result<Vec<Finding>> {
        let after = State::capture(project, self.board, &self.requirements)?;
        Ok(after.findings_after(&self.before).collect())
    }

    pub fn enforce(&self, project: &Project) -> Result<()> {
        let blockers: Vec<_> = self
            .findings_after(project)?
            .into_iter()
            .filter(Finding::blocks_routing)
            .map(|finding| finding.id)
            .collect();
        if blockers.is_empty() {
            Ok(())
        } else {
            Err(crate::Error::BoardRequirementsBlocked(blockers.join(", ")))
        }
    }
}

impl State {
    pub(crate) fn capture(
        project: &Project,
        board: BoardId,
        document: &Requirements,
    ) -> Result<Self> {
        let protected_rules: BTreeMap<_, _> = document
            .on_board(board)
            .filter_map(|rule| match rule.check {
                Check::ProtectedCopper { object } => Some((rule.id.clone(), object)),
                _ => None,
            })
            .collect();
        let native = project.board(board);
        let protected = match native {
            Some(native) if !protected_rules.is_empty() => {
                let ids: BTreeSet<_> = protected_rules.values().copied().collect();
                copper_matching(project, native, |object| ids.contains(&object))?
            }
            _ => BTreeMap::new(),
        };
        let mut findings = Vec::new();
        for rule in document.on_board(board) {
            let (status, measured) = match native {
                Some(native) => observe(project, native, &rule.check, &protected)?,
                None => unresolved(MissingReference::Board),
            };
            findings.push(Finding {
                id: rule.id.clone(),
                board,
                severity: rule.severity,
                source: rule.source.clone(),
                check: rule.check.clone(),
                status,
                measured,
            });
        }
        Ok(Self {
            findings,
            protected,
            protected_rules,
        })
    }

    pub(crate) fn findings_after<'a>(
        &'a self,
        before: &'a Self,
    ) -> impl Iterator<Item = Finding> + 'a {
        self.findings.iter().cloned().map(|mut finding| {
            if let Some(id) = before.protected_rules.get(&finding.id)
                && let Some(old) = before.protected.get(id)
                && !self
                    .protected
                    .get(id)
                    .is_some_and(|new| old.same_geometry(new))
            {
                finding.status = Status::Fail;
                finding.measured = Measurement::ProtectedCopper {
                    object: *id,
                    changed: true,
                };
            }
            if finding.status == Status::Pending
                && before.findings.iter().any(|old| {
                    old.id == finding.id && matches!(old.measured, Measurement::TracePath { .. })
                })
            {
                finding.status = Status::Fail;
            }
            finding
        })
    }
}

fn footprint_pad<'a>(
    project: &'a Project,
    board: &'a Board,
    pad: Pad,
) -> Result<Option<FootprintPadView<'a>>> {
    let Some(device) = board.device(pad.component) else {
        return Ok(None);
    };
    Ok(device.pad(&pad.pad, project.library(), project.circuit())?)
}

fn measured(met: bool, value: Measurement) -> (Status, Measurement) {
    (if met { Status::Pass } else { Status::Fail }, value)
}

fn unresolved(reference: MissingReference) -> (Status, Measurement) {
    (Status::Unresolved, Measurement::Missing { reference })
}

fn observe(
    project: &Project,
    board: &Board,
    check: &Check,
    protected: &BTreeMap<Uuid, Copper>,
) -> Result<(Status, Measurement)> {
    let valid_layers = |layers: &Option<BTreeSet<Layer>>| {
        layers.as_ref().is_none_or(|layers| {
            layers
                .iter()
                .all(|layer| board.copper_layers().contains(layer))
        })
    };
    let assigned = |net: &NetSignalId| project.circuit().net_signal(*net).is_some();
    Ok(match check {
        Check::DeviceRegion {
            component,
            min,
            max,
        } => {
            let Some(device) = board.device(*component) else {
                return Ok(unresolved(MissingReference::Device));
            };
            let position = device.position();
            let (x, y) = position.to_mm();
            measured(
                (min.x..=max.x).contains(&position.x) && (min.y..=max.y).contains(&position.y),
                Measurement::Position { x_mm: x, y_mm: y },
            )
        }
        Check::PadDistance {
            from,
            to,
            max_distance,
        } => {
            let (Some(a), Some(b)) = (
                footprint_pad(project, board, *from)?,
                footprint_pad(project, board, *to)?,
            ) else {
                return Ok(unresolved(MissingReference::Pad));
            };
            let distance = centerline_length_mm(a.position(), b.position());
            measured(
                distance <= max_distance.to_mm(),
                Measurement::Distance {
                    distance_mm: distance,
                },
            )
        }
        Check::TracePath {
            from,
            to,
            net,
            max_length,
            max_layer_changes,
            layers,
        } => {
            let (Some(a), Some(b)) = (
                footprint_pad(project, board, *from)?,
                footprint_pad(project, board, *to)?,
            ) else {
                return Ok(unresolved(MissingReference::Pad));
            };
            if a.net() != Some(*net) || b.net() != Some(*net) || !assigned(net) {
                return Ok(unresolved(MissingReference::PadNet));
            }
            if !valid_layers(layers) {
                return Ok(unresolved(MissingReference::CopperLayer));
            }
            let Some(path) = project
                .trace_path(board.id(), *net, from.address(), to.address())
                .map_err(crate::Error::from_export)?
            else {
                return Ok((Status::Pending, Measurement::NoAnchoredTracePath));
            };
            let used: BTreeSet<_> = path.traces.iter().map(|trace| trace.layer).collect();
            measured(
                max_length.is_none_or(|limit| path.length_mm <= limit.to_mm())
                    && max_layer_changes.is_none_or(|limit| path.layer_changes <= limit)
                    && layers
                        .as_ref()
                        .is_none_or(|allowed| used.is_subset(allowed)),
                Measurement::TracePath {
                    length_mm: path.length_mm,
                    layer_changes: path.layer_changes,
                    layers: used,
                },
            )
        }
        Check::NetRouting {
            net,
            layers,
            allow_vias,
            ..
        } => {
            if !assigned(net) {
                return Ok(unresolved(MissingReference::Net));
            }
            if !valid_layers(layers) {
                return Ok(unresolved(MissingReference::CopperLayer));
            }
            let mut used = BTreeSet::new();
            let mut vias = 0;
            for segment in board
                .net_segments()
                .values()
                .filter(|segment| segment.net() == Some(*net))
            {
                used.extend(segment.traces().values().map(|trace| trace.layer()));
                vias += segment.vias().len();
            }
            measured(
                layers
                    .as_ref()
                    .is_none_or(|allowed| used.is_subset(allowed))
                    && (*allow_vias != Some(false) || vias == 0),
                Measurement::NetRouting {
                    layers: used,
                    via_count: vias,
                },
            )
        }
        Check::ProtectedCopper { object } => {
            if protected.contains_key(object) {
                (
                    Status::Pass,
                    Measurement::ProtectedCopper {
                        object: *object,
                        changed: false,
                    },
                )
            } else {
                unresolved(MissingReference::TraceOrVia)
            }
        }
    })
}
