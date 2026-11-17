//! Layout quality measurements of a schematic page (original code, no
//! upstream counterpart).
//!
//! The counters describe readability properties which structural ERC does
//! not cover: symbol bodies drawn over each other, wires of different nets
//! that run along each other or end on each other (both read as a
//! connection), wire crossings, bends, wire length, nets split into
//! label-joined segments, unwired pins, off-grid anchors and how much of the
//! drawn area the symbols use. They are measurements for comparing layouts,
//! not acceptance thresholds.

use std::collections::{BTreeMap, BTreeSet};

use foilfern_canvas::kurbo::Rect;
use foilfern_core::geometry::NetLineAnchor;
use foilfern_core::project::schematic::Schematic;
use foilfern_core::project::{NetSegmentId, NetSignalId, Project, SymbolId};
use foilfern_core::types::{Point, Uuid};

use crate::{Error, Result, SchematicObject, SchematicScene};

/// Two symbols whose drawn bodies (shapes and pins) intersect.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolOverlap {
    pub first: SymbolId,
    pub second: SymbolId,
    /// Intersection in schematic coordinates (mm): x0, y0, x1, y1.
    pub bounds: [f64; 4],
}

/// Two wires of different nets which share a collinear stretch.
#[derive(Debug, Clone, PartialEq)]
pub struct WireOverlap {
    pub first: (NetSegmentId, Uuid),
    pub second: (NetSegmentId, Uuid),
    /// Shared stretch in schematic coordinates (mm).
    pub from: (f64, f64),
    pub to: (f64, f64),
}

/// A wire end lying on the interior of a wire of a different net, which a
/// reader takes for a connection.
#[derive(Debug, Clone, PartialEq)]
pub struct ForeignWireContact {
    /// The wire whose end touches the other one.
    pub wire: (NetSegmentId, Uuid),
    /// The touched wire.
    pub touched: (NetSegmentId, Uuid),
    /// Contact position (mm).
    pub position: (f64, f64),
}

/// Wire geometry counters of a page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WireMetrics {
    pub lines: usize,
    pub total_length_mm: f64,
    pub longest_mm: f64,
    /// Lines that are neither horizontal nor vertical.
    pub diagonal: usize,
    /// Junctions joining exactly two non-collinear lines.
    pub bends: usize,
    /// Proper crossings of lines of different net segments.
    pub crossings: usize,
    pub different_net_overlaps: Vec<WireOverlap>,
    pub foreign_contacts: Vec<ForeignWireContact>,
}

/// Net counters of a page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetMetrics {
    /// Nets with at least one segment on the page.
    pub nets: usize,
    pub segments: usize,
    pub labels: usize,
    /// Nets drawn as two or more segments on the page (joined by labels).
    pub split_nets: usize,
    /// Sum over nets of the half perimeter of the bounding box of their
    /// pins on this page (mm). Lower means connected pins sit closer.
    pub pin_span_mm: f64,
    /// Symbol pins that belong to a net but have no wire on the page.
    pub unwired_pins: usize,
}

/// Layout measurements of one schematic page.
#[derive(Debug, Clone, PartialEq)]
pub struct SchematicMetrics {
    pub symbols: usize,
    /// Bounding box of all visible items (mm), `None` for an empty page.
    pub content_bounds: Option<[f64; 4]>,
    /// Sum of symbol body areas divided by the content area (0 when empty).
    pub symbol_fill: f64,
    pub symbol_overlaps: Vec<SymbolOverlap>,
    pub wires: WireMetrics,
    pub nets: NetMetrics,
    pub off_grid: OffGrid,
}

/// Anchors which are not on the page grid.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OffGrid {
    /// Symbol origins.
    pub symbols: usize,
    /// Net segment junctions.
    pub junctions: usize,
    /// Net label anchors.
    pub labels: usize,
}

impl SchematicMetrics {
    /// Number of findings which make the drawing misleading or unreadable
    /// (overlapping bodies, wires of different nets drawn as one, contacts
    /// with foreign wires); crossings, bends and spread are not counted.
    pub fn defects(&self) -> usize {
        self.symbol_overlaps.len()
            + self.wires.different_net_overlaps.len()
            + self.wires.foreign_contacts.len()
    }
}

/// Smallest drawn overlap counted, to ignore touching outlines (mm).
const OVERLAP_EPS_MM: f64 = 0.1;

#[derive(Debug, Clone, Copy)]
struct Line {
    segment: NetSegmentId,
    uuid: Uuid,
    net: NetSignalId,
    a: Point,
    b: Point,
}

impl Line {
    fn horizontal(&self) -> bool {
        self.a.y == self.b.y
    }
    fn vertical(&self) -> bool {
        self.a.x == self.b.x
    }
    fn id(&self) -> (NetSegmentId, Uuid) {
        (self.segment, self.uuid)
    }
    fn length_mm(&self) -> f64 {
        let (ax, ay) = self.a.to_mm();
        let (bx, by) = self.b.to_mm();
        (bx - ax).hypot(by - ay)
    }
    /// Whether `p` lies strictly inside the line (not at an end).
    fn interior_contains(&self, p: Point) -> bool {
        if p == self.a || p == self.b {
            return false;
        }
        let (ax, ay, bx, by) = (nm(self.a.x), nm(self.a.y), nm(self.b.x), nm(self.b.y));
        let (px, py) = (nm(p.x), nm(p.y));
        let cross = (bx - ax) * (py - ay) - (by - ay) * (px - ax);
        cross == 0 && px >= ax.min(bx) && px <= ax.max(bx) && py >= ay.min(by) && py <= ay.max(by)
    }
}

fn nm(length: foilfern_core::types::Length) -> i128 {
    i128::from(length.to_nm())
}

fn mm(p: Point) -> (f64, f64) {
    p.to_mm()
}

/// Measures the layout of the scene's page. The scene must be built from
/// the same project state.
pub fn schematic_metrics(project: &Project, scene: &SchematicScene) -> Result<SchematicMetrics> {
    let schematic = project
        .schematic(scene.schematic())
        .ok_or(Error::SchematicNotFound(scene.schematic()))?;
    let lines = net_lines(project, schematic);
    let (symbol_fill, symbol_overlaps) = symbol_bodies(scene);
    Ok(SchematicMetrics {
        symbols: schematic.symbols().len(),
        content_bounds: scene.content_bounds().map(|r| [r.x0, r.y0, r.x1, r.y1]),
        symbol_fill,
        symbol_overlaps,
        wires: wire_metrics(schematic, &lines),
        nets: net_metrics(project, schematic)?,
        off_grid: off_grid(schematic),
    })
}

fn net_lines(project: &Project, schematic: &Schematic) -> Vec<Line> {
    let view = project.view();
    schematic
        .net_segments()
        .iter()
        .flat_map(|(id, segment)| {
            segment.lines().values().filter_map(move |line| {
                Some(Line {
                    segment: *id,
                    uuid: line.uuid(),
                    net: segment.net(),
                    a: schematic.net_line_anchor_position(*id, line.p1(), view)?,
                    b: schematic.net_line_anchor_position(*id, line.p2(), view)?,
                })
            })
        })
        .collect()
}

fn symbol_bodies(scene: &SchematicScene) -> (f64, Vec<SymbolOverlap>) {
    let mut bodies: BTreeMap<SymbolId, Rect> = BTreeMap::new();
    for (id, item) in scene.scene().items() {
        if !scene
            .scene()
            .layer(item.layer)
            .is_some_and(|layer| layer.visible)
        {
            continue;
        }
        let symbol = match scene.object(id) {
            Some(SchematicObject::Symbol(symbol) | SchematicObject::SymbolPin(symbol, _)) => symbol,
            _ => continue,
        };
        let bounds = item.bounding_box();
        bodies
            .entry(symbol)
            .and_modify(|rect| *rect = rect.union(bounds))
            .or_insert(bounds);
    }
    let fill = match scene.content_bounds() {
        Some(content) if content.area() > 0.0 => {
            bodies.values().map(Rect::area).sum::<f64>() / content.area()
        }
        _ => 0.0,
    };
    let mut bodies: Vec<_> = bodies.into_iter().collect();
    bodies.sort_by(|(_, a), (_, b)| a.x0.total_cmp(&b.x0));
    let mut overlaps = Vec::new();
    for (index, &(first, a)) in bodies.iter().enumerate() {
        for &(second, b) in bodies[index + 1..].iter().take_while(|(_, b)| b.x0 < a.x1) {
            let overlap = a.intersect(b);
            if overlap.width() > OVERLAP_EPS_MM && overlap.height() > OVERLAP_EPS_MM {
                let (first, second) = (first.min(second), first.max(second));
                overlaps.push(SymbolOverlap {
                    first,
                    second,
                    bounds: [overlap.x0, overlap.y0, overlap.x1, overlap.y1],
                });
            }
        }
    }
    overlaps.sort_by_key(|o| (o.first, o.second));
    (fill, overlaps)
}

fn wire_metrics(schematic: &Schematic, lines: &[Line]) -> WireMetrics {
    let mut metrics = WireMetrics {
        lines: lines.len(),
        ..WireMetrics::default()
    };
    for line in lines {
        let length = line.length_mm();
        metrics.total_length_mm += length;
        metrics.longest_mm = metrics.longest_mm.max(length);
        if !line.horizontal() && !line.vertical() {
            metrics.diagonal += 1;
        }
    }
    for (id, segment) in schematic.net_segments() {
        for junction in segment.junctions().keys() {
            let at: Vec<_> = segment
                .lines_at(NetLineAnchor::Junction(*junction))
                .filter_map(|l| {
                    lines
                        .iter()
                        .find(|line| (line.segment, line.uuid) == (*id, l.uuid()))
                })
                .collect();
            if let [a, b] = at[..]
                && a.horizontal() != b.horizontal()
                && (a.horizontal() || a.vertical())
                && (b.horizontal() || b.vertical())
            {
                metrics.bends += 1;
            }
        }
    }
    for (index, a) in lines.iter().enumerate() {
        for b in &lines[index + 1..] {
            if a.segment == b.segment {
                continue;
            }
            if let Some((from, to)) = collinear_overlap(a, b) {
                if a.net != b.net {
                    metrics.different_net_overlaps.push(WireOverlap {
                        first: a.id(),
                        second: b.id(),
                        from,
                        to,
                    });
                }
                continue;
            }
            if a.net != b.net {
                for (wire, touched) in [(a, b), (b, a)] {
                    for end in [wire.a, wire.b] {
                        if touched.interior_contains(end) {
                            metrics.foreign_contacts.push(ForeignWireContact {
                                wire: wire.id(),
                                touched: touched.id(),
                                position: mm(end),
                            });
                        }
                    }
                }
            }
            if properly_cross(a, b) {
                metrics.crossings += 1;
            }
        }
    }
    metrics
}

/// The shared stretch of two collinear lines, longer than the overlap
/// tolerance.
fn collinear_overlap(a: &Line, b: &Line) -> Option<((f64, f64), (f64, f64))> {
    let collinear = |p: Point| {
        let (ax, ay, bx, by) = (nm(a.a.x), nm(a.a.y), nm(a.b.x), nm(a.b.y));
        (bx - ax) * (nm(p.y) - ay) - (by - ay) * (nm(p.x) - ax) == 0
    };
    if a.a == a.b || !collinear(b.a) || !collinear(b.b) {
        return None;
    }
    // Project onto the dominant axis of `a`.
    let along_x = (a.b.x.to_nm() - a.a.x.to_nm()).abs() >= (a.b.y.to_nm() - a.a.y.to_nm()).abs();
    let key = |p: Point| if along_x { p.x.to_nm() } else { p.y.to_nm() };
    let (lo_a, hi_a) = (key(a.a).min(key(a.b)), key(a.a).max(key(a.b)));
    let (lo_b, hi_b) = (key(b.a).min(key(b.b)), key(b.a).max(key(b.b)));
    let (lo, hi) = (lo_a.max(lo_b), hi_a.min(hi_b));
    let factor = (a.length_mm()) / ((hi_a - lo_a) as f64 / 1e6);
    if ((hi - lo) as f64 / 1e6) * factor <= OVERLAP_EPS_MM {
        return None;
    }
    let point = |k: i64| {
        let t = (k - key(a.a)) as f64 / (key(a.b) - key(a.a)) as f64;
        let ((ax, ay), (bx, by)) = (mm(a.a), mm(a.b));
        (ax + t * (bx - ax), ay + t * (by - ay))
    };
    Some((point(lo), point(hi)))
}

/// Whether the lines cross at a point interior to both.
fn properly_cross(a: &Line, b: &Line) -> bool {
    let orient = |p: Point, q: Point, r: Point| {
        ((nm(q.x) - nm(p.x)) * (nm(r.y) - nm(p.y)) - (nm(q.y) - nm(p.y)) * (nm(r.x) - nm(p.x)))
            .signum()
    };
    let (d1, d2) = (orient(a.a, a.b, b.a), orient(a.a, a.b, b.b));
    let (d3, d4) = (orient(b.a, b.b, a.a), orient(b.a, b.b, a.b));
    d1 * d2 < 0 && d3 * d4 < 0
}

fn net_metrics(project: &Project, schematic: &Schematic) -> Result<NetMetrics> {
    let view = project.view();
    let mut segments_per_net: BTreeMap<NetSignalId, usize> = BTreeMap::new();
    let mut labels = 0;
    for segment in schematic.net_segments().values() {
        *segments_per_net.entry(segment.net()).or_default() += 1;
        labels += segment.labels().len();
    }
    let mut pins_per_net: BTreeMap<NetSignalId, Vec<Point>> = BTreeMap::new();
    let mut unwired_pins = 0;
    for symbol in schematic.symbols().values() {
        for pin in symbol.pins(view)? {
            let Some(net) = pin.net() else { continue };
            pins_per_net.entry(net).or_default().push(pin.position());
            if schematic.pin_net_segment(symbol.id(), pin.uuid()).is_none() {
                unwired_pins += 1;
            }
        }
    }
    let pin_span_mm = pins_per_net
        .values()
        .filter(|pins| pins.len() > 1)
        .map(|pins| {
            let span = |values: Vec<f64>| {
                let lo = values.iter().copied().fold(f64::MAX, f64::min);
                let hi = values.iter().copied().fold(f64::MIN, f64::max);
                hi - lo
            };
            span(pins.iter().map(|p| p.x.to_mm()).collect())
                + span(pins.iter().map(|p| p.y.to_mm()).collect())
        })
        .sum();
    let nets: BTreeSet<_> = segments_per_net.keys().chain(pins_per_net.keys()).collect();
    Ok(NetMetrics {
        nets: nets.len(),
        segments: schematic.net_segments().len(),
        labels,
        split_nets: segments_per_net
            .values()
            .filter(|count| **count > 1)
            .count(),
        pin_span_mm,
        unwired_pins,
    })
}

fn off_grid(schematic: &Schematic) -> OffGrid {
    let grid = schematic.grid_interval().get().to_nm();
    let off = |p: Point| p.x.to_nm() % grid != 0 || p.y.to_nm() % grid != 0;
    let mut result = OffGrid {
        symbols: schematic
            .symbols()
            .values()
            .filter(|s| off(s.position()))
            .count(),
        ..OffGrid::default()
    };
    for segment in schematic.net_segments().values() {
        result.junctions += segment
            .junctions()
            .values()
            .filter(|j| off(j.position()))
            .count();
        result.labels += segment
            .labels()
            .values()
            .filter(|l| off(l.position()))
            .count();
    }
    result
}
