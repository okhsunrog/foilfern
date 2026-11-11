//! Validation of routing against the applied project model (no upstream
//! counterpart). Shared by routing callers, independent of the MCP protocol.
//!
//! Each snapshot runs full DRC, including rebuilt planes and air wires.
//! Error identities use the upstream approval expressions; missing connections
//! are tracked separately because their air-wire tree can change after routing.
//! The netlist comparison checks signal assignments, not physical connectivity;
//! shorts and copper geometry are checked by DRC.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::project::board::drc::DrcMessageKind;
use librepcb_core::project::{BoardId, ComponentInstanceId, NetSignalId};
use librepcb_core::rule_check::Severity;
use librepcb_core::types::Uuid;
use serde::{Deserialize, Serialize};

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

/// A non-connectivity DRC error, identified independently of translated text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingError {
    pub kind: String,
    pub message: String,
    pub approval: String,
    pub copper: bool,
}

/// Captured state before or after a routing operation.
#[derive(Debug, Clone)]
pub struct RoutingSnapshot {
    pub checks: RoutingCheckSummary,
    errors: BTreeMap<String, RoutingError>,
    signals: BTreeMap<(ComponentInstanceId, Uuid), Option<NetSignalId>>,
    nets: BTreeSet<NetSignalId>,
}

/// Comparison of the applied result with the pre-routing board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub resolved_error_count: usize,
    /// At most 50 newly reported errors (the count above is never truncated).
    pub new_errors: Vec<RoutingError>,
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
    let result = editor.update_derived_data(|p| p.run_drc(board, None, false, &|_| {}))?;
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
    match result {
        Ok(result) => {
            checks.check_failures = result.errors;
            checks.complete = checks.check_failures.is_empty();
            for message in result.messages {
                if approvals.is_some_and(|a| a.contains(message.approval())) {
                    continue;
                }
                match message.message().severity() {
                    Severity::Warning => checks.warnings += 1,
                    Severity::Error => {
                        checks.errors += 1;
                        let copper = is_copper_error(message.kind());
                        checks.copper_errors += usize::from(copper);
                        if message.kind() != DrcMessageKind::MissingConnection {
                            let approval =
                                message.approval().to_string_with_mode(Default::default())?;
                            errors.insert(
                                approval.clone(),
                                RoutingError {
                                    kind: format!("{:?}", message.kind()),
                                    message: message.message().message().to_owned(),
                                    approval,
                                    copper,
                                },
                            );
                        }
                    }
                    Severity::Hint => {}
                }
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
    Ok(RoutingSnapshot {
        checks,
        errors,
        signals,
        nets: p.circuit().net_signals().keys().copied().collect(),
    })
}

impl RoutingValidation {
    pub fn compare(before: RoutingSnapshot, after: RoutingSnapshot) -> Self {
        let new_errors: Vec<_> = after
            .errors
            .iter()
            .filter(|(key, _)| !before.errors.contains_key(*key))
            .map(|(_, error)| error.clone())
            .collect();
        let netlist_preserved = before.signals == after.signals && before.nets == after.nets;
        Self {
            acceptable: before.checks.complete
                && after.checks.complete
                && netlist_preserved
                && new_errors.is_empty(),
            drc_clean: after.checks.complete && after.checks.errors == 0,
            netlist_preserved,
            new_error_count: new_errors.len(),
            new_copper_error_count: new_errors.iter().filter(|e| e.copper).count(),
            resolved_error_count: before
                .errors
                .keys()
                .filter(|key| !after.errors.contains_key(*key))
                .count(),
            new_errors: new_errors.into_iter().take(50).collect(),
            before: before.checks,
            after: after.checks,
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
