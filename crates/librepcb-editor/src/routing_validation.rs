//! Validation of routing against the applied project model (no upstream
//! counterpart). Shared by routing callers, independent of the MCP protocol.
//!
//! Each snapshot runs full DRC, including rebuilt planes and air wires.
//! Finding identities use the upstream approval expressions; missing connections
//! are tracked separately because their air-wire tree can change after routing.
//! The netlist comparison checks signal assignments, not physical connectivity;
//! shorts and copper geometry are checked by DRC.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::TraceAnchor;
use librepcb_core::project::board::AirWire;
use librepcb_core::project::board::drc::DrcMessageKind;
use librepcb_core::project::board_requirements::Requirements;
use librepcb_core::project::{BoardId, ComponentInstanceId, NetSignalId};
use librepcb_core::rule_check::Severity;
use librepcb_core::types::Uuid;
use serde::{Deserialize, Serialize};

use crate::board_requirements::{Finding, State};
use crate::{ProjectEditor, Result, routing_stats};

/// A full DRC snapshot of the actual board. Counts exclude approved messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingCheckSummary {
    pub ran: bool,
    pub complete: bool,
    pub unrouted: usize,
    pub errors: usize,
    pub copper_errors: usize,
    pub warnings: usize,
    pub check_failures: Vec<String>,
}

/// A non-connectivity DRC finding, identified independently of translated text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingDrcFinding {
    pub kind: String,
    pub message: String,
    pub approval: String,
    pub copper: bool,
}

pub type RoutingError = RoutingDrcFinding;

/// Captured state before or after a routing operation.
#[derive(Debug, Clone)]
pub struct RoutingSnapshot {
    pub checks: RoutingCheckSummary,
    requirements: State,
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
        self.errors.len()
    }

    pub(crate) fn has_new_warning(&self, before: &Self, kind: DrcMessageKind) -> bool {
        let kind = format!("{kind:?}");
        self.warnings
            .iter()
            .any(|(key, finding)| finding.kind == kind && !before.warnings.contains_key(key))
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
    pub drc_clean: bool,
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
    let result = editor
        .update_derived_data(|p| p.run_drc_with_cancel(board, None, false, &|_| {}, cancelled))?;
    let p = editor.project();
    let approvals = p.board(board).map(|b| b.drc_approvals());
    let mut checks = RoutingCheckSummary {
        ran: result.is_ok(),
        complete: false,
        unrouted: routing_stats(p, board).air_wires,
        errors: 0,
        copper_errors: 0,
        warnings: 0,
        check_failures: Vec::new(),
    };
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
                let findings = match message.message().severity() {
                    Severity::Warning => {
                        checks.warnings += 1;
                        &mut warnings
                    }
                    Severity::Error => {
                        checks.errors += 1;
                        checks.copper_errors += usize::from(copper);
                        if message.kind() == DrcMessageKind::MissingConnection {
                            continue;
                        }
                        &mut errors
                    }
                    Severity::Hint => continue,
                };
                let approval = message.approval().to_string_with_mode(Default::default())?;
                findings.insert(
                    approval.clone(),
                    RoutingDrcFinding {
                        kind: format!("{:?}", message.kind()),
                        message: message.message().message().to_owned(),
                        approval,
                        copper,
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
    let air_wires: Vec<_> = p
        .board(board)
        .into_iter()
        .flat_map(|b| b.derived().air_wires())
        .filter_map(|(net, wires)| net.map(|net| (net, wires)))
        .collect();
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
            .filter(|(key, _)| !before.errors.contains_key(*key))
            .map(|(_, error)| error.clone())
            .collect();
        let mut new_warnings = Vec::new();
        let mut new_warning_count = 0;
        for (key, warning) in &after.warnings {
            if !before.warnings.contains_key(key) {
                new_warning_count += 1;
                if new_warnings.len() < 50 {
                    new_warnings.push(warning.clone());
                }
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
        Self {
            acceptable: before.checks.complete
                && after.checks.complete
                && netlist_preserved
                && new_errors.is_empty()
                && !board_requirements_after.iter().any(Finding::blocks_routing),
            drc_clean: after.checks.complete && after.checks.errors == 0,
            netlist_preserved,
            new_error_count: new_errors.len(),
            new_copper_error_count: new_errors.iter().filter(|e| e.copper).count(),
            connectivity_progress,
            unrouted_improvements,
            unrouted_regressions,
            unrouted_regression_details,
            resolved_error_count: before
                .errors
                .keys()
                .filter(|key| !after.errors.contains_key(*key))
                .count(),
            new_errors: new_errors.into_iter().take(50).collect(),
            new_warning_count,
            new_warnings,
            before: before.checks.clone(),
            after: after.checks.clone(),
            board_requirements_before: before.requirements.findings.clone(),
            board_requirements_after,
        }
    }

    /// Human-readable evidence for an operation response.
    pub fn summary(&self) -> String {
        format!(
            "Validation: {} -> {} unrouted; {} -> {} DRC errors ({} new); {} -> {} copper errors; netlist {}; checks {}.",
            self.before.unrouted,
            self.after.unrouted,
            self.before.errors,
            self.after.errors,
            self.new_error_count,
            self.before.copper_errors,
            self.after.copper_errors,
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
