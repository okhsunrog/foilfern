//! Net-level wiring for agents (no upstream counterpart: upstream users
//! draw every wire by hand): connect a list of component signals to a
//! net, drawing readable schematic wires, or disconnect signals.
//!
//! [`ConnectNet`] draws direct wires between nearby pins of a schematic
//! page (a minimum spanning tree, edges up to a maximum length, with one
//! corner) and connects far pins through short stub wires with net labels,
//! which is how upstream's schematics connect distant pins and pins on
//! other pages. All wires are drawn with the regular wire drawing logic
//! (net merging, forced net names).

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::{NetLabel, NetLineAnchor};
use librepcb_core::project::{
    ComponentInstanceId, ComponentSignalRef, Mutation, NetSegmentId, NetSegmentRef, NetSignalId,
    Project, SchematicId, SchematicMutation, SymbolId,
};
use librepcb_core::types::{
    Angle, CircuitIdentifier, Length, Point, PositiveLength, UnsignedLength, Uuid,
};
use librepcb_i18n::tr;

use super::circuit::{
    add_net, combine_net_signals, default_net_class, remove_unused_nets, set_component_signal_net,
};
use super::component::MoveSymbol;
use super::placement::{
    Rect, is_frame, net_label_direction, net_label_length, net_label_rect, placed_symbol_rect,
    schematic_obstacles,
};
use super::resolve;
use super::schematic::{
    DEFAULT_STUB_LENGTH, SchematicSelection, WireAnchor, WireMode, draw_wire, forced_net_name,
    net_label_orientation, remove_schematic_items,
};
use crate::editor::{Command, Transaction};
use crate::error::{Error, Result};

/// A placed symbol pin of a component signal.
#[derive(Debug, Clone, Copy)]
struct PlacedPin {
    schematic: SchematicId,
    symbol: SymbolId,
    pin: Uuid,
    position: Point,
    rotation: Angle,
}

impl PlacedPin {
    fn anchor(&self) -> WireAnchor {
        WireAnchor::SymbolPin {
            symbol: self.symbol,
            pin: self.pin,
        }
    }

    /// The end of a stub wire leaving the pin away from the symbol.
    fn stub_end(&self) -> Point {
        Point::new(self.position.x + DEFAULT_STUB_LENGTH, self.position.y)
            .rotated(self.rotation + Angle::DEG180, self.position)
    }

    /// Whether a wire leaves the pin horizontally.
    fn is_horizontal(&self) -> bool {
        let r = self.rotation.mapped_to_0_360deg();
        r == Angle::DEG0 || r == Angle::DEG180
    }
}

/// Returns the placed symbol pins of a component signal.
fn placed_pins(p: &Project, signal: ComponentSignalRef) -> Result<Vec<PlacedPin>> {
    let mut pins = Vec::new();
    let Some(uses) = p.component_uses(signal.component) else {
        return Ok(pins);
    };
    for (schematic, symbol) in uses.symbols.values() {
        let Some(sym) = p
            .schematic(*schematic)
            .and_then(|s| s.symbols().get(symbol))
        else {
            continue;
        };
        for view in sym.pins(p.view())? {
            if view.signal() == signal {
                pins.push(PlacedPin {
                    schematic: *schematic,
                    symbol: *symbol,
                    pin: view.uuid(),
                    position: view.position(),
                    rotation: view.rotation(),
                });
            }
        }
    }
    Ok(pins)
}

fn manhattan(a: Point, b: Point) -> Length {
    (a.x - b.x).abs() + (a.y - b.y).abs()
}

/// Draws a stub wire from a pin and puts a net label at its end (its text
/// runs back along the stub, like upstream's labels sit on their wire). The
/// stub is as long as the label and leaves the pin straight or with a short
/// jog sideways, whichever variant first keeps the label and the wire clear
/// of symbols, texts, other labels, wires and pins (see
/// [`label_stub_route()`]).
fn stub_with_label(
    tx: &mut Transaction<'_>,
    pin: &PlacedPin,
    net: &CircuitIdentifier,
) -> Result<()> {
    let (corners, end) = label_stub_route(tx.project(), pin, net.as_str().chars().count())?;
    let wire = draw_wire(
        tx,
        Some(pin.schematic),
        &pin.anchor(),
        &WireAnchor::Point(end),
        &corners,
        Some(net),
    )?;
    let before_end = corners.last().copied().unwrap_or(pin.position);
    let (rotation, mirrored) = net_label_orientation(end, before_end);
    tx.apply(Mutation::Schematic(SchematicMutation::AddNetLabel {
        segment: NetSegmentRef {
            schematic: pin.schematic,
            segment: wire.segment,
        },
        label: NetLabel::new(Uuid::new_random(), end, rotation, mirrored),
    }))
}

/// Things a new wire or net label must keep clear of on a schematic page.
struct Clearance {
    /// Symbols (with texts, frames excluded) and net labels.
    rects: Vec<Rect>,
    /// Existing net lines.
    lines: Vec<(Point, Point)>,
    /// Symbol pins and junctions.
    points: Vec<Point>,
}

impl Clearance {
    fn new(p: &Project, schematic: SchematicId) -> Result<Self> {
        let s = p
            .schematic(schematic)
            .ok_or_else(|| Error::not_found("Schematic", schematic))?;
        let mut rects = Vec::new();
        let mut points = Vec::new();
        for (id, sym) in s.symbols() {
            if !is_frame(p, schematic, *id)? {
                rects.push(placed_symbol_rect(p, schematic, *id, true)?);
            }
            points.extend(sym.pins(p.view())?.iter().map(|pin| pin.position()));
        }
        let mut lines = Vec::new();
        for (id, segment) in s.net_segments() {
            points.extend(segment.junctions().values().map(|j| j.position()));
            for line in segment.lines().values() {
                let ends =
                    [line.p1(), line.p2()].map(|x| s.net_line_anchor_position(*id, x, p.view()));
                if let [Some(p1), Some(p2)] = ends {
                    lines.push((p1, p2));
                }
            }
            let chars = p
                .circuit()
                .net_signal(segment.net())
                .map_or(8, |n| n.name().as_str().chars().count());
            for l in segment.labels().values() {
                let direction = net_label_direction(l.rotation(), l.mirrored());
                rects.push(net_label_rect(l.position(), direction, chars));
            }
        }
        Ok(Self {
            rects,
            lines,
            points,
        })
    }

    /// Whether a wire along `path` (starting at a pin) and a label of
    /// `chars` characters at its end, running back along the last segment
    /// in `direction`, are clear.
    fn is_clear(&self, path: &[Point], direction: (i64, i64), chars: usize) -> bool {
        let Some(end) = path.last().copied() else {
            return false;
        };
        let start = path[0];
        let label = net_label_rect(end, direction, chars);
        let label_clear = !self.rects.iter().any(|r| r.intersects(&label))
            && !self.points.iter().any(|x| {
                label.min.x <= x.x && x.x <= label.max.x && label.min.y <= x.y && x.y <= label.max.y
            })
            && !self
                .lines
                .iter()
                .any(|(a, b)| segment_crosses_rect(*a, *b, &label));
        label_clear
            && path.windows(2).all(|w| {
                let (a, b) = (w[0], w[1]);
                !self.rects.iter().any(|r| segment_crosses_rect(a, b, r))
                    && !self
                        .points
                        .iter()
                        .any(|x| *x != start && point_on_segment(*x, a, b))
                    && !self
                        .lines
                        .iter()
                        .any(|l| segments_overlap((a, b), *l) || segments_cross((a, b), *l))
            })
    }
}

/// Whether a horizontal and a vertical segment cross (at a point which is
/// not an end of both).
fn segments_cross(a: (Point, Point), b: (Point, Point)) -> bool {
    let cross = |h: (Point, Point), v: (Point, Point)| {
        let (hx0, hx1) = (h.0.x.min(h.1.x), h.0.x.max(h.1.x));
        let (vy0, vy1) = (v.0.y.min(v.1.y), v.0.y.max(v.1.y));
        let x = v.0.x;
        let y = h.0.y;
        hx0 <= x && x <= hx1 && vy0 <= y && y <= vy1 && {
            let at = Point::new(x, y);
            !((at == h.0 || at == h.1) && (at == v.0 || at == v.1))
        }
    };
    let horizontal = |s: (Point, Point)| s.0.y == s.1.y && s.0.x != s.1.x;
    let vertical = |s: (Point, Point)| s.0.x == s.1.x && s.0.y != s.1.y;
    (horizontal(a) && vertical(b) && cross(a, b)) || (vertical(a) && horizontal(b) && cross(b, a))
}

/// Returns the corner points and the end of a stub wire with a net label
/// for `pin` (see [`stub_with_label()`]): the first clear variant of a
/// straight stub (as long as the label, or 2.54 mm longer), or a stub with
/// a jog of 2.54 to 7.62 mm to either side; the straight stub if none is
/// clear.
fn label_stub_route(p: &Project, pin: &PlacedPin, chars: usize) -> Result<(Vec<Point>, Point)> {
    let clearance = Clearance::new(p, pin.schematic)?;
    let out = pin.stub_end();
    let direction = (
        (out.x - pin.position.x).to_nm().signum(),
        (out.y - pin.position.y).to_nm().signum(),
    );
    let (dx, dy) = direction;
    let back = (-dx, -dy);
    let g = DEFAULT_STUB_LENGTH;
    // Label length plus some wire, in grid units.
    let needed = (net_label_length(chars) + Length::new(1_000_000)).to_nm();
    let label_units = ((needed + g.to_nm() - 1) / g.to_nm()).max(1);
    let at = |from: Point, (x, y): (i64, i64), n: i64| {
        Point::new(from.x + g * x * n, from.y + g * y * n)
    };
    let perpendicular = (-dy, dx);
    let mut candidates: Vec<Vec<Point>> = vec![
        vec![pin.position, at(pin.position, direction, label_units)],
        vec![pin.position, at(pin.position, direction, label_units + 1)],
    ];
    for k in [1, -1, 2, -2, 3, -3] {
        let c1 = at(pin.position, direction, 1);
        let c2 = at(c1, perpendicular, k);
        let end = at(c2, direction, label_units);
        candidates.push(vec![pin.position, c1, c2, end]);
    }
    for path in &candidates {
        if clearance.is_clear(path, back, chars) {
            let end = path[path.len() - 1];
            return Ok((path[1..path.len() - 1].to_vec(), end));
        }
    }
    Ok((Vec::new(), candidates[0][1]))
}

/// Connects component signals to one net (created if needed, other nets
/// of the signals are merged into it) and draws the schematic wiring:
/// pins on the same page closer than `max_wire_length` are wired directly
/// (shortest connections first, one corner per wire), all other pins get a
/// short stub wire with a net label. If the net consists of several wire
/// groups afterwards, each group gets a net label, so the schematic shows
/// the connection. Forced net names of the signals (supply symbols) take
/// precedence over `net`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConnectNet {
    /// Name of the net (default: the forced net name of a signal, else the
    /// net of the first connected signal, else a new automatic name).
    #[serde(default)]
    pub net: Option<CircuitIdentifier>,
    /// The component signals.
    pub signals: Vec<ComponentSignalRef>,
    /// Maximum length (Manhattan) of direct wires between pins (default:
    /// 25.4 mm; zero: only net labels).
    #[serde(default)]
    pub max_wire_length: Option<UnsignedLength>,
}

/// Result of [`ConnectNet`].
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConnectedNet {
    /// The net.
    pub net: Option<NetSignalId>,
    /// Its name.
    pub name: String,
    /// Number of direct wires drawn between pins.
    pub wires: usize,
    /// Number of net labels added (each with a stub wire).
    pub labels: usize,
    /// Nets which were merged into the net.
    pub merged_nets: Vec<String>,
    /// Unconnected supply symbols (designators) which were attached to the
    /// net because their forced net name is the net name.
    #[serde(default)]
    pub attached_supplies: Vec<String>,
    /// Hints about what could not be done as requested.
    pub warnings: Vec<String>,
}

impl Command for ConnectNet {
    type Output = ConnectedNet;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::SchematicEditorState_DrawWire",
            "Draw Wire"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<ConnectedNet> {
        if self.signals.is_empty() {
            return Err(Error::InvalidArgument("No pins to connect.".to_owned()));
        }
        let mut result = ConnectedNet::default();
        let p = tx.project();
        let mut signals = Vec::new();
        for s in &self.signals {
            p.circuit()
                .component_instance(s.component)
                .and_then(|c| c.signal(&s.signal))
                .ok_or_else(|| Error::not_found("Component signal", s.signal))?;
            if !signals.contains(s) {
                signals.push(*s);
            }
        }
        let current_net = |p: &Project, s: &ComponentSignalRef| {
            p.circuit()
                .component_instance(s.component)
                .and_then(|c| c.signal(&s.signal))
                .and_then(|s| s.net())
        };

        // The net name.
        let forced: BTreeSet<CircuitIdentifier> = signals
            .iter()
            .filter_map(|s| forced_net_name(p, *s))
            .collect();
        if forced.len() > 1 {
            return Err(Error::InvalidArgument(format!(
                "The pins force different net names: {}.",
                forced
                    .iter()
                    .map(|n| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let name = match (self.net, forced.into_iter().next()) {
            (Some(n), Some(f)) if n != f => {
                result.warnings.push(format!(
                    "A pin forces the net name \"{}\"; it is used instead of \"{}\".",
                    f.as_str(),
                    n.as_str()
                ));
                Some(f)
            }
            (n, f) => n.or(f),
        };

        // The target net.
        let named = name
            .as_ref()
            .and_then(|n| p.circuit().net_signal_by_name(n.as_str()).map(|(id, _)| id));
        let existing = signals.iter().find_map(|s| current_net(p, s));
        let target = match (named, existing) {
            (Some(id), _) => id,
            (None, Some(id)) => {
                if let Some(name) = &name {
                    let mut net = p
                        .circuit()
                        .net_signal(id)
                        .ok_or_else(|| Error::not_found("Net", id))?
                        .clone();
                    net.set_name(name.clone(), false);
                    tx.apply(Mutation::UpdateNetSignal(net))?;
                }
                id
            }
            (None, None) => {
                let class = default_net_class(tx)?;
                add_net(tx, name.clone(), class)?
            }
        };

        // Merge other nets of the signals.
        for s in &signals {
            if let Some(other) = current_net(tx.project(), s)
                && other != target
            {
                result
                    .merged_nets
                    .push(resolve::net_name(tx.project(), Some(other)));
                combine_net_signals(tx, other, target)?;
            }
        }
        let net_name = tx
            .project()
            .circuit()
            .net_signal(target)
            .ok_or_else(|| Error::not_found("Net", target))?
            .name()
            .clone();

        // Unconnected supply symbols of this net name (e.g. a "GND" symbol
        // when connecting net "GND") are attached too; they are moved next
        // to the nearest pin below.
        let p = tx.project();
        let mut supplies: Vec<(String, ComponentSignalRef)> = p
            .circuit()
            .component_instances()
            .keys()
            .filter_map(|id| {
                let (signal, name) = supply_signal(p, *id)?;
                let unconnected = current_net(p, &signal).is_none();
                (name == net_name && unconnected && !signals.contains(&signal))
                    .then(|| (resolve::component_name(p, *id), signal))
            })
            .collect();
        supplies.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, signal) in supplies {
            signals.push(signal);
            result.attached_supplies.push(name);
        }

        // Placed pins per schematic.
        let mut pages: BTreeMap<SchematicId, Vec<PlacedPin>> = BTreeMap::new();
        for s in &signals {
            let pins = placed_pins(tx.project(), *s)?;
            match pins.first() {
                Some(pin) => pages.entry(pin.schematic).or_default().push(*pin),
                None => {
                    set_component_signal_net(tx, *s, Some(target))?;
                    result.warnings.push(format!(
                        "{} has no placed symbol pin; it is connected without wire.",
                        signal_name(tx.project(), *s)
                    ));
                }
            }
        }
        let max_len = self.max_wire_length.map_or(Length::new(25_400_000), |l| *l);

        for (schematic, mut pins) in pages {
            let schematic = &schematic;
            // Supply symbols (e.g. GND) are moved next to the pin they
            // connect to, like a human would draw them.
            snap_supply_symbols(tx, *schematic, &mut pins)?;

            // Nodes: requested pins without wire, and the segments of the
            // requested pins which are wired already.
            let p = tx.project();
            let s = p
                .schematic(*schematic)
                .ok_or_else(|| Error::not_found("Schematic", schematic))?;
            let mut nodes: Vec<Vec<PlacedPin>> = Vec::new();
            let mut node_segment: Vec<Option<NetSegmentId>> = Vec::new();
            for pin in &pins {
                match s.pin_net_segment(pin.symbol, pin.pin) {
                    Some(seg) => match node_segment.iter().position(|n| *n == Some(seg)) {
                        Some(i) => nodes[i].push(*pin),
                        None => {
                            nodes.push(vec![*pin]);
                            node_segment.push(Some(seg));
                        }
                    },
                    None => {
                        nodes.push(vec![*pin]);
                        node_segment.push(None);
                    }
                }
            }
            // Minimum spanning tree (Prim) over the nodes.
            let n = nodes.len();
            let dist = |a: &[PlacedPin], b: &[PlacedPin]| {
                let mut best: Option<(Length, PlacedPin, PlacedPin)> = None;
                for x in a {
                    for y in b {
                        let d = manhattan(x.position, y.position);
                        if d > Length::ZERO && best.is_none_or(|(bd, _, _)| d < bd) {
                            best = Some((d, *x, *y));
                        }
                    }
                }
                best
            };
            let mut in_tree = vec![false; n];
            let mut group: Vec<usize> = (0..n).collect();
            let mut edges = Vec::new();
            if n > 0 {
                in_tree[0] = true;
            }
            for _ in 1..n {
                let mut best: Option<(Length, usize, PlacedPin, PlacedPin)> = None;
                for i in (0..n).filter(|i| in_tree[*i]) {
                    for j in (0..n).filter(|j| !in_tree[*j]) {
                        if let Some((d, a, b)) = dist(&nodes[i], &nodes[j])
                            && best.is_none_or(|(bd, ..)| d < bd)
                        {
                            best = Some((d, j, a, b));
                        }
                    }
                }
                let Some((d, j, a, b)) = best else {
                    break;
                };
                in_tree[j] = true;
                if d <= max_len {
                    edges.push((a, b));
                }
            }
            // Draw the wires (shortest first) which have a clean route (not
            // through symbols, pins or along other wires); the other
            // connections get net labels below.
            edges.sort_by_key(|(a, b)| manhattan(a.position, b.position));
            let node_of = |pin: &PlacedPin| {
                nodes
                    .iter()
                    .position(|n| n.iter().any(|x| x.symbol == pin.symbol && x.pin == pin.pin))
                    .unwrap_or(0)
            };
            for (a, b) in edges {
                let Some(points) = clean_route(tx.project(), *schematic, &a, &b)? else {
                    continue;
                };
                draw_wire(
                    tx,
                    Some(*schematic),
                    &a.anchor(),
                    &b.anchor(),
                    &points,
                    Some(&net_name),
                )?;
                result.wires += 1;
                let (ga, gb) = (group[node_of(&a)], group[node_of(&b)]);
                for g in group.iter_mut() {
                    if *g == gb {
                        *g = ga;
                    }
                }
            }
            // Pins which are still unwired get a stub with a label.
            for (i, node) in nodes.iter().enumerate() {
                let alone = group.iter().filter(|g| **g == group[i]).count() == 1;
                if alone && node_segment[i].is_none() {
                    stub_with_label(tx, &node[0], &net_name)?;
                    result.labels += 1;
                }
            }
        }

        // If the net consists of several segments, every segment needs a
        // label to show the connection.
        let segments = resolve::schematic_segments_of_net(tx.project(), target);
        if segments.len() > 1 {
            for (schematic, segment) in segments {
                let p = tx.project();
                let Some(seg) = p
                    .schematic(schematic)
                    .and_then(|s| s.net_segments().get(&segment))
                else {
                    continue;
                };
                if !seg.labels().is_empty() {
                    continue;
                }
                let pins: Vec<(SymbolId, Uuid)> = seg.connected_pins().into_iter().collect();
                let s = p
                    .schematic(schematic)
                    .ok_or_else(|| Error::not_found("Schematic", schematic))?;
                let mut placed = Vec::new();
                for (symbol, pin) in pins {
                    if let Some(view) = s
                        .symbols()
                        .get(&symbol)
                        .and_then(|sym| sym.pin(p.view(), pin).ok().flatten())
                    {
                        placed.push(PlacedPin {
                            schematic,
                            symbol,
                            pin,
                            position: view.position(),
                            rotation: view.rotation(),
                        });
                    }
                }
                // A supply symbol shows the net name already.
                if placed.iter().any(|pin| is_supply_pin(p, pin)) {
                    continue;
                }
                // Label on the middle of the longest line, else a stub.
                let longest = seg
                    .lines()
                    .values()
                    .filter_map(|l| {
                        let a = s.net_line_anchor_position(segment, l.p1(), p.view())?;
                        let b = s.net_line_anchor_position(segment, l.p2(), p.view())?;
                        Some((a, b))
                    })
                    .filter(|(a, b)| a.x == b.x || a.y == b.y)
                    .max_by_key(|(a, b)| manhattan(*a, *b));
                match (longest, placed.first()) {
                    (Some((a, b)), _) if manhattan(a, b) >= Length::new(5_080_000) => {
                        let middle = Point::new((a.x + b.x) / 2, (a.y + b.y) / 2).mapped_to_grid(
                            PositiveLength::new(Length::new(1_270_000))
                                .map_err(librepcb_core::project::Error::from)?,
                        );
                        let rotation = if a.x == b.x {
                            Angle::DEG90
                        } else {
                            Angle::DEG0
                        };
                        tx.apply(Mutation::Schematic(SchematicMutation::AddNetLabel {
                            segment: NetSegmentRef { schematic, segment },
                            label: NetLabel::new(Uuid::new_random(), middle, rotation, false),
                        }))?;
                    }
                    (_, Some(pin)) => {
                        let pin = *pin;
                        stub_with_label(tx, &pin, &net_name)?;
                    }
                    (_, None) => {
                        let Some(junction) = seg.junctions().values().next() else {
                            continue;
                        };
                        let position = junction.position();
                        tx.apply(Mutation::Schematic(SchematicMutation::AddNetLabel {
                            segment: NetSegmentRef { schematic, segment },
                            label: NetLabel::new(Uuid::new_random(), position, Angle::DEG0, false),
                        }))?;
                    }
                }
                result.labels += 1;
            }
        }
        remove_unused_nets(tx)?;
        result.net = Some(target);
        result.name = net_name.to_string();
        Ok(result)
    }
}

/// Whether the axis-aligned segment `a`-`b` passes through the interior
/// of `r`.
fn segment_crosses_rect(a: Point, b: Point, r: &Rect) -> bool {
    let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
    let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
    if x0 == x1 {
        r.min.x < x0 && x0 < r.max.x && y1 > r.min.y && y0 < r.max.y
    } else if y0 == y1 {
        r.min.y < y0 && y0 < r.max.y && x1 > r.min.x && x0 < r.max.x
    } else {
        // Not axis aligned: check the bounding box (conservative).
        x1 > r.min.x && x0 < r.max.x && y1 > r.min.y && y0 < r.max.y
    }
}

/// Whether `p` lies on the axis-aligned segment `a`-`b`.
fn point_on_segment(p: Point, a: Point, b: Point) -> bool {
    let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
    let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
    let in_box = x0 <= p.x && p.x <= x1 && y0 <= p.y && p.y <= y1;
    in_box && (x0 == x1 || y0 == y1)
}

/// Whether two axis-aligned segments overlap collinearly (more than a
/// point).
fn segments_overlap(a: (Point, Point), b: (Point, Point)) -> bool {
    let horizontal = |s: (Point, Point)| s.0.y == s.1.y && s.0.x != s.1.x;
    let vertical = |s: (Point, Point)| s.0.x == s.1.x && s.0.y != s.1.y;
    let overlap = |a0: Length, a1: Length, b0: Length, b1: Length| {
        a0.max(a1).min(b0.max(b1)) > a0.min(a1).max(b0.min(b1))
    };
    if horizontal(a) && horizontal(b) && a.0.y == b.0.y {
        overlap(a.0.x, a.1.x, b.0.x, b.1.x)
    } else if vertical(a) && vertical(b) && a.0.x == b.0.x {
        overlap(a.0.y, a.1.y, b.0.y, b.1.y)
    } else {
        false
    }
}

/// Returns the corner points of a clean wire from `a` to `b` (straight or
/// with one corner, preferring to leave `a` in its pin direction), or
/// `None` if every variant passes through a symbol body or text, over
/// another pin or junction, or along or across an existing wire.
fn clean_route(
    p: &Project,
    schematic: SchematicId,
    a: &PlacedPin,
    b: &PlacedPin,
) -> Result<Option<Vec<Point>>> {
    let s = p
        .schematic(schematic)
        .ok_or_else(|| Error::not_found("Schematic", schematic))?;
    let mut bodies = Vec::new();
    let mut points = Vec::new();
    for (id, sym) in s.symbols() {
        if !is_frame(p, schematic, *id)? {
            bodies.push(placed_symbol_rect(p, schematic, *id, true)?);
        }
        for pin in sym.pins(p.view())? {
            points.push(pin.position());
        }
    }
    let mut lines = Vec::new();
    for (id, segment) in s.net_segments() {
        for junction in segment.junctions().values() {
            points.push(junction.position());
        }
        for line in segment.lines().values() {
            let ends = [line.p1(), line.p2()].map(|x| s.net_line_anchor_position(*id, x, p.view()));
            if let [Some(p1), Some(p2)] = ends {
                lines.push((p1, p2));
            }
        }
    }
    let modes = if a.is_horizontal() {
        [WireMode::HV, WireMode::VH]
    } else {
        [WireMode::VH, WireMode::HV]
    };
    for mode in modes {
        let middle = mode.middle_point(a.position, b.position);
        let corners: Vec<Point> = [middle]
            .into_iter()
            .filter(|m| *m != a.position && *m != b.position)
            .collect();
        let mut path = vec![a.position];
        path.extend(corners.iter().copied());
        path.push(b.position);
        let clean = path.windows(2).all(|w| {
            let (p1, p2) = (w[0], w[1]);
            !bodies.iter().any(|r| segment_crosses_rect(p1, p2, r))
                && !points
                    .iter()
                    .any(|x| *x != a.position && *x != b.position && point_on_segment(*x, p1, p2))
                && !lines
                    .iter()
                    .any(|l| segments_overlap((p1, p2), *l) || segments_cross((p1, p2), *l))
        });
        if clean {
            return Ok(Some(corners));
        }
    }
    Ok(None)
}

/// Returns the signal and forced net name of a supply symbol component: a
/// schematic-only component with a single signal forcing its net name.
pub(crate) fn supply_signal(
    p: &Project,
    component: ComponentInstanceId,
) -> Option<(ComponentSignalRef, CircuitIdentifier)> {
    let instance = p.circuit().component_instance(component)?;
    let lib = p.library().component(&instance.lib_component())?;
    if !lib.schematic_only() || lib.signals().len() != 1 {
        return None;
    }
    let signal = ComponentSignalRef {
        component,
        signal: lib.signals().iter().next()?.uuid(),
    };
    Some((signal, forced_net_name(p, signal)?))
}

/// Whether a placed pin belongs to a supply symbol (see
/// [`supply_signal()`]).
fn is_supply_pin(p: &Project, pin: &PlacedPin) -> bool {
    p.schematic(pin.schematic)
        .and_then(|s| s.symbols().get(&pin.symbol))
        .is_some_and(|sym| supply_signal(p, sym.component()).is_some())
}

/// Moves unwired supply symbols of `pins` next to the nearest other
/// (unwired) pin, with the supply pin 5.08 mm in front of it, if the
/// symbol fits there without overlapping other symbols. Updates the pin
/// positions.
fn snap_supply_symbols(
    tx: &mut Transaction<'_>,
    schematic: SchematicId,
    pins: &mut [PlacedPin],
) -> Result<()> {
    let p = tx.project();
    let Some(s) = p.schematic(schematic) else {
        return Ok(());
    };
    let unwired = |pin: &PlacedPin| s.pin_net_segment(pin.symbol, pin.pin).is_none();
    let supplies: Vec<usize> = (0..pins.len())
        .filter(|i| unwired(&pins[*i]) && is_supply_pin(p, &pins[*i]))
        .collect();
    let mut targets: Vec<usize> = (0..pins.len())
        .filter(|i| !supplies.contains(i) && unwired(&pins[*i]))
        .collect();
    for i in supplies {
        let supply = pins[i];
        let Some((k, _)) = targets
            .iter()
            .enumerate()
            .min_by_key(|(_, t)| manhattan(pins[**t].position, supply.position))
        else {
            break;
        };
        let target = pins[targets[k]];
        let p = tx.project();
        let Some(sym) = p
            .schematic(schematic)
            .and_then(|s| s.symbols().get(&supply.symbol))
        else {
            continue;
        };
        let offset = supply.position - sym.position();
        let end = Point::new(
            target.position.x + Length::new(5_080_000),
            target.position.y,
        )
        .rotated(target.rotation + Angle::DEG180, target.position);
        let new_position = end - offset;
        let delta = new_position - sym.position();
        let rect = placed_symbol_rect(p, schematic, supply.symbol, true)?.translated(delta);
        let exclude: BTreeSet<SymbolId> = [supply.symbol].into();
        let free = !schematic_obstacles(p, schematic, &exclude, false)?
            .iter()
            .any(|o| o.intersects(&rect));
        if !free {
            continue;
        }
        tx.run(MoveSymbol {
            symbol: supply.symbol,
            position: Some(new_position),
            rotation: None,
            mirrored: None,
        })?;
        pins[i].position = end;
        targets.remove(k);
    }
    Ok(())
}

/// Returns `<component>:<signal>` for messages.
fn signal_name(p: &Project, s: ComponentSignalRef) -> String {
    let signal = p
        .circuit()
        .component_instance(s.component)
        .and_then(|c| p.library().component(&c.lib_component()))
        .and_then(|c| c.signals().by_uuid(&s.signal))
        .map_or_else(|| s.signal.to_string(), |sig| sig.name().to_string());
    format!("{}.{signal}", resolve::component_name(p, s.component))
}

/// Disconnects component signals from their nets: removes the wires at
/// their symbol pins (the rest of the wiring stays, split where needed)
/// and the traces at their pads, then removes nets which became unused.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DisconnectSignals {
    /// The component signals.
    pub signals: Vec<ComponentSignalRef>,
}

impl Command for DisconnectSignals {
    /// The number of signals which were connected.
    type Output = usize;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdRemoveSelectedSchematicItems",
            "Remove Schematic Elements"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<usize> {
        let mut count = 0;
        let mut selections: BTreeMap<SchematicId, SchematicSelection> = BTreeMap::new();
        for signal in &self.signals {
            let p = tx.project();
            let instance = p
                .circuit()
                .component_instance(signal.component)
                .and_then(|c| c.signal(&signal.signal))
                .ok_or_else(|| Error::not_found("Component signal", signal.signal))?;
            if instance.net().is_some() {
                count += 1;
            }
            for pin in placed_pins(p, *signal)? {
                let Some(s) = p.schematic(pin.schematic) else {
                    continue;
                };
                let Some(segment) = s.pin_net_segment(pin.symbol, pin.pin) else {
                    continue;
                };
                let Some(seg) = s.net_segments().get(&segment) else {
                    continue;
                };
                let selection = selections.entry(pin.schematic).or_default();
                for line in seg.lines_at(NetLineAnchor::Pin {
                    symbol: pin.symbol.0,
                    pin: pin.pin,
                }) {
                    selection.net_lines.push((segment, line.uuid()));
                }
            }
        }
        // Other pins which would lose all their lines (e.g. the other end
        // of a direct wire) stay on the net: they get a stub with a label
        // first, so the removal keeps them connected.
        let requested: BTreeSet<ComponentSignalRef> = self.signals.iter().copied().collect();
        let mut keep = Vec::new();
        for (schematic, selection) in &selections {
            let p = tx.project();
            let Some(s) = p.schematic(*schematic) else {
                continue;
            };
            let removed: BTreeSet<Uuid> = selection.net_lines.iter().map(|(_, l)| *l).collect();
            let segments: BTreeSet<NetSegmentId> =
                selection.net_lines.iter().map(|(s, _)| *s).collect();
            for segment in segments {
                let Some(seg) = s.net_segments().get(&segment) else {
                    continue;
                };
                let Some(net) = p.circuit().net_signal(seg.net()) else {
                    continue;
                };
                for (symbol, pin) in seg.connected_pins() {
                    let Some(view) = s
                        .symbols()
                        .get(&symbol)
                        .and_then(|sym| sym.pin(p.view(), pin).ok().flatten())
                    else {
                        continue;
                    };
                    let all_removed = seg
                        .lines_at(NetLineAnchor::Pin {
                            symbol: symbol.0,
                            pin,
                        })
                        .all(|l| removed.contains(&l.uuid()));
                    if all_removed && !requested.contains(&view.signal()) {
                        keep.push((
                            PlacedPin {
                                schematic: *schematic,
                                symbol,
                                pin,
                                position: view.position(),
                                rotation: view.rotation(),
                            },
                            net.name().clone(),
                        ));
                    }
                }
            }
        }
        for (pin, net) in &keep {
            stub_with_label(tx, pin, net)?;
        }
        for (schematic, selection) in selections {
            remove_schematic_items(tx, schematic, &selection)?;
        }
        for signal in &self.signals {
            set_component_signal_net(tx, *signal, None)?;
        }
        remove_unused_nets(tx)?;
        Ok(count)
    }
}
