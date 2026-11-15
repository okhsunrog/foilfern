//! Validation of routing against the applied project model (no upstream
//! counterpart). Shared by routing callers, independent of the MCP protocol.
//!
//! Each snapshot runs full DRC, including rebuilt planes and air wires.
//! Finding identities use the upstream approval expressions; missing connections
//! have their own category because their air-wire tree can change after routing.
//! Comparison counts retain duplicate identities; samples use distinct findings.
//! The netlist comparison checks signal assignments, not physical connectivity;
//! shorts and copper geometry are checked by DRC.

use std::collections::{BTreeMap, BTreeSet};

use foilfern_core::geometry::TraceAnchor;
use foilfern_core::project::board::drc::{DrcMessage, DrcMessageKind};
use foilfern_core::project::board::{AirWire, BoardAirWiresBuilder};
use foilfern_core::project::board_requirements::Requirements;
use foilfern_core::project::{BoardId, ComponentInstanceId, NetSignalId};
use foilfern_core::rule_check::Severity;
use foilfern_core::types::{Layer, Uuid};
use serde::{Deserialize, Serialize};

use crate::board_requirements::{Finding, State};
use crate::commands::autoroute::{RoutingObject, RoutingObjectKind};
use crate::{ProjectEditor, Result, routing_stats};

/// A full DRC snapshot of the actual board. Counts exclude approved messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingCheckSummary {
    pub ran: bool,
    pub complete: bool,
    pub unrouted: usize,
    pub errors: usize,
    pub non_connectivity_errors: usize,
    pub missing_connection_errors: usize,
    pub copper_errors: usize,
    pub warnings: usize,
    pub check_failures: Vec<String>,
}

/// Multiset difference of native approval identities, including repeated messages.
/// `before = resolved + unchanged`; `after = added + unchanged`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingFindingDelta {
    pub before: usize,
    pub after: usize,
    pub added: usize,
    pub resolved: usize,
    pub unchanged: usize,
}

impl RoutingFindingDelta {
    fn compare(before: &BTreeMap<String, usize>, after: &BTreeMap<String, usize>) -> Self {
        let unchanged = before
            .iter()
            .map(|(key, count)| (*count).min(after.get(key).copied().unwrap_or(0)))
            .sum::<usize>();
        let before = before.values().sum::<usize>();
        let after = after.values().sum::<usize>();
        Self {
            before,
            after,
            added: after - unchanged,
            resolved: before - unchanged,
            unchanged,
        }
    }
}

/// Full-board unapproved findings. Copper and missing connections are subsets
/// of errors; warnings are separate. Missing-connection identities describe the
/// rebuilt air-wire tree, not a persistent edge topology.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingFindingDeltas {
    pub errors: RoutingFindingDelta,
    pub non_connectivity_errors: RoutingFindingDelta,
    pub copper_errors: RoutingFindingDelta,
    pub missing_connections: RoutingFindingDelta,
    pub warnings: RoutingFindingDelta,
}

#[derive(Debug, Clone, Default)]
struct FindingCounts {
    errors: BTreeMap<String, usize>,
    non_connectivity_errors: BTreeMap<String, usize>,
    copper_errors: BTreeMap<String, usize>,
    missing_connections: BTreeMap<String, usize>,
    warnings: BTreeMap<String, usize>,
}

impl FindingCounts {
    fn record(&mut self, key: &str, severity: Severity, missing: bool, copper: bool) {
        let increment = |counts: &mut BTreeMap<String, usize>| {
            *counts.entry(key.to_owned()).or_default() += 1;
        };
        match severity {
            Severity::Error => {
                increment(&mut self.errors);
                if missing {
                    increment(&mut self.missing_connections);
                } else {
                    increment(&mut self.non_connectivity_errors);
                }
                if copper {
                    increment(&mut self.copper_errors);
                }
            }
            Severity::Warning => increment(&mut self.warnings),
            Severity::Hint => {}
        }
    }

    fn compare(&self, after: &Self) -> RoutingFindingDeltas {
        RoutingFindingDeltas {
            errors: RoutingFindingDelta::compare(&self.errors, &after.errors),
            non_connectivity_errors: RoutingFindingDelta::compare(
                &self.non_connectivity_errors,
                &after.non_connectivity_errors,
            ),
            copper_errors: RoutingFindingDelta::compare(&self.copper_errors, &after.copper_errors),
            missing_connections: RoutingFindingDelta::compare(
                &self.missing_connections,
                &after.missing_connections,
            ),
            warnings: RoutingFindingDelta::compare(&self.warnings, &after.warnings),
        }
    }
}

/// Status of the full-board unapproved error/warning check, independent of
/// whether a local change is acceptable. Hints and approved findings are excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingDrcStatus {
    Incomplete,
    ErrorsPresent,
    WarningsOnly,
    NoErrorsOrWarnings,
}

impl RoutingDrcStatus {
    fn from_checks(checks: &RoutingCheckSummary) -> Self {
        if !checks.ran || !checks.complete {
            Self::Incomplete
        } else if checks.errors > 0 {
            Self::ErrorsPresent
        } else if checks.warnings > 0 {
            Self::WarningsOnly
        } else {
            Self::NoErrorsOrWarnings
        }
    }
}

/// A non-connectivity DRC finding, identified independently of translated text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingDrcFinding {
    pub kind: String,
    pub message: String,
    pub approval: String,
    pub copper: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<RoutingObject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<Layer>,
}

/// Board objects with canonical identities in the native DRC approval.
pub fn drc_objects(message: &DrcMessage) -> Vec<RoutingObject> {
    use RoutingObjectKind as Kind;
    use foilfern_core::serialization::SExpression;
    let graphics = [
        ("polygon/@0", Kind::Polygon),
        ("circle/@0", Kind::Circle),
        ("stroke_text/@0", Kind::StrokeText),
    ];
    let parse = |object: &SExpression, kinds: &[(&str, Kind)]| {
        let (kind, uuid) = kinds
            .iter()
            .copied()
            .find_map(|(path, kind)| object.child_value(path).ok().map(|uuid| (kind, uuid)))?;
        Some(RoutingObject {
            kind,
            uuid,
            device_uuid: object.child_value("device/@0").ok(),
            pad_uuid: None,
        })
    };
    match message.kind() {
        DrcMessageKind::CopperCopperClearanceViolation => {
            let kinds = [
                ("trace/@0", Kind::Trace),
                ("via/@0", Kind::Via),
                ("pad/@0", Kind::Pad),
                graphics[0],
                graphics[1],
                graphics[2],
            ];
            message
                .approval()
                .children_named("object")
                .filter_map(|object| parse(object, &kinds))
                .collect()
        }
        DrcMessageKind::MinimumWidthViolation | DrcMessageKind::SilkscreenClearanceViolation => {
            parse(message.approval(), &graphics).into_iter().collect()
        }
        DrcMessageKind::UselessVia => parse(message.approval(), &[("via/@0", Kind::Via)])
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

pub type RoutingError = RoutingDrcFinding;

/// Captured state before or after a routing operation.
#[derive(Debug, Clone)]
pub struct RoutingSnapshot {
    pub checks: RoutingCheckSummary,
    requirements: State,
    finding_counts: FindingCounts,
    errors: BTreeMap<String, RoutingError>,
    warnings: BTreeMap<String, RoutingDrcFinding>,
    unrouted_by_net: BTreeMap<NetSignalId, usize>,
    unrouted_examples: BTreeMap<NetSignalId, (String, Vec<AirWire>)>,
    signals: BTreeMap<(ComponentInstanceId, Uuid), Option<NetSignalId>>,
    nets: BTreeSet<NetSignalId>,
}

const MAX_REGRESSION_NETS: usize = 20;
const MAX_REGRESSION_CONNECTIONS: usize = 8;

/// A sampled air wire after rebuilding the candidate's copper planes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingGap {
    pub from: TraceAnchor,
    pub to: TraceAnchor,
    pub from_mm: (f64, f64),
    pub to_mm: (f64, f64),
}

/// Name, counts and sampled locations for a regressed net.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingRegression {
    pub net: NetSignalId,
    pub name: String,
    pub before: usize,
    pub after: usize,
    pub air_wires: Vec<RoutingGap>,
}

/// Direction of the per-net connectivity changes. DRC is reported separately.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectivityProgress {
    Improved,
    #[default]
    Unchanged,
    Regressed,
}

/// A net with fewer open connections after rebuilding the board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingImprovement {
    pub net: NetSignalId,
    pub name: String,
    pub before: usize,
    pub after: usize,
}

impl RoutingSnapshot {
    pub(crate) fn non_connectivity_errors(&self) -> usize {
        self.checks.non_connectivity_errors
    }

    pub(crate) fn has_new_warning(&self, before: &Self, kind: DrcMessageKind) -> bool {
        let kind = format!("{kind:?}");
        self.warnings.iter().any(|(key, finding)| {
            finding.kind == kind
                && self.finding_counts.warnings.get(key) > before.finding_counts.warnings.get(key)
        })
    }

    pub(crate) fn unrouted_on(&self, nets: &BTreeSet<NetSignalId>) -> usize {
        nets.iter().map(|&net| self.unrouted_for(net)).sum()
    }

    pub(crate) fn unrouted_for(&self, net: NetSignalId) -> usize {
        self.unrouted_by_net.get(&net).copied().unwrap_or(0)
    }
}

/// Comparison of the applied result with the pre-routing board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingValidation {
    pub before: RoutingCheckSummary,
    pub after: RoutingCheckSummary,
    pub netlist_preserved: bool,
    /// All checks completed, the netlist stayed intact and no new
    /// non-connectivity errors appeared. This does not mean all nets are routed.
    pub acceptable: bool,
    /// All after-checks completed with no unapproved errors, including missing
    /// connections. Warnings may remain; inspect `drc_status`.
    pub drc_clean: bool,
    pub drc_status: RoutingDrcStatus,
    /// None if either check is incomplete: absence cannot prove resolution.
    pub finding_delta: Option<RoutingFindingDeltas>,
    /// Newly observed non-connectivity errors only; missing connections are in
    /// `finding_delta.missing_connections` and per-net connectivity progress.
    pub new_error_count: usize,
    pub new_copper_error_count: usize,
    /// A regression on any net takes precedence over gains elsewhere.
    #[serde(default)]
    pub connectivity_progress: ConnectivityProgress,
    /// First 20 nets whose rebuilt air-wire count decreased.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unrouted_improvements: Vec<RoutingImprovement>,
    /// Nets whose rebuilt air-wire count increased.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unrouted_regressions: Vec<NetSignalId>,
    /// First 20 regressed nets, with at most eight resulting air wires each.
    /// These are samples of the rebuilt tree, not an edge-by-edge diff.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unrouted_regression_details: Vec<RoutingRegression>,
    /// Resolved non-connectivity errors only; zero when either check is
    /// incomplete. See `finding_delta.errors` for all errors.
    pub resolved_error_count: usize,
    /// At most 50 newly reported errors (the count above is never truncated).
    pub new_errors: Vec<RoutingError>,
    /// Warnings inform review without blocking an otherwise acceptable patch.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub new_warning_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub new_warnings: Vec<RoutingDrcFinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub board_requirements_before: Vec<Finding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub board_requirements_after: Vec<Finding>,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

fn is_copper_error(kind: DrcMessageKind) -> bool {
    matches!(
        kind,
        DrcMessageKind::MinimumWidthViolation
            | DrcMessageKind::CopperCopperClearanceViolation
            | DrcMessageKind::CopperBoardClearanceViolation
            | DrcMessageKind::CopperHoleClearanceViolation
            | DrcMessageKind::CopperInKeepoutZone
            | DrcMessageKind::DrillDrillClearanceViolation
            | DrcMessageKind::MinimumAnnularRingViolation
            | DrcMessageKind::MinimumDrillDiameterViolation
            | DrcMessageKind::MinimumSlotWidthViolation
            | DrcMessageKind::InvalidPadConnection
            | DrcMessageKind::ForbiddenSlot
            | DrcMessageKind::ForbiddenVia
            | DrcMessageKind::InvalidVia
            | DrcMessageKind::UnconnectedJunction
            | DrcMessageKind::ImpossibleConnection
    )
}

/// Checks the actual project, rebuilding derived data without adding an undo
/// step. Check failures remain explicit evidence and never count as a pass.
pub fn capture_routing_state(
    editor: &mut ProjectEditor,
    board: BoardId,
) -> Result<RoutingSnapshot> {
    capture_routing_state_with_cancel(editor, board, &|| false)
}

pub(crate) fn capture_routing_state_with_cancel(
    editor: &mut ProjectEditor,
    board: BoardId,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<RoutingSnapshot> {
    capture_state(editor, board, cancelled, &BTreeSet::new())
}

/// Checks physical placement against the same DRC as routing. Exclude newly
/// placed unwired devices from the connectivity comparison only.
pub fn capture_placement_state(
    editor: &mut ProjectEditor,
    board: BoardId,
    new_devices: &BTreeSet<ComponentInstanceId>,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<RoutingSnapshot> {
    capture_state(editor, board, cancelled, new_devices)
}

fn capture_state(
    editor: &mut ProjectEditor,
    board: BoardId,
    cancelled: &(dyn Fn() -> bool + Sync),
    omitted_devices: &BTreeSet<ComponentInstanceId>,
) -> Result<RoutingSnapshot> {
    let result = editor
        .update_derived_data(|p| p.run_drc_with_cancel(board, None, false, &|_| {}, cancelled))?;
    let p = editor.project();
    let approvals = p.board(board).map(|b| b.drc_approvals());
    let mut checks = RoutingCheckSummary {
        ran: result.is_ok(),
        complete: false,
        unrouted: routing_stats(p, board).air_wires,
        errors: 0,
        non_connectivity_errors: 0,
        missing_connection_errors: 0,
        copper_errors: 0,
        warnings: 0,
        check_failures: Vec::new(),
    };
    let mut finding_counts = FindingCounts::default();
    let mut errors = BTreeMap::new();
    let mut warnings = BTreeMap::new();
    match result {
        Ok(result) => {
            checks.check_failures = result.errors;
            checks.complete = checks.check_failures.is_empty();
            for message in result.messages {
                if approvals.is_some_and(|a| a.contains(message.approval())) {
                    continue;
                }
                let copper = is_copper_error(message.kind());
                let missing = message.kind() == DrcMessageKind::MissingConnection;
                let approval = message.approval().to_string_with_mode(Default::default())?;
                finding_counts.record(&approval, message.message().severity(), missing, copper);
                let findings = match message.message().severity() {
                    Severity::Warning => {
                        checks.warnings += 1;
                        &mut warnings
                    }
                    Severity::Error => {
                        checks.errors += 1;
                        checks.copper_errors += usize::from(copper);
                        if missing {
                            checks.missing_connection_errors += 1;
                            continue;
                        }
                        checks.non_connectivity_errors += 1;
                        &mut errors
                    }
                    Severity::Hint => continue,
                };
                findings.insert(
                    approval.clone(),
                    RoutingDrcFinding {
                        kind: format!("{:?}", message.kind()),
                        message: message.message().message().to_owned(),
                        approval,
                        copper,
                        objects: drc_objects(&message),
                        layers: message.copper_layers().collect(),
                    },
                );
            }
        }
        Err(error) => checks.check_failures.push(error.to_string()),
    }
    let signals = p
        .circuit()
        .component_instances()
        .values()
        .flat_map(|component| {
            component
                .signals()
                .iter()
                .map(move |(signal, instance)| ((component.id(), *signal), instance.net()))
        })
        .collect();
    let existing_wires = p
        .board(board)
        .filter(|_| !omitted_devices.is_empty())
        .map(|board| {
            let builder = BoardAirWiresBuilder::excluding_unwired_devices(
                board,
                p.library(),
                p.circuit(),
                omitted_devices,
            );
            p.circuit()
                .net_signals()
                .keys()
                .map(|&net| (net, builder.build_air_wires(net)))
                .collect::<BTreeMap<_, _>>()
        });
    let air_wires: Vec<_> = match &existing_wires {
        Some(wires) => wires.iter().map(|(&net, wires)| (net, wires)).collect(),
        None => p
            .board(board)
            .into_iter()
            .flat_map(|b| b.derived().air_wires())
            .filter_map(|(net, wires)| net.map(|net| (net, wires)))
            .collect(),
    };
    let unrouted_by_net = air_wires
        .iter()
        .map(|(id, wires)| (*id, wires.len()))
        .collect();
    let unrouted_examples = air_wires
        .into_iter()
        .filter(|(_, wires)| !wires.is_empty())
        .filter_map(|(id, wires)| {
            let net = p.circuit().net_signals().get(&id)?;
            Some((
                id,
                (
                    net.name().as_str().to_owned(),
                    wires
                        .iter()
                        .take(MAX_REGRESSION_CONNECTIONS)
                        .cloned()
                        .collect(),
                ),
            ))
        })
        .collect();
    Ok(RoutingSnapshot {
        checks,
        finding_counts,
        requirements: State::capture(p, board, &Requirements::load(p)?)?,
        unrouted_by_net,
        unrouted_examples,
        errors,
        warnings,
        signals,
        nets: p.circuit().net_signals().keys().copied().collect(),
    })
}

impl RoutingValidation {
    pub fn compare(before: RoutingSnapshot, after: RoutingSnapshot) -> Self {
        Self::compare_snapshots(&before, &after)
    }

    /// Placement additionally rejects new courtyard conflicts.
    pub fn compare_placement(before: RoutingSnapshot, after: RoutingSnapshot) -> (Self, bool) {
        let mut result = Self::compare_candidate(&before, &after);
        let courtyard = after.has_new_warning(&before, DrcMessageKind::DeviceInCourtyard);
        result.acceptable &= !courtyard;
        (result, courtyard)
    }

    /// Reviewed candidates also require every net to keep its connectivity.
    pub(crate) fn compare_candidate(before: &RoutingSnapshot, after: &RoutingSnapshot) -> Self {
        let mut validation = Self::compare_snapshots(before, after);
        validation.acceptable &= validation.unrouted_regressions.is_empty();
        validation
    }

    fn compare_snapshots(before: &RoutingSnapshot, after: &RoutingSnapshot) -> Self {
        let new_errors: Vec<_> = after
            .errors
            .iter()
            .filter(|(key, _)| {
                after.finding_counts.non_connectivity_errors.get(*key)
                    > before.finding_counts.non_connectivity_errors.get(*key)
            })
            .map(|(_, error)| error.clone())
            .collect();
        let mut new_warnings = Vec::new();
        let deltas = before.finding_counts.compare(&after.finding_counts);
        let new_warning_count = deltas.warnings.added;
        for (key, warning) in &after.warnings {
            if after.finding_counts.warnings.get(key) > before.finding_counts.warnings.get(key)
                && new_warnings.len() < 50
            {
                new_warnings.push(warning.clone());
            }
        }
        let netlist_preserved = before.signals == after.signals && before.nets == after.nets;
        let unrouted_regressions: Vec<_> = after
            .unrouted_by_net
            .iter()
            .filter(|(net, count)| **count > before.unrouted_by_net.get(net).copied().unwrap_or(0))
            .map(|(net, _)| *net)
            .collect();
        let unrouted_regression_details = unrouted_regressions
            .iter()
            .take(MAX_REGRESSION_NETS)
            .filter_map(|id| {
                let (name, wires) = after.unrouted_examples.get(id)?;
                Some(RoutingRegression {
                    net: *id,
                    name: name.clone(),
                    before: before.unrouted_by_net.get(id).copied().unwrap_or(0),
                    after: after.unrouted_by_net[id],
                    air_wires: wires
                        .iter()
                        .map(|wire| RoutingGap {
                            from: wire.p1(),
                            to: wire.p2(),
                            from_mm: wire.p1_position().to_mm(),
                            to_mm: wire.p2_position().to_mm(),
                        })
                        .collect(),
                })
            })
            .collect();
        let unrouted_improvements: Vec<_> = before
            .unrouted_examples
            .iter()
            .filter_map(|(id, (name, _))| {
                let remaining = after.unrouted_by_net.get(id).copied().unwrap_or(0);
                let initial = before.unrouted_by_net[id];
                (remaining < initial).then(|| RoutingImprovement {
                    net: *id,
                    name: name.clone(),
                    before: initial,
                    after: remaining,
                })
            })
            .take(MAX_REGRESSION_NETS)
            .collect();
        let connectivity_progress = if !unrouted_regressions.is_empty() {
            ConnectivityProgress::Regressed
        } else if !unrouted_improvements.is_empty() {
            ConnectivityProgress::Improved
        } else {
            ConnectivityProgress::Unchanged
        };
        let board_requirements_after: Vec<_> = after
            .requirements
            .findings_after(&before.requirements)
            .collect();
        let complete = before.checks.ran
            && before.checks.complete
            && after.checks.ran
            && after.checks.complete;
        let new_error_count = deltas.non_connectivity_errors.added;
        let new_copper_error_count = deltas.copper_errors.added;
        let resolved_error_count = if complete {
            deltas.non_connectivity_errors.resolved
        } else {
            0
        };
        Self {
            acceptable: before.checks.complete
                && after.checks.complete
                && netlist_preserved
                && new_error_count == 0
                && !board_requirements_after.iter().any(Finding::blocks_routing),
            drc_clean: after.checks.ran && after.checks.complete && after.checks.errors == 0,
            drc_status: RoutingDrcStatus::from_checks(&after.checks),
            finding_delta: complete.then_some(deltas),
            netlist_preserved,
            new_error_count,
            new_copper_error_count,
            connectivity_progress,
            unrouted_improvements,
            unrouted_regressions,
            unrouted_regression_details,
            resolved_error_count,
            new_errors: new_errors.into_iter().take(50).collect(),
            new_warning_count,
            new_warnings,
            before: before.checks.clone(),
            after: after.checks.clone(),
            board_requirements_before: before.requirements.findings.clone(),
            board_requirements_after,
        }
    }

    /// Copper may already have committed when its final check fails. Invalidate
    /// comparison evidence rather than leaving the candidate's successful check.
    pub(crate) fn fail_after_check(&mut self, error: String) {
        self.acceptable = false;
        self.drc_clean = false;
        self.drc_status = RoutingDrcStatus::Incomplete;
        self.finding_delta = None;
        self.resolved_error_count = 0;
        self.after.ran = false;
        self.after.complete = false;
        self.after.check_failures.push(error);
    }

    /// Human-readable evidence for an operation response.
    pub fn summary(&self) -> String {
        format!(
            "Validation: {} -> {} unrouted; {} -> {} DRC errors ({} new non-connectivity); {} -> {} copper errors; {} -> {} warnings; DRC {:?} (unapproved errors/warnings); netlist {}; checks {}.",
            self.before.unrouted,
            self.after.unrouted,
            self.before.errors,
            self.after.errors,
            self.new_error_count,
            self.before.copper_errors,
            self.after.copper_errors,
            self.before.warnings,
            self.after.warnings,
            self.drc_status,
            if self.netlist_preserved {
                "preserved"
            } else {
                "CHANGED"
            },
            if self.before.complete && self.after.complete {
                "complete"
            } else {
                "INCOMPLETE"
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finding_multisets_preserve_duplicate_messages_and_category_identities() {
        let mut before = FindingCounts::default();
        let mut after = FindingCounts::default();
        before.record("missing-a", Severity::Error, true, false);
        before.record("clearance-a", Severity::Error, false, true);
        before.record("clearance-a", Severity::Error, false, true);
        before.record("warning-a", Severity::Warning, false, false);
        before.record("warning-b", Severity::Warning, false, false);
        after.record("clearance-a", Severity::Error, false, true);
        after.record("clearance-b", Severity::Error, false, true);
        after.record("warning-a", Severity::Warning, false, false);
        after.record("warning-a", Severity::Warning, false, false);
        let delta = before.compare(&after);
        assert_eq!(
            delta.errors,
            RoutingFindingDelta {
                before: 3,
                after: 2,
                added: 1,
                resolved: 2,
                unchanged: 1,
            }
        );
        assert_eq!(
            delta.missing_connections,
            RoutingFindingDelta {
                before: 1,
                after: 0,
                added: 0,
                resolved: 1,
                unchanged: 0,
            }
        );
        assert_eq!(delta.copper_errors, delta.non_connectivity_errors);
        assert_eq!(
            delta.copper_errors,
            RoutingFindingDelta {
                before: 2,
                after: 2,
                added: 1,
                resolved: 1,
                unchanged: 1,
            }
        );
        assert_eq!(
            delta.warnings,
            RoutingFindingDelta {
                before: 2,
                after: 2,
                added: 1,
                resolved: 1,
                unchanged: 1,
            }
        );
    }
}
