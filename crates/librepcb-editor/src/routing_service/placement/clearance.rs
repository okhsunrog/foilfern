//! Native staged-copper evidence and bounded translations derived from it.

use std::collections::BTreeMap;

use librepcb_core::project::board::Board;
use librepcb_core::project::board::drc::{DrcMessage, DrcMessageKind};
use librepcb_core::project::{BoardId, ComponentInstanceId, NetSegmentId, Project};
use librepcb_core::rule_check::Severity;
use librepcb_core::serialization::SExpression;
use librepcb_core::types::{Point, Uuid};
use serde::Serialize;

use super::{PlacementSites, PlacementTarget, PlacementVia, Pose};
use crate::commands::resolve_board;
use crate::{Error, ProjectEditor, Result};

#[derive(Debug, Clone, Serialize)]
pub struct ClearanceObject {
    kind: &'static str,
    uuid: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    component: Option<ComponentInstanceId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    segment: Option<NetSegmentId>,
    /// Index in copper.vias for a new via whose UUID only exists in this trial.
    #[serde(skip_serializing_if = "Option::is_none")]
    planned_via: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlacementClearanceConflict {
    kind: String,
    message: String,
    objects: Vec<ClearanceObject>,
    layers: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location_bounds_mm: Option<[(f64, f64); 2]>,
}

impl PlacementClearanceConflict {
    fn from_message(message: &DrcMessage, vias: &[Uuid]) -> Self {
        let approval = message.approval();
        let mut objects: Vec<_> = approval
            .children_named("object")
            .flat_map(|node| clearance_objects(node, vias))
            .collect();
        if objects.is_empty() {
            objects.extend(clearance_objects(approval, vias));
        }
        objects.sort_by_key(ClearanceObject::stable_identity);
        let mut vertices = message
            .message()
            .locations()
            .iter()
            .flat_map(|path| path.vertices().iter().map(|vertex| vertex.pos));
        let bounds = vertices.next().map(|first| {
            vertices
                .fold([first, first], |[min, max], point| {
                    [
                        Point::new(min.x.min(point.x), min.y.min(point.y)),
                        Point::new(max.x.max(point.x), max.y.max(point.y)),
                    ]
                })
                .map(Point::to_mm)
        });
        Self {
            kind: format!("{:?}", message.kind()),
            message: message.message().message().to_owned(),
            objects,
            layers: message.copper_layers().map(|layer| layer.id()).collect(),
            location_bounds_mm: bounds,
        }
    }

    /// Order the bounded response and subsequent trial selection by stable
    /// identities. New vias and merged segments can get random UUIDs.
    pub(super) fn introduced(
        before: &BTreeMap<SExpression, DrcMessage>,
        after: BTreeMap<SExpression, DrcMessage>,
        vias: &[Uuid],
    ) -> Vec<Self> {
        let mut conflicts: Vec<_> = after
            .into_iter()
            .filter(|(key, _)| !before.contains_key(key))
            .map(|(_, message)| Self::from_message(&message, vias))
            .collect();
        conflicts.sort_by_cached_key(|conflict| {
            (
                conflict.kind.clone(),
                conflict
                    .objects
                    .iter()
                    .map(ClearanceObject::stable_identity)
                    .collect::<Vec<_>>(),
            )
        });
        conflicts.truncate(8);
        conflicts
    }

    pub(super) fn message(&self) -> &str {
        &self.message
    }

    pub(super) fn neighbors(
        &self,
        project: &Project,
        original: &Project,
        board_id: BoardId,
        current: &[Pose],
        targets: &[PlacementTarget],
        via_rules: &[PlacementVia],
    ) -> Vec<Vec<Pose>> {
        let (Some(board), Some(original_board)) =
            (project.board(board_id), original.board(board_id))
        else {
            return Vec::new();
        };
        // The pair identifies the moving shape and exactly one fixed shape.
        // Board/hole clearances and conflicts between two moving targets
        // remain evidence, since this translation has no fixed copper axis.
        let [first, second] = self.objects.as_slice() else {
            return Vec::new();
        };
        let moving_component = |object: &ClearanceObject| {
            object
                .component
                .or_else(|| {
                    let index = object.planned_via?;
                    via_rules.iter().find(|rule| rule.via == index)?.component
                })
                .filter(|id| current.iter().any(|pose| pose.component == *id))
        };
        let (moving, fixed, component) = match (moving_component(first), moving_component(second)) {
            (Some(component), None) => (first, second, component),
            (None, Some(component)) => (second, first, component),
            _ => return Vec::new(),
        };
        // New explicit traces may change when a device moves. Only steer
        // against a known fixed copper object in the native source board.
        let Some(PlacementSites::Grid { bounds, pitch }) = targets
            .iter()
            .find(|target| target.component == component)
            .map(|target| &target.sites)
        else {
            return Vec::new();
        };
        let Some(index) = current.iter().position(|pose| pose.component == component) else {
            return Vec::new();
        };
        let Some([start, _]) = moving.line(project, board) else {
            return Vec::new();
        };
        let Some([a, b]) = fixed.line(original, original_board) else {
            return Vec::new();
        };
        let (x, y) = start.to_mm();
        let (ax, ay) = a.to_mm();
        let (bx, by) = b.to_mm();
        let (dx, dy) = (bx - ax, by - ay);
        let length_sq = dx * dx + dy * dy;
        let along = if length_sq > 0.0 {
            (((x - ax) * dx + (y - ay) * dy) / length_sq).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let delta = (x - ax - along * dx, y - ay - along * dy);
        let distance = delta.0.hypot(delta.1);
        let directions = if distance > 1e-9 {
            vec![(delta.0 / distance, delta.1 / distance)]
        } else if length_sq > 0.0 {
            let length = length_sq.sqrt();
            vec![(-dy / length, dx / length), (dy / length, -dx / length)]
        } else {
            return Vec::new();
        };
        let (cx, cy) = current[index].position.to_mm();
        let step = pitch.to_mm();
        let mut result = Vec::new();
        for (nx, ny) in directions {
            for (away, side) in [(0.5, 0.0), (1.0, 0.0), (0.5, 0.5), (0.5, -0.5), (2.0, 0.0)] {
                let Ok(position) = Point::from_mm(
                    cx + step * (away * nx - side * ny),
                    cy + step * (away * ny + side * nx),
                ) else {
                    continue;
                };
                if position.x >= bounds.min.x
                    && position.x <= bounds.max.x
                    && position.y >= bounds.min.y
                    && position.y <= bounds.max.y
                {
                    let mut poses = current.to_vec();
                    poses[index].position = position;
                    result.push(poses);
                }
            }
        }
        result
    }
}

fn clearance_objects<'a>(
    node: &'a SExpression,
    vias: &'a [Uuid],
) -> impl Iterator<Item = ClearanceObject> + 'a {
    let component = node.child_value("device/@0").ok().map(ComponentInstanceId);
    let segment = node.child_value("netsegment/@0").ok().map(NetSegmentId);
    node.children().iter().filter_map(move |child| {
        let kind = match child.as_list()?.name() {
            "pad" => "pad",
            "trace" => "trace",
            "via" => "via",
            "plane" => "plane",
            "hole" => "hole",
            "polygon" => "polygon",
            "circle" => "circle",
            "stroke_text" => "stroke_text",
            _ => return None,
        };
        let uuid = child.child_value::<Uuid>("@0").ok()?;
        Some(ClearanceObject {
            kind,
            uuid,
            component,
            segment,
            planned_via: (kind == "via")
                .then(|| vias.iter().position(|&via| via == uuid))
                .flatten(),
        })
    })
}

impl ClearanceObject {
    fn stable_identity(
        &self,
    ) -> (
        &'static str,
        Option<ComponentInstanceId>,
        Option<usize>,
        Option<Uuid>,
    ) {
        (
            self.kind,
            self.component,
            self.planned_via,
            self.planned_via.is_none().then_some(self.uuid),
        )
    }

    fn line(&self, project: &Project, board: &Board) -> Option<[Point; 2]> {
        match self.kind {
            "pad" => {
                let pad = board
                    .device(self.component?)?
                    .pad(&self.uuid, project.library(), project.circuit())
                    .ok()??;
                Some([pad.position(); 2])
            }
            "via" | "trace" => {
                // Later staging commands may merge the original segment.
                let geometry = |segment: &librepcb_core::project::board::BoardNetSegment| {
                    if self.kind == "via" {
                        return Some([segment.vias().get(&self.uuid)?.position(); 2]);
                    }
                    let trace = segment.traces().get(&self.uuid)?;
                    let point = |anchor| {
                        board.anchor_position(segment, anchor, project.library(), project.circuit())
                    };
                    Some([point(trace.p1())?, point(trace.p2())?])
                };
                self.segment
                    .and_then(|id| board.net_segment(id))
                    .and_then(geometry)
                    .or_else(|| board.net_segments().values().find_map(geometry))
            }
            _ => None,
        }
    }
}

/// Quick DRC on protected and staged copper with the real board clearances.
pub(super) fn clearance_errors(
    editor: &mut ProjectEditor,
    board: BoardId,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Option<BTreeMap<SExpression, DrcMessage>>> {
    let check = editor
        .update_derived_data(|p| p.run_drc_with_cancel(board, None, true, &|_| {}, cancelled))?
        .map_err(|error| Error::InvalidArgument(error.to_string()))?;
    let approvals = resolve_board(editor.project(), Some(board))?.drc_approvals();
    Ok(check.errors.is_empty().then(|| {
        check
            .messages
            .into_iter()
            .filter(|message| {
                message.message().severity() == Severity::Error
                    && matches!(
                        message.kind(),
                        DrcMessageKind::CopperCopperClearanceViolation
                            | DrcMessageKind::CopperBoardClearanceViolation
                            | DrcMessageKind::CopperHoleClearanceViolation
                    )
                    && !approvals.contains(message.approval())
            })
            .map(|message| (message.approval().clone(), message))
            .collect()
    }))
}
