//! Explicit orthogonal paths for pins assigned to existing nets.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::{Junction, NetLabel, NetLine, NetLineAnchor};
use librepcb_core::project::schematic::{Schematic, SchematicNetSegment};
use librepcb_core::project::{
    Mutation, NetSegmentId, NetSegmentRef, Project, SchematicId, SchematicMutation,
};
use librepcb_core::types::{Angle, Length, Point, Uuid};
use librepcb_i18n::tr;

use super::{
    WireGroup, WireGroupPlan, is_frame, placed_symbol_rect, point_on_segment, segment_crosses_rect,
    segments_overlap,
};
use crate::{Command, Error, Result, Transaction};

/// A wire vertex is a pin index in the group or an absolute sheet coordinate.
pub enum WirePathPoint {
    Pin(usize),
    Waypoint(WireWaypoint),
}

/// An absolute axis coordinate or an offset from a pin on the same axis.
pub enum WireCoordinate {
    Absolute(Length),
    Relative { pin: usize, offset: Length },
}

pub struct WireWaypoint {
    pub x: WireCoordinate,
    pub y: WireCoordinate,
}

impl WireWaypoint {
    fn resolve(&self, pins: &[super::PlacedPin]) -> Result<Point> {
        let coordinate = |spec: &WireCoordinate, axis: fn(Point) -> Length| match *spec {
            WireCoordinate::Absolute(value) => Ok(value),
            WireCoordinate::Relative { pin, offset } => {
                let pin = pins.get(pin).ok_or_else(|| {
                    invalid("Relative coordinate refers to a pin outside its group.")
                })?;
                axis(pin.position)
                    .to_nm()
                    .checked_add(offset.to_nm())
                    .map(Length::new)
                    .ok_or_else(|| invalid("Relative wire coordinate is out of range."))
            }
        };
        Ok(Point::new(
            coordinate(&self.x, |p| p.x)?,
            coordinate(&self.y, |p| p.y)?,
        ))
    }
}

/// One connected group of paths and an optional net label on its wire.
pub struct WirePathSpec {
    pub group: WireGroup,
    pub paths: Vec<Vec<WirePathPoint>>,
    pub label: Option<WireWaypoint>,
}

/// Replaces only the segments belonging to each selected pin group.
pub struct DrawWirePaths {
    pub schematic: SchematicId,
    pub groups: Vec<WirePathSpec>,
}

type Edge = (Point, Point);

fn edge(a: Point, b: Point) -> Edge {
    (a.min(b), a.max(b))
}

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidArgument(message.into())
}

fn position(point: Point) -> String {
    format!("({}, {}) mm", point.x.to_mm(), point.y.to_mm())
}

/// Closed intersection, including endpoints and collinear overlaps. New edges
/// are orthogonal. Older schematic lines can be diagonal.
fn touches((a, b): Edge, (c, d): Edge) -> bool {
    let horizontal = a.y == b.y;
    let along = |p: Point| if horizontal { p.x } else { p.y };
    let across = |p: Point| if horizontal { p.y } else { p.x };
    let (lo, hi, fixed) = (along(a), along(b), across(a));
    let (u, v) = (across(c), across(d));
    if fixed < u.min(v) || fixed > u.max(v) {
        return false;
    }
    let (start, end) = (along(c), along(d));
    if u == v {
        return lo <= start.max(end) && hi >= start.min(end);
    }
    // Comparing weighted endpoint coordinates avoids division or rounding.
    let nm = |length: Length| i128::from(length.to_nm());
    let (u, v, start, end) = if u < v {
        (u, v, start, end)
    } else {
        (v, u, end, start)
    };
    let left = nm(v) - nm(fixed);
    let right = nm(fixed) - nm(u);
    let projected = left * nm(start) + right * nm(end);
    (left + right) * nm(lo) <= projected && projected <= (left + right) * nm(hi)
}

struct PlannedPath {
    segment: SchematicNetSegment,
    edges: BTreeSet<Edge>,
    replaced: BTreeSet<NetSegmentId>,
}

impl WirePathSpec {
    /// Segments this group may replace, before changing any symbol positions.
    pub fn replaceable_segments(&self, project: &Project) -> Result<BTreeSet<NetSegmentId>> {
        let plan = self.group.resolve_pins(project)?;
        self.group.check_replacement(project, &plan)?;
        Ok(plan.segments)
    }

    fn plan(self, project: &Project, sheet: &Schematic) -> Result<Option<PlannedPath>> {
        let plan = self.group.resolve_pins(project)?;
        let WireGroupPlan { pins, segments } = &plan;
        if pins.is_empty() {
            return Err(invalid("Explicit wire paths require at least one pin."));
        }
        self.group.check_replacement(project, &plan)?;

        let mut vertices = BTreeMap::new();
        for pin in pins {
            let anchor = NetLineAnchor::Pin {
                symbol: pin.symbol.0,
                pin: pin.pin,
            };
            if vertices.insert(pin.position, anchor).is_some() {
                return Err(invalid(format!(
                    "Multiple selected pins coincide at {}.",
                    position(pin.position)
                )));
            }
        }
        let mut used = BTreeSet::new();
        let mut paths = Vec::new();
        let limit = Length::new(10_000_000_000);
        for path in self.paths {
            if path.len() < 2 {
                return Err(invalid("Each wire path requires at least two vertices."));
            }
            let mut points = Vec::new();
            for vertex in path {
                let point = match vertex {
                    WirePathPoint::Pin(index) => {
                        used.insert(index);
                        pins.get(index)
                            .ok_or_else(|| invalid("Wire path refers to a pin outside its group."))?
                            .position
                    }
                    WirePathPoint::Waypoint(waypoint) => {
                        let point = waypoint.resolve(pins)?;
                        if vertices
                            .get(&point)
                            .is_some_and(|a| matches!(a, NetLineAnchor::Pin { .. }))
                        {
                            return Err(invalid(format!(
                                "Use the pin name at {}.",
                                position(point)
                            )));
                        }
                        vertices
                            .entry(point)
                            .or_insert_with(|| NetLineAnchor::Junction(Uuid::new_random()));
                        point
                    }
                };
                if [point.x, point.y]
                    .into_iter()
                    .any(|v| v < -limit || v > limit)
                {
                    return Err(invalid(
                        "Wire coordinates must be within 10 m of the sheet origin.",
                    ));
                }
                points.push(point);
            }
            paths.push(points);
        }
        if used.len() != pins.len() {
            return Err(invalid("Every selected pin must occur in a wire path."));
        }

        let mut edges = BTreeSet::new();
        for (a, b) in paths
            .iter()
            .flat_map(|path| path.iter().copied().zip(path.iter().copied().skip(1)))
        {
            if a == b || (a.x != b.x && a.y != b.y) {
                return Err(invalid(format!(
                    "Wire from {} to {} must be horizontal or vertical and nonzero.",
                    position(a),
                    position(b)
                )));
            }
            // A branch endpoint on a longer wire splits that wire at the
            // endpoint, giving both paths the same junction anchor.
            let mut points = vertices
                .range(a.min(b)..=a.max(b))
                .filter(|(point, _)| point_on_segment(**point, a, b));
            let (&first, _) = points.next().expect("path endpoints are vertices");
            let mut previous = first;
            for (&next, _) in points {
                let new = edge(previous, next);
                if edges.iter().any(|&old: &Edge| {
                    segments_overlap(old, new)
                        || (touches(old, new)
                            && ![old.0, old.1].iter().any(|p| *p == new.0 || *p == new.1))
                }) {
                    return Err(invalid(format!(
                        "Wire paths overlap or cross near {}. Use a shared junction for a branch.",
                        position(previous)
                    )));
                }
                edges.insert(new);
                previous = next;
            }
        }

        let mut segment = SchematicNetSegment::new(Uuid::new_random(), self.group.net);
        for (&point, &anchor) in &vertices {
            if let NetLineAnchor::Junction(id) = anchor {
                segment.insert_junction(Junction::new(id, point));
            }
        }
        for &(a, b) in &edges {
            segment.insert_line(NetLine::new(
                Uuid::new_random(),
                SchematicNetSegment::default_line_width(),
                vertices[&a],
                vertices[&b],
            ));
        }
        if !segment.is_cohesive() {
            return Err(invalid(
                "Wire paths must form one connected group. Use separate groups for separate segments of the same net.",
            ));
        }
        if let Some(label) = self.label {
            let point = label.resolve(pins)?;
            let horizontal = edges
                .iter()
                .any(|&(a, b)| a.y == b.y && point_on_segment(point, a, b));
            let vertical = edges
                .iter()
                .any(|&(a, b)| a.x == b.x && point_on_segment(point, a, b));
            if !horizontal && !vertical {
                return Err(invalid(format!(
                    "Place the net label on a wire at {}.",
                    position(point)
                )));
            }
            let angle = if horizontal {
                Angle::DEG0
            } else {
                Angle::DEG90
            };
            segment.insert_label(NetLabel::new(Uuid::new_random(), point, angle, false));
        }

        if segments.len() == 1 {
            let old = &sheet.net_segments()[segments.first().expect("one segment")];
            let old_edges = old
                .lines()
                .values()
                .filter_map(|line| {
                    let a = sheet.net_line_anchor_position(old.id(), line.p1(), project.view())?;
                    let b = sheet.net_line_anchor_position(old.id(), line.p2(), project.view())?;
                    let matches = |point, anchor| match (vertices.get(&point), anchor) {
                        (Some(NetLineAnchor::Junction(_)), NetLineAnchor::Junction(_)) => true,
                        (Some(expected), actual) => *expected == actual,
                        _ => false,
                    };
                    (matches(a, line.p1()) && matches(b, line.p2())).then(|| edge(a, b))
                })
                .collect::<BTreeSet<_>>();
            let same_labels = old.labels().len() == segment.labels().len()
                && old
                    .labels()
                    .values()
                    .zip(segment.labels().values())
                    .all(|(a, b)| {
                        a.position() == b.position()
                            && a.rotation() == b.rotation()
                            && a.mirrored() == b.mirrored()
                    });
            if old_edges == edges
                && old.lines().len() == edges.len()
                && old.junctions().len() == segment.junctions().len()
                && old.connected_pins() == segment.connected_pins()
                && same_labels
            {
                return Ok(None);
            }
        }
        Ok(Some(PlannedPath {
            segment,
            edges,
            replaced: segments.clone(),
        }))
    }
}

impl Command for DrawWirePaths {
    /// Replaced segment IDs per group, or None when the existing geometry matches.
    type Output = Vec<Option<BTreeSet<NetSegmentId>>>;

    fn text(&self) -> String {
        tr!("CmdSchematicNetSegmentAdd", "Draw wires")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Self::Output> {
        let project = tx.project();
        let sheet = project
            .schematic(self.schematic)
            .ok_or_else(|| Error::not_found("Schematic", self.schematic))?;
        let plans = self
            .groups
            .into_iter()
            .map(|group| {
                if group.group.schematic != self.schematic {
                    return Err(invalid("All wire paths must be on the selected sheet."));
                }
                group.plan(project, sheet)
            })
            .collect::<Result<Vec<_>>>()?;
        if plans.iter().all(Option::is_none) {
            return Ok(plans.into_iter().map(|_| None).collect());
        }
        let mut removed = BTreeSet::new();
        for plan in plans.iter().flatten() {
            for id in &plan.replaced {
                if !removed.insert(*id) {
                    return Err(invalid(format!(
                        "Multiple wire groups would replace segment {id}."
                    )));
                }
            }
        }

        let net_name = |id| {
            project
                .circuit()
                .net_signal(id)
                .map_or("?", |net| net.name().as_str())
        };
        let mut obstacles = Vec::new();
        for (id, segment) in sheet.net_segments() {
            if removed.contains(id) {
                continue;
            }
            for line in segment.lines().values() {
                if let [Some(a), Some(b)] = [line.p1(), line.p2()]
                    .map(|anchor| sheet.net_line_anchor_position(*id, anchor, project.view()))
                {
                    obstacles.push((edge(a, b), net_name(segment.net())));
                }
            }
        }
        for bus in sheet.bus_segments().values() {
            for line in bus.lines().values() {
                if let [Some(a), Some(b)] =
                    [line.p1(), line.p2()].map(|anchor| bus.junction_position(anchor))
                {
                    obstacles.push((edge(a, b), "bus"));
                }
            }
        }

        let mut bodies = Vec::new();
        let mut pins = Vec::new();
        for (id, symbol) in sheet.symbols() {
            let name = super::resolve::component_name(project, symbol.component());
            if !is_frame(project, self.schematic, *id)? {
                bodies.push((
                    name.clone(),
                    placed_symbol_rect(project, self.schematic, *id, false)?,
                ));
            }
            for pin in symbol.pins(project.view())? {
                pins.push((
                    (*id, pin.uuid()),
                    pin.position(),
                    format!("{name}.{}", pin.name()),
                ));
            }
        }
        for plan in plans.iter().flatten() {
            let name = net_name(plan.segment.net());
            let own_pins = plan.segment.connected_pins();
            for &(a, b) in &plan.edges {
                let describe = || format!("{name} wire from {} to {}", position(a), position(b));
                if let Some((symbol, _)) = bodies
                    .iter()
                    .find(|(_, rect)| segment_crosses_rect(a, b, rect))
                {
                    return Err(invalid(format!(
                        "{} passes through symbol {symbol}.",
                        describe()
                    )));
                }
                if let Some((_, _, pin)) = pins.iter().find(|(anchor, point, _)| {
                    !own_pins.contains(anchor) && point_on_segment(*point, a, b)
                }) {
                    return Err(invalid(format!(
                        "{} touches unrelated pin {pin}.",
                        describe()
                    )));
                }
                if let Some((_, other)) = obstacles.iter().find(|(old, _)| touches((a, b), *old)) {
                    return Err(invalid(format!(
                        "{} touches, crosses or overlaps existing wire on {other}.",
                        describe()
                    )));
                }
            }
            obstacles.extend(plan.edges.iter().map(|&edge| (edge, name)));
        }

        let mut changes = Vec::with_capacity(plans.len());
        tx.apply_all(removed.into_iter().map(|segment| {
            Mutation::Schematic(SchematicMutation::RemoveNetSegment(NetSegmentRef {
                schematic: self.schematic,
                segment,
            }))
        }))?;
        for plan in plans {
            if let Some(plan) = plan {
                tx.apply(Mutation::Schematic(SchematicMutation::AddNetSegment {
                    schematic: self.schematic,
                    segment: plan.segment,
                }))?;
                changes.push(Some(plan.replaced));
            } else {
                changes.push(None);
            }
        }
        Ok(changes)
    }
}
